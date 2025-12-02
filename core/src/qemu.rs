// core/src/qemu.rs

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use tracing::{debug, info, warn};

use crate::config::{DevicesConfig, PciDeviceConfig};
use crate::errors::{ChalybsError, Result};
use crate::model::{QemuState, VmRuntime};

/// Preflight checks for QEMU and firmware paths.
pub fn preflight(rt: &VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    if !Path::new(&q.binary).exists() {
        return Err(ChalybsError::Qemu(format!(
            "QEMU binary not found: {}",
            q.binary
        )));
    }

    if !Path::new(&q.ovmf_code).exists() {
        return Err(ChalybsError::Qemu(format!(
            "OVMF code not found: {}",
            q.ovmf_code
        )));
    }

    if !Path::new(&q.ovmf_vars).exists() {
        return Err(ChalybsError::Qemu(format!(
            "OVMF vars not found: {}",
            q.ovmf_vars
        )));
    }

    Ok(())
}

/// Derive QMP socket path for a given VM name.
fn qmp_path_for_vm(vm_name: &str) -> String {
    format!("/run/chalybs/{vm_name}.qmp")
}

/// Build the QEMU -cpu argument from config:
///   - If cpu_extras is present, compose:
///       "<abi or host>,<topo>,<hv_contexts>,<vendor_id>"
///     skipping any empty components.
///   - Otherwise, fall back to "host".
fn build_cpu_arg(rt: &VmRuntime) -> String {
    let q = &rt.cfg.qemu;

    if let Some(extra) = &q.cpu_extras {
        let mut parts = Vec::new();

        let base = extra
            .abi
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("host");
        parts.push(base.to_string());

        if let Some(topo) = extra.topo.as_deref() {
            if !topo.trim().is_empty() {
                parts.push(topo.trim().to_string());
            }
        }

        if let Some(hv) = extra.hv_contexts.as_deref() {
            if !hv.trim().is_empty() {
                parts.push(hv.trim().to_string());
            }
        }

        if let Some(vendor) = extra.vendor_id.as_deref() {
            if !vendor.trim().is_empty() {
                parts.push(vendor.trim().to_string());
            }
        }

        parts.join(",")
    } else {
        "host".to_string()
    }
}

/// Inject SMBIOS configuration into the QEMU command (if configured).
fn apply_smbios_args(cmd: &mut Command, rt: &VmRuntime) {
    let q = &rt.cfg.qemu;
    let Some(smb) = &q.smbios else {
        return;
    };

    // type=0: BIOS
    let mut t0_parts = Vec::new();
    if let Some(v) = smb.bios_vendor.as_deref() {
        if !v.is_empty() {
            t0_parts.push(format!("vendor={v}"));
        }
    }
    if let Some(v) = smb.bios_version.as_deref() {
        if !v.is_empty() {
            t0_parts.push(format!("version={v}"));
        }
    }
    if let Some(v) = smb.bios_date.as_deref() {
        if !v.is_empty() {
            t0_parts.push(format!("date={v}"));
        }
    }
    if !t0_parts.is_empty() {
        cmd.arg("-smbios")
            .arg(format!("type=0,{}", t0_parts.join(",")));
    }

    // type=1: system
    let mut t1_parts = Vec::new();
    if let Some(v) = smb.system_manufacturer.as_deref() {
        if !v.is_empty() {
            t1_parts.push(format!("manufacturer={v}"));
        }
    }
    if let Some(v) = smb.system_product_name.as_deref() {
        if !v.is_empty() {
            t1_parts.push(format!("product={v}"));
        }
    }
    if let Some(v) = smb.system_uuid.as_deref() {
        if !v.is_empty() {
            t1_parts.push(format!("uuid={v}"));
        }
    }
    if !t1_parts.is_empty() {
        cmd.arg("-smbios")
            .arg(format!("type=1,{}", t1_parts.join(",")));
    }

    // type=2: baseboard
    let mut t2_parts = Vec::new();
    if let Some(v) = smb.baseboard_manufacturer.as_deref() {
        if !v.is_empty() {
            t2_parts.push(format!("manufacturer={v}"));
        }
    }
    if let Some(v) = smb.baseboard_product.as_deref() {
        if !v.is_empty() {
            t2_parts.push(format!("product={v}"));
        }
    }
    if !t2_parts.is_empty() {
        cmd.arg("-smbios")
            .arg(format!("type=2,{}", t2_parts.join(",")));
    }
}

/// Wire VFIO PCIe devices from VmConfig into the QEMU command line.
///
/// This is the missing "attach devices to the VM" phase:
///   - For each GPU, NVMe, NIC, and USB device configured in vm.cfg.devices,
///     emit a corresponding `-device vfio-pci,host=...`.
///   - For the first GPU entry, we additionally set `multifunction=on,x-vga=on`
///     to mirror the typical primary-GPU passthrough semantics used in your
///     legacy Bash suite (video + audio function pair).
fn add_vfio_pci_devices(cmd: &mut Command, rt: &VmRuntime) {
    let DevicesConfig {
        gpu,
        nvme,
        nic,
        usb,
    } = &rt.cfg.devices;

    // GPUs: first entry treated as primary video function.
    if let Some(gpus) = gpu.as_ref() {
        for (idx, dev) in gpus.iter().enumerate() {
            let mut params = format!("host={}", dev.pci_address);

            if idx == 0 && gpus.len() > 1 {
                // Primary GPU function (e.g., 0000:4a:00.0 when paired with
                // 0000:4a:00.1 for audio). We mark it as multifunction + x-vga.
                params.push_str(",multifunction=on,x-vga=on");
            }

            cmd.arg("-device").arg(format!("vfio-pci,{params}"));

            info!(
                pci = %dev.pci_address,
                "qemu: attached GPU passthrough device via vfio-pci"
            );
        }
    }

    // Helper for non-GPU kinds: plain vfio-pci attachment.
    fn add_generic_list(cmd: &mut Command, list: &Option<Vec<PciDeviceConfig>>, kind: &str) {
        if let Some(devs) = list.as_ref() {
            for dev in devs {
                cmd.arg("-device")
                    .arg(format!("vfio-pci,host={}", dev.pci_address));

                info!(
                    pci = %dev.pci_address,
                    kind,
                    "qemu: attached {kind} passthrough device via vfio-pci"
                );
            }
        }
    }

    add_generic_list(cmd, nvme, "NVMe");
    add_generic_list(cmd, nic, "NIC");
    add_generic_list(cmd, usb, "USB");
}

/// Inject RTC arguments based on QemuConfig.
///
/// Behavior:
///   - If q.rtc = Some(non-empty), emit exactly `-rtc <value>`
///   - If q.rtc = Some("") (empty/whitespace), emit nothing (QEMU default)
///   - If q.rtc = None, emit the legacy Bash default:
///       `-rtc base=localtime,driftfix=slew`
fn apply_rtc_args(cmd: &mut Command, rt: &VmRuntime) {
    let q = &rt.cfg.qemu;

    match q.rtc.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(val) => {
            cmd.arg("-rtc").arg(val);
            info!(rtc = %val, "qemu: using explicit RTC policy from config");
        }
        None => {
            // Default: mirror Bash suite behavior for Windows guests.
            let default_rtc = "base=localtime,driftfix=slew";
            cmd.arg("-rtc").arg(default_rtc);
            info!(
                rtc = %default_rtc,
                "qemu: using default RTC policy (localtime + driftfix)"
            );
        }
    }
}

/// Launch QEMU and move it into the vm cpuset.
pub fn launch(rt: &mut VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;
    let vm_name = &rt.name;

    // Ensure /run/chalybs exists for QMP sockets, etc.
    fs::create_dir_all("/run/chalybs")
        .map_err(|e| ChalybsError::Qemu(format!("failed to create /run/chalybs: {e}")))?;

    let qmp_path = qmp_path_for_vm(vm_name);

    let mut cmd = Command::new(&q.binary);

    // 1) Pre-arguments from config (inserted before core args).
    if let Some(pre) = q.pre_args.as_ref() {
        for tok in pre.split_whitespace() {
            if !tok.is_empty() {
                cmd.arg(tok);
            }
        }
    }

    // 2) Core Chalybs-managed arguments.
    let cpu_arg = build_cpu_arg(rt);

    cmd.arg("-enable-kvm")
        .arg("-cpu")
        .arg(cpu_arg)
        .arg("-smp")
        .arg(q.num_vcpus.to_string())
        .arg("-m")
        .arg(q.mem_mb.to_string())
        .arg("-machine")
        .arg("q35,accel=kvm")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,readonly,file={}",
            q.ovmf_code
        ))
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,file={}", q.ovmf_vars))
        // QMP socket for deterministic vCPU discovery.
        .arg("-qmp")
        .arg(format!("unix:{},server,nowait", qmp_path));

    // 3) RTC configuration (mirrors legacy Bash default unless overridden).
    apply_rtc_args(&mut cmd, rt);

    // 4) SMBIOS configuration (if any).
    apply_smbios_args(&mut cmd, rt);

    // 5) VFIO PCI devices from VmConfig (GPU, NVMe, NIC, USB).
    add_vfio_pci_devices(&mut cmd, rt);

    // 6) Mid-section extra args (historical `args` field).
    if !q.args.trim().is_empty() {
        for tok in q.args.split_whitespace() {
            if !tok.is_empty() {
                cmd.arg(tok);
            }
        }
    }

    // 7) Post-arguments (final overrides).
    if let Some(post) = q.post_args.as_ref() {
        for tok in post.split_whitespace() {
            if !tok.is_empty() {
                cmd.arg(tok);
            }
        }
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    debug!("launching QEMU: {:?}", cmd);

    let child = cmd
        .spawn()
        .map_err(|e| ChalybsError::Qemu(format!("failed to spawn QEMU: {e}")))?;

    let pid = child.id() as i32;

    info!(pid, vm = %vm_name, qmp = %qmp_path, "spawned QEMU process");

    // Move QEMU into vm cpuset if configured.
    if let Some(cg) = &rt.cgroups {
        let procs_path = cg.vm.join("cgroup.procs");
        if procs_path.exists() {
            fs::write(&procs_path, format!("{pid}\n")).map_err(|e| {
                ChalybsError::Qemu(format!(
                    "failed to write QEMU pid to {}: {e}",
                    procs_path.display()
                ))
            })?;
            info!(
                pid,
                path = %procs_path.display(),
                "moved QEMU to vm cpuset"
            );
        } else {
            warn!(
                path = %procs_path.display(),
                "vm cpuset cgroup.procs not found; QEMU not moved into cpuset"
            );
        }
    }

    rt.qemu = Some(QemuState { pid, child });

    Ok(())
}

/// Attempt a graceful QEMU shutdown via SIGTERM and wait.
pub fn shutdown(rt: &mut VmRuntime) -> Result<()> {
    let Some(mut q) = rt.qemu.take() else {
        return Ok(());
    };

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let pid = Pid::from_raw(q.pid);

    info!(pid = q.pid, "sending SIGTERM to QEMU");
    if let Err(e) = kill(pid, Signal::SIGTERM) {
        return Err(ChalybsError::Qemu(format!(
            "failed to send SIGTERM to QEMU {}: {e}",
            q.pid
        )));
    }

    let status = q.child.wait().map_err(|e| {
        ChalybsError::Qemu(format!("failed to wait for QEMU {} to exit: {e}", q.pid))
    })?;

    info!(pid = q.pid, ?status, "QEMU exited");
    Ok(())
}
