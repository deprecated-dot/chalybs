// core/src/vfio/isolation.rs

use std::collections::{HashMap, HashSet};

use tracing::{info, warn};

use crate::config::{IsolationMode, IsolationPolicyConfig, PciDeviceConfig, VmConfig};
use crate::errors::{ChalybsError, Result};
use crate::pci::{GpuSafetyClass, PciFunction, PciInventory};

/// Severity of a single isolation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IsolationSeverity {
    Info,
    Warning,
    Violation,
}

/// A single isolation finding for this VM.
#[derive(Debug, Clone)]
pub struct IsolationFinding {
    pub severity: IsolationSeverity,
    pub code: &'static str,
    pub message: String,
    pub device_bdf: Option<String>,
    pub iommu_group: Option<u32>,
}

/// Aggregate isolation report.
#[derive(Debug, Clone)]
pub struct IsolationReport {
    pub vm_name: String,
    pub findings: Vec<IsolationFinding>,
}

impl IsolationReport {
    pub fn has_violations(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == IsolationSeverity::Violation)
    }
}

/// Phase 8: Evaluate device isolation policy for a VM.
///
/// This is a pure, read-only pass over the current PCI inventory and
/// the VM's configuration. Depending on the per-VM IsolationMode:
///
/// - Disabled → no-op; always Ok.
/// - Audit    → log findings but never block.
/// - Enforce  → treat any Violation as a hard error and abort VFIO
///   staging before touching sysfs.
pub fn evaluate_isolation_for_vm(vm_name: &str, cfg: &VmConfig, inv: &PciInventory) -> Result<()> {
    let policy: &IsolationPolicyConfig = &cfg.isolation;

    match policy.mode {
        IsolationMode::Disabled => {
            info!(
                vm = vm_name,
                "vfio: isolation mode Disabled; skipping Phase 8 checks"
            );
            return Ok(());
        }
        IsolationMode::Audit => {
            info!(
                vm = vm_name,
                "vfio: isolation mode Audit; evaluating Phase 8 checks (non-blocking)"
            );
        }
        IsolationMode::Enforce => {
            info!(
                vm = vm_name,
                "vfio: isolation mode Enforce; evaluating Phase 8 checks (blocking on violations)"
            );
        }
    }

    let mut findings: Vec<IsolationFinding> = Vec::new();

    // Build a set of all passthrough BDFs that resolve in inventory.
    let passthrough_bdfs = collect_passthrough_bdfs(cfg, inv)?;

    // 1) IOMMU-group exclusivity.
    if policy.require_iommu_exclusive {
        evaluate_iommu_exclusivity(vm_name, inv, &passthrough_bdfs, &mut findings);
    }

    // 2) Multi-function consistency (same domain/bus/slot).
    if policy.require_multifunction_consistency {
        evaluate_multifunction_consistency(vm_name, inv, &passthrough_bdfs, &mut findings);
    }

    // 3) Host-critical GPU sharing: IOMMU groups that contain both
    //    passthrough devices and host-owned GPUs (amdgpu/nvidia/nouveau).
    if policy.forbid_host_critical_in_group {
        evaluate_host_critical_sharing(vm_name, inv, &passthrough_bdfs, &mut findings);
    }

    let report = IsolationReport {
        vm_name: vm_name.to_string(),
        findings,
    };

    // Emit logs for all findings, regardless of mode.
    log_report(&report);

    match policy.mode {
        IsolationMode::Disabled | IsolationMode::Audit => Ok(()),
        IsolationMode::Enforce => {
            if report.has_violations() {
                let violation_count = report
                    .findings
                    .iter()
                    .filter(|f| f.severity == IsolationSeverity::Violation)
                    .count();

                Err(ChalybsError::Vfio(format!(
                    "VM {vm_name}: device isolation policy violations detected (count = {violation_count}); \
                     see logs for detailed findings"
                )))
            } else {
                Ok(())
            }
        }
    }
}

/// Collect all configured passthrough devices that resolve in the
/// current inventory into a set of BDF strings.
///
/// Optional devices that are missing are skipped via resolve_configured().
fn collect_passthrough_bdfs(cfg: &VmConfig, inv: &PciInventory) -> Result<HashSet<String>> {
    let mut set: HashSet<String> = HashSet::new();

    collect_devices_for_kind(cfg.devices.gpu.as_ref(), inv, &mut set)?;
    collect_devices_for_kind(cfg.devices.nvme.as_ref(), inv, &mut set)?;
    collect_devices_for_kind(cfg.devices.nic.as_ref(), inv, &mut set)?;
    collect_devices_for_kind(cfg.devices.usb.as_ref(), inv, &mut set)?;

    Ok(set)
}

fn collect_devices_for_kind(
    cfgs_opt: Option<&Vec<PciDeviceConfig>>,
    inv: &PciInventory,
    out: &mut HashSet<String>,
) -> Result<()> {
    let cfgs = match cfgs_opt {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()),
    };

    let funcs = inv.resolve_configured(cfgs)?;
    for f in funcs {
        out.insert(f.bdf.clone());
    }

    Ok(())
}

/// Evaluate IOMMU-group exclusivity: any IOMMU group that contains at
/// least one passthrough device must not contain non-passthrough
/// members if require_iommu_exclusive = true.
fn evaluate_iommu_exclusivity(
    vm_name: &str,
    inv: &PciInventory,
    passthrough_bdfs: &HashSet<String>,
    findings: &mut Vec<IsolationFinding>,
) {
    let groups = inv.by_iommu_group();

    for (gid, members) in groups {
        let mut has_passthrough = false;
        let mut non_passthrough: Vec<String> = Vec::new();
        let mut passthrough_members: Vec<String> = Vec::new();

        for m in members {
            if passthrough_bdfs.contains(&m.bdf) {
                has_passthrough = true;
                passthrough_members.push(m.bdf.clone());
            } else {
                non_passthrough.push(m.bdf.clone());
            }
        }

        if !has_passthrough {
            // No passthrough devices in this group; nothing to check.
            continue;
        }

        if non_passthrough.is_empty() {
            // Group is "cleanly" exclusive to passthrough devices for this VM.
            let message = format!(
                "IOMMU group {gid} contains only passthrough devices ({}); \
                 group is exclusive for VM {vm_name}",
                passthrough_members.join(","),
            );

            findings.push(IsolationFinding {
                severity: IsolationSeverity::Info,
                code: "IOMMU_GROUP_EXCLUSIVE_PASSTHROUGH",
                message,
                device_bdf: passthrough_members.first().cloned(),
                iommu_group: Some(gid),
            });

            continue;
        }

        let message = format!(
            "IOMMU group {gid} contains passthrough devices ({}) and non-passthrough members ({}); \
             this violates require_iommu_exclusive for VM {vm_name}",
            passthrough_members.join(","),
            non_passthrough.join(","),
        );

        findings.push(IsolationFinding {
            severity: IsolationSeverity::Violation,
            code: "IOMMU_GROUP_NOT_EXCLUSIVE",
            message,
            device_bdf: passthrough_members.first().cloned(),
            iommu_group: Some(gid),
        });
    }
}

/// Evaluate multi-function consistency: devices that share the same
/// domain/bus/slot but differ in function are considered a unit. If
/// some functions are passed through and others are not, emit a
/// finding.
fn evaluate_multifunction_consistency(
    vm_name: &str,
    inv: &PciInventory,
    passthrough_bdfs: &HashSet<String>,
    findings: &mut Vec<IsolationFinding>,
) {
    // Build a map of (domain,bus,slot) → all functions discovered.
    let mut by_slot: HashMap<(u16, u8, u8), Vec<&PciFunction>> = HashMap::new();

    for func in &inv.functions {
        if let Some(key) = parse_slot_key(&func.bdf) {
            by_slot.entry(key).or_default().push(func);
        }
    }

    for (key, members) in by_slot {
        let mut passthrough_members: Vec<&PciFunction> = Vec::new();
        let mut host_members: Vec<&PciFunction> = Vec::new();

        for m in members {
            if passthrough_bdfs.contains(&m.bdf) {
                passthrough_members.push(m);
            } else {
                host_members.push(m);
            }
        }

        if passthrough_members.is_empty() || host_members.is_empty() {
            continue;
        }

        let (domain, bus, slot) = key;
        let pt_list: Vec<String> = passthrough_members.iter().map(|f| f.bdf.clone()).collect();
        let host_list: Vec<String> = host_members.iter().map(|f| f.bdf.clone()).collect();

        let message = format!(
            "PCI device {domain:04x}:{bus:02x}:{slot:02x}.x has mixed ownership: \
             passthrough functions ({}) and host-owned functions ({}); \
             this violates require_multifunction_consistency for VM {vm_name}",
            pt_list.join(","),
            host_list.join(","),
        );

        findings.push(IsolationFinding {
            severity: IsolationSeverity::Violation,
            code: "MULTIFUNCTION_MIXED_OWNERSHIP",
            message,
            device_bdf: pt_list.first().cloned(),
            iommu_group: None,
        });
    }
}

/// Evaluate "host-critical" GPU sharing: any IOMMU group that contains
/// host-owned GPU(s) (amdgpu/nvidia/nouveau). If the group also contains
/// passthrough devices, it's a violation when forbid_host_critical_in_group
/// is true; if not, we still emit a warning for a host-only critical group.
fn evaluate_host_critical_sharing(
    vm_name: &str,
    inv: &PciInventory,
    passthrough_bdfs: &HashSet<String>,
    findings: &mut Vec<IsolationFinding>,
) {
    let groups = inv.by_iommu_group();

    for (gid, members) in groups {
        let mut has_passthrough = false;
        let mut host_critical_gpus: Vec<&PciFunction> = Vec::new();

        for m in members {
            if passthrough_bdfs.contains(&m.bdf) {
                has_passthrough = true;
            }

            if m.is_display_controller() {
                if let Some(GpuSafetyClass::HostOwned) = m.gpu_safety_class() {
                    host_critical_gpus.push(m);
                }
            }
        }

        if host_critical_gpus.is_empty() {
            // No host-critical GPU in this group; nothing to do.
            continue;
        }

        let gpu_bdfs: Vec<String> = host_critical_gpus.iter().map(|f| f.bdf.clone()).collect();

        if has_passthrough {
            // This is the original violation case: passthrough + host GPU in same group.
            let message = format!(
                "IOMMU group {gid} contains passthrough devices and host-owned GPU(s) ({}); \
                 this violates forbid_host_critical_in_group for VM {vm_name}",
                gpu_bdfs.join(","),
            );

            findings.push(IsolationFinding {
                severity: IsolationSeverity::Violation,
                code: "HOST_CRITICAL_GPU_SHARED_GROUP",
                message,
                device_bdf: gpu_bdfs.first().cloned(),
                iommu_group: Some(gid),
            });
        } else {
            // Host-only critical group (no passthrough members). Not a violation,
            // but worth warning about as it can still be a sensitive grouping.
            let message = format!(
                "IOMMU group {gid} contains host-owned GPU(s) ({}), but no passthrough devices; \
                 group is host-only but may still be performance/availability sensitive \
                 for VM {vm_name}",
                gpu_bdfs.join(","),
            );

            findings.push(IsolationFinding {
                severity: IsolationSeverity::Warning,
                code: "HOST_CRITICAL_GPU_SHARED_GROUP_HOST_ONLY",
                message,
                device_bdf: gpu_bdfs.first().cloned(),
                iommu_group: Some(gid),
            });
        }
    }
}

/// Parse a BDF string "0000:bb:dd.f" into a (domain,bus,slot) key.
/// Returns None if the BDF is malformed.
fn parse_slot_key(bdf: &str) -> Option<(u16, u8, u8)> {
    // Expected form: "dddd:bb:dd.f"
    let mut parts = bdf.split(':');
    let domain_str = parts.next()?;
    let bus_str = parts.next()?;
    let devfunc_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    let mut devfunc_parts = devfunc_str.split('.');
    let dev_str = devfunc_parts.next()?;
    let func_str = devfunc_parts.next()?;
    if devfunc_parts.next().is_some() {
        return None;
    }

    // Enforce strict widths to avoid accepting malformed BDFs like "00:01:23.1".
    if domain_str.len() != 4 || bus_str.len() != 2 || dev_str.len() != 2 || func_str.is_empty() {
        return None;
    }

    let domain = u16::from_str_radix(domain_str, 16).ok()?;
    let bus = u8::from_str_radix(bus_str, 16).ok()?;
    let dev = u8::from_str_radix(dev_str, 16).ok()?;

    Some((domain, bus, dev))
}

/// Log all findings in a report.
fn log_report(report: &IsolationReport) {
    if report.findings.is_empty() {
        info!(
            vm = report.vm_name.as_str(),
            "vfio: Phase 8 isolation evaluation produced no findings"
        );
        return;
    }

    for f in &report.findings {
        match f.severity {
            IsolationSeverity::Info => {
                info!(
                    vm = report.vm_name.as_str(),
                    code = f.code,
                    bdf = f.device_bdf.as_deref().unwrap_or("<n/a>"),
                    iommu_group = ?f.iommu_group,
                    msg = f.message.as_str(),
                    "vfio: isolation finding (info)"
                );
            }
            IsolationSeverity::Warning => {
                warn!(
                    vm = report.vm_name.as_str(),
                    code = f.code,
                    bdf = f.device_bdf.as_deref().unwrap_or("<n/a>"),
                    iommu_group = ?f.iommu_group,
                    msg = f.message.as_str(),
                    "vfio: isolation finding (warning)"
                );
            }
            IsolationSeverity::Violation => {
                warn!(
                    vm = report.vm_name.as_str(),
                    code = f.code,
                    bdf = f.device_bdf.as_deref().unwrap_or("<n/a>"),
                    iommu_group = ?f.iommu_group,
                    msg = f.message.as_str(),
                    "vfio: isolation finding (VIOLATION)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CpuConfig, DevicesConfig, GpuPolicyConfig, IsolationMode, IsolationPolicyConfig,
        NumaConfig, PciDeviceConfig, QemuConfig,
    };
    use crate::pci::{PciFunction, PciInventory};
    use std::collections::HashSet;

    fn make_gpu(bdf: &str, driver: Option<&str>, iommu_group: Option<u32>) -> PciFunction {
        PciFunction {
            bdf: bdf.to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000, // display controller
            driver: driver.map(|d| d.to_string()),
            iommu_group,
            numa_node: Some(0),
        }
    }

    fn make_generic(
        bdf: &str,
        class: u32,
        driver: Option<&str>,
        iommu_group: Option<u32>,
    ) -> PciFunction {
        PciFunction {
            bdf: bdf.to_string(),
            vendor_id: 0x1111,
            device_id: 0x2222,
            class,
            driver: driver.map(|d| d.to_string()),
            iommu_group,
            numa_node: Some(0),
        }
    }

    fn minimal_vm_with_gpu_and_isolation(bdf: &str, mode: IsolationMode) -> VmConfig {
        // Honor the requested isolation mode while using default
        // policy flags for everything else.
        VmConfig {
            cpu: CpuConfig {
                host_cpus: "0-3".to_string(),
                vm_cpus: "0-1".to_string(),
            },
            qemu: QemuConfig {
                binary: "/usr/bin/qemu-system-x86_64".to_string(),
                args: "".to_string(),
                num_vcpus: 2,
                mem_mb: 2048,
                hugepages: false,
                ovmf_code: "/usr/share/OVMF/OVMF_CODE.fd".to_string(),
                ovmf_vars: "/var/lib/libvirt/qemu/nvram/test_VARS.fd".to_string(),
            },
            numa: Some(NumaConfig { node: None }),
            devices: DevicesConfig {
                gpu: Some(vec![PciDeviceConfig {
                    pci_address: bdf.to_string(),
                    required: true,
                    level: None,
                }]),
                nvme: None,
                nic: None,
                usb: None,
            },
            gpu: GpuPolicyConfig {
                allow_single_gpu: false,
                force_use_igpu: false,
            },
            isolation: IsolationPolicyConfig {
                mode,
                ..IsolationPolicyConfig::default()
            },
            peripherals: None,
        }
    }

    #[test]
    fn parse_slot_key_parses_valid_bdf() {
        let key = parse_slot_key("0000:01:23.4").unwrap();
        assert_eq!(key, (0x0000, 0x01, 0x23));
    }

    #[test]
    fn parse_slot_key_rejects_invalid_bdf() {
        assert!(parse_slot_key("not-a-bdf").is_none());
        assert!(parse_slot_key("0000:01:23").is_none());
        assert!(parse_slot_key("0000:01:23.").is_none());
        assert!(parse_slot_key("00:01:23.1").is_none());
    }

    #[test]
    fn iommu_exclusivity_flags_mixed_group() {
        let pt = make_generic("0000:01:00.0", 0x020000, Some("e1000e"), Some(10));
        let host = make_generic("0000:01:00.1", 0x020000, Some("e1000e"), Some(10));

        let inv = PciInventory {
            functions: vec![pt.clone(), host.clone()],
        };

        let mut findings = Vec::new();
        let mut passthrough_bdfs: HashSet<String> = HashSet::new();
        passthrough_bdfs.insert(pt.bdf.clone());

        evaluate_iommu_exclusivity("testvm", &inv, &passthrough_bdfs, &mut findings);

        assert!(findings
            .iter()
            .any(|f| f.code == "IOMMU_GROUP_NOT_EXCLUSIVE"
                && f.severity == IsolationSeverity::Violation));
    }

    #[test]
    fn iommu_exclusivity_flags_cross_slot_shared_group() {
        // Cross-slot: different slot numbers but same IOMMU group.
        let pt = make_generic("0000:01:00.0", 0x010802, Some("nvme"), Some(42));
        let host = make_generic("0000:02:00.0", 0x020000, Some("e1000e"), Some(42));

        let inv = PciInventory {
            functions: vec![pt.clone(), host.clone()],
        };

        let mut findings = Vec::new();
        let mut passthrough_bdfs: HashSet<String> = HashSet::new();
        passthrough_bdfs.insert(pt.bdf.clone());

        evaluate_iommu_exclusivity("testvm", &inv, &passthrough_bdfs, &mut findings);

        assert!(
            findings
                .iter()
                .any(|f| f.code == "IOMMU_GROUP_NOT_EXCLUSIVE"
                    && f.severity == IsolationSeverity::Violation),
            "expected cross-slot exclusivity violation, got: {findings:?}"
        );
    }

    #[test]
    fn multifunction_consistency_flags_mixed_functions() {
        let pt = make_generic("0000:01:00.0", 0x020000, Some("e1000e"), Some(5));
        let host = make_generic("0000:01:00.1", 0x020000, Some("e1000e"), Some(5));

        let inv = PciInventory {
            functions: vec![pt.clone(), host.clone()],
        };

        let mut findings = Vec::new();
        let mut passthrough_bdfs: HashSet<String> = HashSet::new();
        passthrough_bdfs.insert(pt.bdf.clone());

        evaluate_multifunction_consistency("testvm", &inv, &passthrough_bdfs, &mut findings);

        assert!(findings
            .iter()
            .any(|f| f.code == "MULTIFUNCTION_MIXED_OWNERSHIP"
                && f.severity == IsolationSeverity::Violation));
    }

    #[test]
    fn host_critical_sharing_flags_group_with_host_gpu() {
        // Passthrough NIC and host-owned GPU share an IOMMU group.
        let nic = make_generic("0000:01:00.0", 0x020000, Some("e1000e"), Some(7));
        let host_gpu = make_gpu("0000:01:00.1", Some("amdgpu"), Some(7));

        let inv = PciInventory {
            functions: vec![nic.clone(), host_gpu.clone()],
        };

        let mut findings = Vec::new();
        let mut passthrough_bdfs: HashSet<String> = HashSet::new();
        passthrough_bdfs.insert(nic.bdf.clone());

        evaluate_host_critical_sharing("testvm", &inv, &passthrough_bdfs, &mut findings);

        assert!(findings
            .iter()
            .any(|f| f.code == "HOST_CRITICAL_GPU_SHARED_GROUP"
                && f.severity == IsolationSeverity::Violation));
    }

    #[test]
    fn host_critical_sharing_flags_group_with_passthrough_gpu_and_host_gpu() {
        // Multi-GPU scenario: one GPU is passthrough, the other is host-owned.
        let pt_gpu = make_gpu("0000:03:00.0", Some("vfio-pci"), Some(9));
        let host_gpu = make_gpu("0000:03:00.1", Some("amdgpu"), Some(9));

        let inv = PciInventory {
            functions: vec![pt_gpu.clone(), host_gpu.clone()],
        };

        let mut findings = Vec::new();
        let mut passthrough_bdfs: HashSet<String> = HashSet::new();
        passthrough_bdfs.insert(pt_gpu.bdf.clone());

        evaluate_host_critical_sharing("testvm", &inv, &passthrough_bdfs, &mut findings);

        assert!(
            findings
                .iter()
                .any(|f| f.code == "HOST_CRITICAL_GPU_SHARED_GROUP"
                    && f.severity == IsolationSeverity::Violation),
            "expected host-critical GPU sharing violation, got: {findings:?}"
        );
    }

    #[test]
    fn evaluate_isolation_disabled_is_noop_even_with_issues() {
        let gpu_bdf = "0000:02:00.0";
        let cfg = minimal_vm_with_gpu_and_isolation(gpu_bdf, IsolationMode::Disabled);

        // Inventory with an obvious exclusivity violation.
        let pt_gpu = make_gpu(gpu_bdf, Some("vfio-pci"), Some(3));
        let host_dev = make_generic("0000:02:00.1", 0x020000, Some("e1000e"), Some(3));

        let inv = PciInventory {
            functions: vec![pt_gpu, host_dev],
        };

        // Disabled mode must never error (under current default policy).
        evaluate_isolation_for_vm("testvm", &cfg, &inv).unwrap();
    }

    #[test]
    fn evaluate_isolation_enforce_errors_on_violation() {
        let gpu_bdf = "0000:03:00.0";
        let cfg = minimal_vm_with_gpu_and_isolation(gpu_bdf, IsolationMode::Enforce);

        // Same exclusivity violation as previous test, but in Enforce mode.
        let pt_gpu = make_gpu(gpu_bdf, Some("vfio-pci"), Some(4));
        let host_dev = make_generic("0000:03:00.1", 0x020000, Some("e1000e"), Some(4));

        let inv = PciInventory {
            functions: vec![pt_gpu, host_dev],
        };

        let err = evaluate_isolation_for_vm("testvm", &cfg, &inv).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("device isolation policy violations detected"));
    }
}
