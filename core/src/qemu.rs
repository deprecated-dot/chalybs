// core/src/qemu.rs

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};

use std::ffi::CString;
use std::os::unix::fs::OpenOptionsExt;

use libc;

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

/// Declarative description of a QEMU command.
///
/// This is deliberately "dumb data": the builder fills it, and the
/// launch() path is responsible for translating it into a std::process::Command.
#[derive(Debug)]
struct QemuCommand {
    binary: String,
    args: Vec<String>,
    qmp_path: String,
}

/// Builder for QemuCommand.
///
/// This folds all existing QEMU launch semantics into a structured
/// command model without altering behavior.
struct QemuCommandBuilder<'a> {
    rt: &'a VmRuntime,
    binary: String,
    args: Vec<String>,
    qmp_path: String,
}

impl<'a> QemuCommandBuilder<'a> {
    fn new(rt: &'a VmRuntime) -> Self {
        let q = &rt.cfg.qemu;
        let binary = q.binary.clone();
        let qmp_path = qmp_path_for_vm(&rt.name);

        Self {
            rt,
            binary,
            args: Vec::new(),
            qmp_path,
        }
    }

    /// 1) Pre-arguments from config (inserted before core args).
    fn apply_pre_args(&mut self) {
        let q = &self.rt.cfg.qemu;

        if let Some(pre) = q.pre_args.as_ref() {
            for tok in pre.split_whitespace() {
                if !tok.is_empty() {
                    self.args.push(tok.to_string());
                }
            }
        }
    }

    /// 2) Core Chalybs-managed arguments (-enable-kvm, -cpu, -smp, -m, hugepages).
    fn apply_core_args(&mut self) {
        let q = &self.rt.cfg.qemu;

        let cpu_arg = build_cpu_arg(self.rt);

        self.args.push("-enable-kvm".to_string());
        self.args.push("-cpu".to_string());
        self.args.push(cpu_arg);
        self.args.push("-smp".to_string());
        self.args.push(q.num_vcpus.to_string());
        self.args.push("-m".to_string());
        self.args.push(q.mem_mb.to_string());

        // If hugepages are active for this VM, direct QEMU to allocate
        // RAM from the hugetlbfs mount that Phase 12 provisioned.
        if self.rt.hugepages_active {
            info!(
                vm = %self.rt.name,
                node = ?self.rt.hugepages_node,
                pages = self.rt.hugepages_pages,
                bytes = self.rt.hugepages_bytes,
                "qemu: using hugepages-backed RAM via hugetlbfs"
            );
            self.args.push("-mem-prealloc".to_string());
            self.args.push("-mem-path".to_string());
            self.args.push("/dev/hugepages".to_string());
        }
    }

    /// 3) Machine type, firmware drives, and QMP socket.
    fn apply_machine_and_firmware(&mut self) {
        let q = &self.rt.cfg.qemu;

        self.args.push("-machine".to_string());
        self.args.push("q35,accel=kvm".to_string());

        self.args.push("-drive".to_string());
        self.args.push(format!(
            "if=pflash,format=raw,readonly=on,file={}",
            q.ovmf_code
        ));

        self.args.push("-drive".to_string());
        self.args
            .push(format!("if=pflash,format=raw,file={}", q.ovmf_vars));

        self.args.push("-qmp".to_string());
        self.args
            .push(format!("unix:{},server,nowait", self.qmp_path));
    }

    /// 4) RTC configuration.
    fn apply_rtc(&mut self) {
        apply_rtc_args(&mut self.args, self.rt);
    }

    /// 5) SMBIOS configuration.
    fn apply_smbios(&mut self) {
        apply_smbios_args(&mut self.args, self.rt);
    }

    /// 6) VFIO PCI devices.
    fn apply_vfio_pci_devices(&mut self) -> Result<()> {
        add_vfio_pci_devices(&mut self.args, self.rt)
    }

    /// 7) Looking Glass ivshmem wiring (if configured).
    ///
    /// This mirrors the legacy Bash semantics:
    ///
    ///   -object memory-backend-file,id=ivshmem,share=on,mem-path=$LG_PATH,size=${LG_MEM}M
    ///   -device ivshmem-plain,memdev=ivshmem,bus=pcie.0
    ///
    /// but expressed in declarative form and driven from:
    ///   [vm.<name>.peripherals.looking_glass]
    ///   shm_name = "/dev/shm/looking-glass"
    ///   mem_mb   = 128
    fn apply_looking_glass(&mut self) {
        let lg = match self
            .rt
            .cfg
            .peripherals
            .as_ref()
            .and_then(|p| p.looking_glass.as_ref())
        {
            Some(cfg) => cfg,
            None => return,
        };

        if lg.mem_mb == 0 {
            warn!(
                vm = %self.rt.name,
                shm = %lg.shm_name,
                "qemu: Looking Glass configured with mem_mb=0; skipping ivshmem wiring"
            );
            return;
        }

        let shm_path = &lg.shm_name;
        let mem_mb = lg.mem_mb;

        // memory-backend-file first, then the ivshmem-plain device that
        // references it; QEMU is tolerant of ordering, but this is the
        // least surprising and most explicit form.
        self.args.push("-object".to_string());
        self.args.push(format!(
            "memory-backend-file,id=ivshmem,share=on,mem-path={},size={}M",
            shm_path, mem_mb
        ));

        self.args.push("-device".to_string());
        self.args
            .push("ivshmem-plain,memdev=ivshmem,bus=pcie.0".to_string());

        info!(
            vm = %self.rt.name,
            shm = %shm_path,
            mem_mb,
            "qemu: wired Looking Glass ivshmem backend and device"
        );
    }

    /// 8) SPICE wiring (if configured and enabled).
    ///
    /// This mirrors your legacy Bash suite semantics:
    ///
    ///   -device virtio-serial-pci,id=virtio-serial0,max_ports=16,bus=pcie.0,addr=0x10
    ///   -chardev spicevmc,name=vdagent,id=vdagent
    ///   -device virtserialport,nr=1,bus=virtio-serial0.0,chardev=vdagent,name=com.redhat.spice.0
    ///   -spice port=<port>,addr=<addr>,disable-ticketing=on
    fn apply_spice(&mut self) -> Result<()> {
        let spice = match self
            .rt
            .cfg
            .peripherals
            .as_ref()
            .and_then(|p| p.spice.as_ref())
        {
            Some(cfg) => cfg,
            None => return Ok(()),
        };

        if !spice.enabled {
            info!(
                vm = %self.rt.name,
                "qemu: SPICE peripheral present but disabled; skipping wiring"
            );
            return Ok(());
        }

        if spice.port == 0 {
            return Err(ChalybsError::Qemu(format!(
                "vm {}: peripherals.spice.enabled = true but port=0; \
                 please set a valid SPICE TCP port",
                self.rt.name
            )));
        }

        let addr = spice.addr.trim();
        if addr.is_empty() {
            return Err(ChalybsError::Qemu(format!(
                "vm {}: peripherals.spice.enabled = true but addr is empty; \
                 please set peripherals.spice.addr",
                self.rt.name
            )));
        }

        // Virtio-serial bus for vdagent, fixed addr=0x10 on pcie.0 (matches Bash).
        self.args.push("-device".to_string());
        self.args.push(
            "virtio-serial-pci,id=virtio-serial0,max_ports=16,bus=pcie.0,addr=0x10".to_string(),
        );

        // vdagent channel via spicevmc.
        self.args.push("-chardev".to_string());
        self.args
            .push("spicevmc,name=vdagent,id=vdagent".to_string());

        self.args.push("-device".to_string());
        self.args.push(
            "virtserialport,nr=1,bus=virtio-serial0.0,chardev=vdagent,name=com.redhat.spice.0"
                .to_string(),
        );

        // SPICE server itself.
        self.args.push("-spice".to_string());
        self.args.push(format!(
            "port={port},addr={addr},disable-ticketing=on",
            port = spice.port,
            addr = addr,
        ));

        info!(
            vm = %self.rt.name,
            port = spice.port,
            addr = addr,
            "qemu: wired SPICE server and vdagent virtio-serial channel"
        );

        Ok(())
    }

    /// 9) Mid-section extra args (`q.args`).
    fn apply_mid_args(&mut self) {
        let q = &self.rt.cfg.qemu;

        if !q.args.trim().is_empty() {
            for tok in q.args.split_whitespace() {
                if !tok.is_empty() {
                    self.args.push(tok.to_string());
                }
            }
        }
    }

    /// 10) Post-arguments (`q.post_args`).
    fn apply_post_args(&mut self) {
        let q = &self.rt.cfg.qemu;

        if let Some(post) = q.post_args.as_ref() {
            for tok in post.split_whitespace() {
                if !tok.is_empty() {
                    self.args.push(tok.to_string());
                }
            }
        }
    }

    fn build(self) -> QemuCommand {
        QemuCommand {
            binary: self.binary,
            args: self.args,
            qmp_path: self.qmp_path,
        }
    }
}

/// Build the QEMU -cpu argument from config:
///
///   1. If `q.cpu_model` is Some:
///        - If it is the literal string "auto" (case-insensitive), invoke the
///          host CPU detection subsystem (`crate::cpu::detect`). If detection
///          returns a supported model string, use it. Otherwise fall back to
///          "host".
///        - If it is any other non-empty value, use it verbatim as the base
///          model string.
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
///   - You can set `cpu_model = "auto"` to use the dedicated CPU detection
///     subsystem *and* still provide `topo`, `hv_contexts`, and `vendor_id`
///     via `cpu_extras`.
///   - You can specify an explicit model such as `cpu_model = "EPYC-v2"`
///     and still have extras applied.
///   - Legacy configurations without `cpu_model` maintain their previous
///     semantics.
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
            match crate::cpu::detect::autodetect_qemu_cpu_model() {
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

/// Inject SMBIOS configuration into the QEMU argument list (if configured).
fn apply_smbios_args(args: &mut Vec<String>, rt: &VmRuntime) {
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
        args.push("-smbios".to_string());
        args.push(format!("type=0,{}", t0_parts.join(",")));
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
        args.push("-smbios".to_string());
        args.push(format!("type=1,{}", t1_parts.join(",")));
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
        args.push("-smbios".to_string());
        args.push(format!("type=2,{}", t2_parts.join(",")));
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
fn add_vfio_pci_devices(args: &mut Vec<String>, rt: &VmRuntime) -> Result<()> {
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
                // Legacy behavior for pre-UEFI/GOP GPUs:
                params.push_str(",multifunction=on,x-vga=on");
            }

            if rombar_list.iter().any(|bdf| bdf == &dev.pci_address) {
                params.push_str(",rombar=0");
            }

            apply_rootport_mapping(&mut params, &dev.pci_address, &pci_rootport);

            args.push("-device".to_string());
            args.push(format!("vfio-pci,{params}"));

            info!(
                pci = %dev.pci_address,
                "qemu: attached GPU passthrough device via vfio-pci"
            );
        }
    }

    // Helper for non-GPU kinds: plain vfio-pci attachment, with optional
    // rombar=0 and deterministic pci_rootport mapping.
    fn add_generic_list(
        args: &mut Vec<String>,
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

                args.push("-device".to_string());
                args.push(format!("vfio-pci,{params}"));

                info!(
                    pci = %dev.pci_address,
                    kind,
                    "qemu: attached {kind} passthrough device via vfio-pci"
                );
            }
        }

        Ok(())
    }

    add_generic_list(args, nvme, "NVMe", rombar_list, &pci_rootport)?;
    add_generic_list(args, nic, "NIC", rombar_list, &pci_rootport)?;
    add_generic_list(args, usb, "USB", rombar_list, &pci_rootport)?;

    Ok(())
}

/// Inject RTC arguments based on QemuConfig.
///
/// Behavior:
///   - If q.rtc = Some(non-empty), emit exactly `-rtc <value>`
///   - If q.rtc = Some("") (empty/whitespace), emit nothing (QEMU default)
///   - If q.rtc = None, emit the legacy Bash default:
///       `-rtc base=localtime,driftfix=slew`
fn apply_rtc_args(args: &mut Vec<String>, rt: &VmRuntime) {
    let q = &rt.cfg.qemu;

    match q.rtc.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(val) => {
            args.push("-rtc".to_string());
            args.push(val.to_string());
            info!(rtc = %val, "qemu: using explicit RTC policy from config");
        }
        None => {
            // Default: mirror Bash suite behavior for Windows guests.
            let default_rtc = "base=localtime,driftfix=slew";
            args.push("-rtc".to_string());
            args.push(default_rtc.to_string());
            info!(
                rtc = %default_rtc,
                "qemu: using default RTC policy (localtime + driftfix)"
            );
        }
    }
}

/// Detect the desktop user in a deterministic, minimal-heuristic way.
///
/// Priority:
///   1. SUDO_USER
///   2. DOAS_USER
///   3. USER
///   4. LOGNAME
///
/// "root" is ignored, since the whole point is to discover the
/// non-root desktop session user that invoked Chalybs via sudo/doas.
fn detect_desktop_username() -> Option<String> {
    const CANDIDATES: &[&str] = &["SUDO_USER", "DOAS_USER", "USER", "LOGNAME"];

    for key in CANDIDATES {
        if let Ok(val) = env::var(key) {
            let v = val.trim();
            if !v.is_empty() && v != "root" {
                return Some(v.to_string());
            }
        }
    }

    None
}

/// Minimal /etc/passwd lookup for a single user.
///
/// Returns (uid, primary_gid) on success.
fn lookup_user(username: &str) -> Option<(u32, u32)> {
    let data = fs::read_to_string("/etc/passwd").ok()?;

    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 {
            continue;
        }

        if parts[0] == username {
            let uid = parts[2].parse::<u32>().ok()?;
            let gid = parts[3].parse::<u32>().ok()?;
            return Some((uid, gid));
        }
    }

    None
}

/// Minimal /etc/group lookup for a single group name.
///
/// Returns gid on success.
fn lookup_group(group_name: &str) -> Option<u32> {
    let data = fs::read_to_string("/etc/group").ok()?;

    for line in data.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 3 {
            continue;
        }

        if parts[0] == group_name {
            let gid = parts[2].parse::<u32>().ok()?;
            return Some(gid);
        }
    }

    None
}

/// Thin wrapper over libc::chown for a single path.
fn chown_path(path: &str, uid: u32, gid: u32) -> std::io::Result<()> {
    let cstr = CString::new(path.as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path for chown contains interior NUL",
        )
    })?;

    let rc = unsafe { libc::chown(cstr.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Deterministic preparation of the Looking Glass shared-memory backing file.
///
/// Semantics:
///   - If no looking_glass peripheral is configured → no-op.
///   - If mem_mb == 0 → log + no-op (mirrors ivshmem wiring behavior).
///   - Otherwise:
///       * Create/truncate the shm file at the configured path.
///       * Size it to mem_mb MiB.
///       * Set mode 0660.
///       * Resolve a desktop username (env-based, deterministic order).
///       * Resolve uid/gid from /etc/passwd and /etc/group (kvm).
///       * chown(path, uid, gid_for_kvm_or_primary_group).
///
/// Any failure to *create or size* the file is treated as a hard QEMU
/// error. Failures to detect the user or to chown are logged as warnings
/// but do not block VM startup.
fn prepare_looking_glass_shm(rt: &VmRuntime) -> Result<()> {
    let lg = match rt
        .cfg
        .peripherals
        .as_ref()
        .and_then(|p| p.looking_glass.as_ref())
    {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    if lg.mem_mb == 0 {
        info!(
            vm = %rt.name,
            shm = %lg.shm_name,
            "looking-glass: mem_mb=0; skipping shm preparation"
        );
        return Ok(());
    }

    let shm_path = lg.shm_name.as_str();
    let mem_bytes = lg.mem_mb * 1024 * 1024;

    // Parent directory creation is effectively a no-op for /dev/shm,
    // but this keeps behavior well-defined if a different path is used.
    if let Some(parent) = Path::new(shm_path).parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                ChalybsError::Qemu(format!(
                    "looking-glass: failed to create parent directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }

    // Create/truncate the shm file and size it deterministically.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o660)
        .open(shm_path)
        .map_err(|e| {
            ChalybsError::Qemu(format!(
                "looking-glass: failed to create shm file {}: {e}",
                shm_path
            ))
        })?;

    file.set_len(mem_bytes).map_err(|e| {
        ChalybsError::Qemu(format!(
            "looking-glass: failed to size shm file {} to {} bytes: {e}",
            shm_path, mem_bytes
        ))
    })?;

    // Resolve desktop user.
    let username = match detect_desktop_username() {
        Some(u) => u,
        None => {
            warn!(
                vm = %rt.name,
                shm = %shm_path,
                "looking-glass: could not determine desktop user from environment; \
                 leaving shm ownership as-is"
            );
            return Ok(());
        }
    };

    let (uid, primary_gid) = match lookup_user(&username) {
        Some(pair) => pair,
        None => {
            warn!(
                vm = %rt.name,
                shm = %shm_path,
                user = %username,
                "looking-glass: user not found in /etc/passwd; leaving shm ownership as-is"
            );
            return Ok(());
        }
    };

    // Prefer the kvm group if present; otherwise fall back to the
    // user's primary group. This mirrors your Bash semantics without
    // introducing additional heuristics.
    let gid = lookup_group("kvm").unwrap_or(primary_gid);

    match chown_path(shm_path, uid, gid) {
        Ok(()) => {
            info!(
                vm = %rt.name,
                shm = %shm_path,
                user = %username,
                uid,
                gid,
                "looking-glass: prepared shm file with deterministic ownership"
            );
        }
        Err(e) => {
            warn!(
                vm = %rt.name,
                shm = %shm_path,
                user = %username,
                uid,
                gid,
                "looking-glass: failed to chown shm file; leaving ownership as-is: {e}"
            );
        }
    }

    Ok(())
}

/// Best-effort cleanup of the Looking Glass shared-memory backing file.
///
/// Semantics:
///   - If no looking_glass peripheral is configured → no-op.
///   - If mem_mb == 0 → no-op (mirrors preparation semantics).
///   - Otherwise, attempt to remove the shm file:
///       * Success      → info log.
///       * NotFound     → info log, nothing to do.
///       * Other errors → warn, but do not fail shutdown.
fn cleanup_looking_glass_shm(rt: &VmRuntime) {
    let lg = match rt
        .cfg
        .peripherals
        .as_ref()
        .and_then(|p| p.looking_glass.as_ref())
    {
        Some(cfg) => cfg,
        None => return,
    };

    if lg.mem_mb == 0 {
        info!(
            vm = %rt.name,
            shm = %lg.shm_name,
            "looking-glass: mem_mb=0; skipping shm cleanup"
        );
        return;
    }

    let shm_path = lg.shm_name.as_str();

    match fs::remove_file(shm_path) {
        Ok(()) => {
            info!(
                vm = %rt.name,
                shm = %shm_path,
                "looking-glass: removed shm file at VM teardown"
            );
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(
                vm = %rt.name,
                shm = %shm_path,
                "looking-glass: shm file already absent at teardown; nothing to do"
            );
        }
        Err(e) => {
            warn!(
                vm = %rt.name,
                shm = %shm_path,
                "looking-glass: failed to remove shm file at teardown: {e}"
            );
        }
    }
}

/// Launch QEMU and move it into the vm cpuset.
pub fn launch(rt: &mut VmRuntime) -> Result<()> {
    let vm_name = rt.name.clone();

    // Ensure /run/chalybs exists for QMP sockets, etc.
    fs::create_dir_all("/run/chalybs")
        .map_err(|e| ChalybsError::Qemu(format!("failed to create /run/chalybs: {e}")))?;

    // Deterministically prepare the Looking Glass shm backing file *before*
    // wiring it into QEMU. This mirrors the legacy Bash semantics with
    // NUMA/hugepage awareness handled by the rest of Chalybs.
    prepare_looking_glass_shm(&rt)?;

    // Declarative command construction via builder.
    let mut builder = QemuCommandBuilder::new(rt);
    builder.apply_pre_args();
    builder.apply_core_args();
    builder.apply_machine_and_firmware();
    builder.apply_rtc();
    builder.apply_smbios();
    builder.apply_vfio_pci_devices()?;
    builder.apply_looking_glass();
    builder.apply_spice()?;
    builder.apply_mid_args();
    builder.apply_post_args();

    let cmd_desc = builder.build();

    debug!(
        binary = %cmd_desc.binary,
        args = ?cmd_desc.args,
        "launching QEMU"
    );

    let mut cmd = Command::new(&cmd_desc.binary);
    for arg in &cmd_desc.args {
        cmd.arg(arg);
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let child = cmd
        .spawn()
        .map_err(|e| ChalybsError::Qemu(format!("failed to spawn QEMU: {e}")))?;

    let pid = child.id() as i32;

    info!(
        pid,
        vm = %vm_name,
        qmp = %cmd_desc.qmp_path,
        "spawned QEMU process"
    );

    // Move QEMU into vm cpuset.
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

    cleanup_looking_glass_shm(rt);

    Ok(())
}
