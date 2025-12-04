// core/src/qemu.rs

use std::collections::{HashMap, HashSet};
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

/// Attempt to auto-detect a suitable QEMU CPU model string (-cpu <model>)
/// from the host's /proc/cpuinfo.
///
/// This is deliberately conservative and *opt-in*:
///   - Currently handles x86_64 AMD (AuthenticAMD) family >= 0x17, where
///     using "EPYC-v2" is a safe, well-tested baseline for Zen/Zen+/Zen2-
///     class hardware on your fleet.
///   - For everything else, returns None and the caller must fall back
///     to "host" or whatever the config explicitly requested.
///
/// This keeps existing behavior intact and avoids surprising regressions
/// on non-AMD or very old hardware. Future work can extend this to Intel
/// and newer AMD generations in a table-driven way.
fn autodetect_qemu_cpu_model() -> Option<String> {
    let contents = fs::read_to_string("/proc/cpuinfo").ok()?;

    let mut vendor_id: Option<String> = None;
    let mut family: Option<u32> = None;

    for line in contents.lines() {
        let line = line.trim();

        if line.starts_with("vendor_id") {
            if let Some((_, v)) = line.split_once(':') {
                vendor_id = Some(v.trim().to_string());
            }
        } else if line.starts_with("cpu family") {
            if let Some((_, v)) = line.split_once(':') {
                if let Ok(val) = v.trim().parse::<u32>() {
                    family = Some(val);
                }
            }
        }

        if vendor_id.is_some() && family.is_some() {
            break;
        }
    }

    let vendor = vendor_id?;
    let family = family?;

    if vendor == "AuthenticAMD" && family >= 0x17 {
        // Zen and newer (Naples / Threadripper / Ryzen and up).
        // EPYC-v2 is a known-good baseline on your current fleet.
        info!(
            vendor = %vendor,
            family = family,
            model = "EPYC-v2",
            "qemu: auto-detected AMD host, choosing EPYC-v2 CPU model"
        );
        Some("EPYC-v2".to_string())
    } else {
        info!(
            vendor = %vendor,
            family = family,
            "qemu: CPU autodetect has no mapping for this host; falling back to 'host'"
        );
        None
    }
}

/// Build the QEMU -cpu argument from config:
///
///   1. If `q.cpu_model` is Some:
///        - If it is the literal string "auto" (case-insensitive), attempt a
///          conservative autodetect from /proc/cpuinfo. On failure, fall back
///          to "host".
///        - If it is any other non-empty value, use it as the base model
///          string verbatim.
///   2. Otherwise, if `cpu_extras` is present:
///        - Use `cpu_extras.abi` as the base model if non-empty,
///          otherwise fall back to "host".
///   3. If neither `cpu_model` nor `cpu_extras` is set, fall back to "host".
///
/// In all cases, when `cpu_extras` exists, we append:
///   "<base>,<topo>,<hv_contexts>,<vendor_id>"
/// skipping any empty components.
///
/// This means:
///   - You can set `cpu_model = "auto"` to get autodetection *and* still
///     supply `topo`, `hv_contexts`, and `vendor_id` via `cpu_extras`.
///   - You can pin an explicit model via `cpu_model = "EPYC-v2"` (or similar)
///     and still keep the extra flags.
///   - Legacy configs without `cpu_model` behave as before.
fn build_cpu_arg(rt: &VmRuntime) -> String {
    let q = &rt.cfg.qemu;

    // 1) Resolve the base CPU model string.
    let base = if let Some(raw_model) = q
        .cpu_model
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        if raw_model.eq_ignore_ascii_case("auto") {
            match autodetect_qemu_cpu_model() {
                Some(model) => model,
                None => {
                    info!(
                        requested = %raw_model,
                        "qemu: cpu_model='auto' but autodetect unsupported; falling back to 'host'"
                    );
                    "host".to_string()
                }
            }
        } else {
            raw_model.to_string()
        }
    } else if let Some(extra) = &q.cpu_extras {
        let abi_raw = extra.abi.as_deref().unwrap_or("").trim();
        if !abi_raw.is_empty() {
            abi_raw.to_string()
        } else {
            "host".to_string()
        }
    } else {
        "host".to_string()
    };

    // 2) If there are no extras, just return the base model.
    let Some(extra) = &q.cpu_extras else {
        return base;
    };

    let mut parts = Vec::new();
    parts.push(base);

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

/// Parse and validate a single pci_rootport slot address "0xNN" → u8.
///
/// Valid range is 0x00–0x1f inclusive; we reserve semantics of which
/// slots are actually used for automatic allocation separately.
fn parse_rootport_slot(vm_name: &str, bdf: &str, addr: &str) -> Result<u8> {
    if addr.len() != 4 || !(addr.starts_with("0x") || addr.starts_with("0X")) {
        return Err(ChalybsError::Config(format!(
            "vm {vm_name}: invalid pci_rootport address '{addr}' for device {bdf}; \
expected '0xNN' with NN in 00-1f"
        )));
    }

    let slot_hex = &addr[2..];
    let slot = u8::from_str_radix(slot_hex, 16).map_err(|_| {
        ChalybsError::Config(format!(
            "vm {vm_name}: invalid pci_rootport address '{addr}' for device {bdf}; \
expected '0xNN' with NN in 00-1f"
        ))
    })?;

    if slot > 0x1f {
        return Err(ChalybsError::Config(format!(
            "vm {vm_name}: pci_rootport address '{addr}' for device {bdf} is out of range; \
valid slot range is 0x00-0x1f"
        )));
    }

    Ok(slot)
}

/// Build a deterministic PCI root-port map for all passthrough devices.
///
/// Semantics:
///   - All configured devices under vm.devices.{gpu,nvme,nic,usb} participate.
///   - Base ordering:
///       * GPUs  (priority 0), sorted by BDF
///       * NVMe  (priority 1), sorted by BDF
///       * NIC   (priority 2), sorted by BDF
///       * USB   (priority 3), sorted by BDF
///   - Slots are allocated from 0x01 upward, skipping any slots claimed by
///     explicit overrides in qemu.pci_rootport.
///   - Overrides:
///       * must reference a configured passthrough BDF
///       * must be of the form "0xNN" with NN ∈ [00,1f]
///       * must not assign the same slot to multiple devices
///   - On violation, returns a Config error.
///
/// The resulting map contains an entry for *every* configured passthrough
/// device; apply_rootport_mapping() simply looks up the BDF.
fn build_pci_rootport_map(rt: &VmRuntime) -> Result<HashMap<String, String>> {
    let DevicesConfig {
        gpu,
        nvme,
        nic,
        usb,
    } = &rt.cfg.devices;

    // (priority, bdf)
    let mut entries: Vec<(u8, String)> = Vec::new();

    fn push_kind(out: &mut Vec<(u8, String)>, priority: u8, list: &Option<Vec<PciDeviceConfig>>) {
        if let Some(devs) = list.as_ref() {
            for dev in devs {
                out.push((priority, dev.pci_address.clone()));
            }
        }
    }

    push_kind(&mut entries, 0, gpu);
    push_kind(&mut entries, 1, nvme);
    push_kind(&mut entries, 2, nic);
    push_kind(&mut entries, 3, usb);

    if entries.is_empty() {
        return Ok(HashMap::new());
    }

    // Stable: first by kind priority, then lexicographically by BDF string.
    entries.sort_by(|(k1, b1), (k2, b2)| k1.cmp(k2).then_with(|| b1.cmp(b2)));

    let configured_bdfs: HashSet<&str> = entries.iter().map(|(_, b)| b.as_str()).collect();

    let mut map: HashMap<String, String> = HashMap::new();
    let mut used_slots: HashSet<u8> = HashSet::new();

    // Seed with explicit overrides from config, validating as we go.
    if let Some(overrides) = &rt.cfg.qemu.pci_rootport {
        for (bdf, addr) in overrides {
            if !configured_bdfs.contains(bdf.as_str()) {
                return Err(ChalybsError::Config(format!(
                    "vm {}: pci_rootport override for device {bdf} which is not \
configured under vm.{}.devices",
                    rt.name, rt.name
                )));
            }

            let slot = parse_rootport_slot(&rt.name, bdf, addr)?;
            if !used_slots.insert(slot) {
                return Err(ChalybsError::Config(format!(
                    "vm {}: pci_rootport slot '{addr}' is assigned to more than one device",
                    rt.name
                )));
            }

            // Canonicalize to lowercase 0xNN.
            map.insert(bdf.clone(), format!("0x{:02x}", slot));
        }
    }

    // Auto-assign remaining devices from 0x01 upward, skipping used slots.
    let mut next_slot: u8 = 0x01;

    for (_, bdf) in entries {
        if map.contains_key(&bdf) {
            continue;
        }

        while used_slots.contains(&next_slot) {
            next_slot = next_slot.saturating_add(1);
        }

        if next_slot > 0x1f {
            return Err(ChalybsError::Config(format!(
                "vm {}: more passthrough devices than available PCIe root-port slots (0x01-0x1f)",
                rt.name
            )));
        }

        used_slots.insert(next_slot);
        map.insert(bdf, format!("0x{:02x}", next_slot));
        next_slot = next_slot.saturating_add(1);
    }

    Ok(map)
}

/// Apply a precomputed root-port mapping for a given BDF, if present.
///
/// This simply appends:
///   ,bus=pcie.0,addr=0xNN
///
/// to the device parameter string, using the already-validated address from
/// build_pci_rootport_map().
fn apply_rootport_mapping(params: &mut String, bdf: &str, pci_rootport: &HashMap<String, String>) {
    if let Some(addr) = pci_rootport.get(bdf) {
        params.push_str(",bus=pcie.0");
        params.push_str(",addr=");
        params.push_str(addr);
    }
}

/// Wire VFIO PCIe devices from VmConfig into the QEMU command line.
///
/// This is the missing "attach devices to the VM" phase:
///   - For each GPU, NVMe, NIC, and USB device configured in vm.cfg.devices,
///     emit a corresponding `-device vfio-pci,host=...`.
///   - For legacy primary GPU configs, when explicitly requested via
///     `qemu.legacy_primary_gpu = true`, the first GPU entry is marked with
///     `multifunction=on,x-vga=on` (for pre-UEFI/GOP-era hardware).
///   - For any device present in `qemu.rombar_off`, add `rombar=0`.
///   - All passthrough devices are assigned deterministic `bus=pcie.0,addr=`
///     values via the internal root-port allocator, with any explicit
///     `qemu.pci_rootport` overrides applied first.
fn add_vfio_pci_devices(cmd: &mut Command, rt: &VmRuntime) -> Result<()> {
    let DevicesConfig {
        gpu,
        nvme,
        nic,
        usb,
    } = &rt.cfg.devices;

    // Optional list of PCI devices whose option ROM BAR should be disabled.
    // Entries must be full PCI BDFs, e.g. "0000:49:00.0".
    let rombar_list: &[String] = rt.cfg.qemu.rombar_off.as_deref().unwrap_or(&[]);

    // Deterministic PCI root-port mapping (including overrides, if any).
    let pci_rootport = build_pci_rootport_map(rt)?;

    // Whether the first GPU should be treated as a legacy VGA device, i.e.
    // we explicitly add `multifunction=on,x-vga=on` for pre-UEFI/GOP GPUs.
    let use_legacy_primary_gpu = rt.cfg.qemu.legacy_primary_gpu;

    // GPUs: first entry optionally treated as primary legacy video function.
    if let Some(gpus) = gpu.as_ref() {
        for (idx, dev) in gpus.iter().enumerate() {
            let mut params = format!("host={}", dev.pci_address);

            if idx == 0 && use_legacy_primary_gpu && gpus.len() > 1 {
                // Legacy behavior for pre-UEFI/GOP GPUs: mark the primary
                // function as VGA and multifunction when explicitly requested.
                params.push_str(",multifunction=on,x-vga=on");
            }

            if rombar_list.iter().any(|bdf| bdf == &dev.pci_address) {
                params.push_str(",rombar=0");
            }

            apply_rootport_mapping(&mut params, &dev.pci_address, &pci_rootport);

            cmd.arg("-device").arg(format!("vfio-pci,{params}"));

            info!(
                pci = %dev.pci_address,
                "qemu: attached GPU passthrough device via vfio-pci"
            );
        }
    }

    // Helper for non-GPU kinds: plain vfio-pci attachment, with optional
    // rombar=0 and deterministic pci_rootport mapping.
    fn add_generic_list(
        cmd: &mut Command,
        list: &Option<Vec<PciDeviceConfig>>,
        kind: &str,
        rombar_list: &[String],
        pci_rootport: &HashMap<String, String>,
    ) -> Result<()> {
        if let Some(devs) = list.as_ref() {
            for dev in devs {
                let mut params = format!("host={}", dev.pci_address);

                if rombar_list.iter().any(|bdf| bdf == &dev.pci_address) {
                    params.push_str(",rombar=0");
                }

                apply_rootport_mapping(&mut params, &dev.pci_address, pci_rootport);

                cmd.arg("-device").arg(format!("vfio-pci,{params}"));

                info!(
                    pci = %dev.pci_address,
                    kind,
                    "qemu: attached {kind} passthrough device via vfio-pci"
                );
            }
        }

        Ok(())
    }

    add_generic_list(cmd, nvme, "NVMe", rombar_list, &pci_rootport)?;
    add_generic_list(cmd, nic, "NIC", rombar_list, &pci_rootport)?;
    add_generic_list(cmd, usb, "USB", rombar_list, &pci_rootport)?;

    Ok(())
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
        .arg(q.mem_mb.to_string());

    // If hugepages are active for this VM, direct QEMU to allocate
    // RAM from the hugetlbfs mount that Phase 12 provisioned.
    // This is the only behavior change here: we do not alter mem_mb,
    // topology, or device wiring.
    if rt.hugepages_active {
        info!(
            vm = %rt.name,
            node = ?rt.hugepages_node,
            pages = rt.hugepages_pages,
            bytes = rt.hugepages_bytes,
            "qemu: using hugepages-backed RAM via hugetlbfs"
        );
        cmd.arg("-mem-prealloc")
            .arg("-mem-path")
            .arg("/dev/hugepages");
    }

    cmd.arg("-machine")
        .arg("q35,accel=kvm")
        .arg("-drive")
        .arg(format!(
            "if=pflash,format=raw,readonly=on,file={}",
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
    add_vfio_pci_devices(&mut cmd, rt)?;

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
