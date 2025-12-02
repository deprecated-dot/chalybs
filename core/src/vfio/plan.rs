use std::collections::HashMap;

use tracing::debug;

use crate::config::{PciDeviceConfig, VmConfig};
use crate::errors::{ChalybsError, Result};
use crate::pci::{GpuUnbindAssessment, GpuUnbindFeasibility, PciFunction, PciInventory};

/// Kind of VFIO action to perform on a given PCI function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfioActionKind {
    /// Unbind the device from its current kernel driver, if any.
    UnbindFromCurrentDriver,
    /// Bind the device to vfio-pci.
    BindToVfio,
}

/// A single VFIO-related action on a PCI function.
#[derive(Debug, Clone)]
pub struct VfioAction {
    /// PCI BDF, e.g. "0000:0b:00.0".
    pub bdf: String,
    /// What to do with this device.
    pub kind: VfioActionKind,
    /// Human-readable explanation for logs and debugging.
    pub reason: String,
}

/// Plan of VFIO actions required to stage a VM's devices for passthrough.
#[derive(Debug, Clone)]
pub struct VfioPlan {
    pub vm_name: String,
    pub actions: Vec<VfioAction>,
}

/// Build a VFIO action plan for the given VM using the provided PCI inventory.
///
/// This is a pure function: it does not touch sysfs. All side effects are
/// performed later by `execute_plan`.
pub fn build_plan_for_vm(vm_name: &str, cfg: &VmConfig, inv: &PciInventory) -> Result<VfioPlan> {
    let mut unbind_actions: Vec<VfioAction> = Vec::new();
    let mut bind_actions: Vec<VfioAction> = Vec::new();

    // 1) GPU handling: respect GPU unbind feasibility classification.
    build_gpu_actions(vm_name, cfg, inv, &mut unbind_actions, &mut bind_actions)?;

    // 2) Non-GPU PCI devices.
    build_non_gpu_actions(vm_name, cfg, inv, &mut unbind_actions, &mut bind_actions)?;

    // Order: all unbinds first, then binds.
    let mut actions = Vec::new();
    actions.extend(unbind_actions);
    actions.extend(bind_actions);

    debug!(
        vm = vm_name,
        action_count = actions.len(),
        "vfio: built VFIO action plan"
    );

    Ok(VfioPlan {
        vm_name: vm_name.to_string(),
        actions,
    })
}

/// Build actions for configured GPUs, using GPU unbind feasibility.
///
/// Policy:
/// - Safe   → allowed.
/// - Risky  → hard-error.
/// - Unsafe → hard-error.
///
/// Under Option B GPU-complex semantics:
/// - Functions that are *display controllers* (class base 0x03) follow
///   the existing GPU unbind safety logic.
/// - Functions listed under devices.gpu that are *not* display
///   controllers are accepted if they share a slot with a display
///   controller (GPU complex members, e.g. audio). These are staged
///   like generic PCI devices and do not require a GPU unbind
///   assessment of their own.
fn build_gpu_actions(
    vm_name: &str,
    cfg: &VmConfig,
    inv: &PciInventory,
    unbind_actions: &mut Vec<VfioAction>,
    bind_actions: &mut Vec<VfioAction>,
) -> Result<()> {
    let gpu_cfgs: &[PciDeviceConfig] = match cfg.devices.gpu.as_ref() {
        Some(list) => list.as_slice(),
        None => return Ok(()),
    };

    if gpu_cfgs.is_empty() {
        return Ok(());
    }

    let gpu_funcs = inv.resolve_configured(gpu_cfgs)?;

    // Build BDF → assessment map for GPU *display* functions.
    let assessments = inv.assess_gpu_unbind_safety();
    let mut by_bdf: HashMap<&str, &GpuUnbindAssessment> = HashMap::new();
    for a in &assessments {
        by_bdf.insert(a.bdf.as_str(), a);
    }

    for func in gpu_funcs {
        let bdf = func.bdf.as_str();

        if func.is_display_controller() {
            // Original semantics for true GPU functions.
            let assessment = match by_bdf.get(bdf) {
                Some(a) => *a,
                None => {
                    return Err(ChalybsError::Vfio(format!(
                        "VM {vm_name}: no GPU unbind assessment available for {bdf}"
                    )));
                }
            };

            match &assessment.feasibility {
                GpuUnbindFeasibility::Safe => {
                    append_actions_for_function(vm_name, "GPU", func, unbind_actions, bind_actions);
                }
                GpuUnbindFeasibility::Risky(msg) => {
                    return Err(ChalybsError::Vfio(format!(
                        "VM {vm_name}: GPU {bdf} is classified as RISKY to unbind: {msg}"
                    )));
                }
                GpuUnbindFeasibility::Unsafe(msg) => {
                    return Err(ChalybsError::Vfio(format!(
                        "VM {vm_name}: GPU {bdf} is classified as UNSAFE to unbind: {msg}"
                    )));
                }
            }
        } else {
            // Non-display function listed under devices.gpu — e.g. GPU audio.
            //
            // Option B + A2/B1 semantics:
            // - We *allow* these entries as long as they are members of a
            //   GPU complex (same slot as a display controller).
            // - We treat them like generic PCI devices with respect to
            //   unbind/bind; we do not require a separate GPU unbind
            //   safety assessment keyed on their BDF.
            if inv.is_bdf_in_gpu_complex(bdf) {
                append_actions_for_function(vm_name, "GPU", func, unbind_actions, bind_actions);
            } else {
                return Err(ChalybsError::Vfio(format!(
                    "VM {vm_name}: device {bdf} is listed under devices.gpu but PCI class base is \
                     not 0x03 (display controller) and it does not share a PCI slot with a display controller"
                )));
            }
        }
    }

    Ok(())
}

/// Build actions for non-GPU PCI devices (NVMe, NIC, USB).
fn build_non_gpu_actions(
    vm_name: &str,
    cfg: &VmConfig,
    inv: &PciInventory,
    unbind_actions: &mut Vec<VfioAction>,
    bind_actions: &mut Vec<VfioAction>,
) -> Result<()> {
    stage_device_list(
        vm_name,
        "NVMe",
        cfg.devices.nvme.as_ref(),
        inv,
        unbind_actions,
        bind_actions,
    )?;

    stage_device_list(
        vm_name,
        "NIC",
        cfg.devices.nic.as_ref(),
        inv,
        unbind_actions,
        bind_actions,
    )?;

    stage_device_list(
        vm_name,
        "USB",
        cfg.devices.usb.as_ref(),
        inv,
        unbind_actions,
        bind_actions,
    )?;

    Ok(())
}

/// Append unbind/bind actions for a single PCI function.
fn append_actions_for_function(
    vm_name: &str,
    kind_label: &str,
    func: &PciFunction,
    unbind_actions: &mut Vec<VfioAction>,
    bind_actions: &mut Vec<VfioAction>,
) {
    let bdf = func.bdf.clone();
    let driver = func.driver.as_deref();

    // If device has a current driver and it's not vfio-pci → unbind.
    if let Some(d) = driver {
        if d != "vfio-pci" {
            unbind_actions.push(VfioAction {
                bdf: bdf.clone(),
                kind: VfioActionKind::UnbindFromCurrentDriver,
                reason: format!(
                    "{kind_label} {bdf}: unbinding from current driver `{d}` \
                     before binding to vfio-pci for VM {vm_name}"
                ),
            });
        }
    }

    // Always enqueue a bind-to-vfio step (idempotent).
    bind_actions.push(VfioAction {
        bdf,
        kind: VfioActionKind::BindToVfio,
        reason: format!("{kind_label} device bound to vfio-pci for VM {vm_name}"),
    });
}

/// Stage all configured devices of a kind (NVMe, NIC, USB).
fn stage_device_list(
    vm_name: &str,
    kind_label: &str,
    cfgs_opt: Option<&Vec<PciDeviceConfig>>,
    inv: &PciInventory,
    unbind_actions: &mut Vec<VfioAction>,
    bind_actions: &mut Vec<VfioAction>,
) -> Result<()> {
    let cfgs = match cfgs_opt {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()),
    };

    let funcs = inv.resolve_configured(cfgs)?;

    for func in funcs {
        append_actions_for_function(vm_name, kind_label, func, unbind_actions, bind_actions);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CpuConfig, DevicesConfig, GpuPolicyConfig, IsolationPolicyConfig, NumaConfig,
        PciDeviceConfig, QemuConfig,
    };

    fn minimal_vm_config_with_gpu(bdf: &str) -> VmConfig {
        VmConfig {
            cpu: CpuConfig {
                host_cpus: "0-3".to_string(),
                vm_cpus: "0-1".to_string(),
            },
            qemu: QemuConfig {
                binary: "/usr/bin/qemu-system-x86_64".to_string(),
                pre_args: None,
                args: "".to_string(),
                post_args: None,
                num_vcpus: 2,
                mem_mb: 2048,
                hugepages: false,
                ovmf_code: "/usr/share/OVMF/OVMF_CODE.fd".to_string(),
                ovmf_vars: "/var/lib/libvirt/qemu/nvram/test_VARS.fd".to_string(),
                smbios: None,
                cpu_extras: None,
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
            isolation: IsolationPolicyConfig::default(),
            peripherals: None,
        }
    }

    fn make_vfio_gpu(bdf: &str, group: u32) -> PciFunction {
        PciFunction {
            bdf: bdf.to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000, // display controller
            driver: Some("vfio-pci".to_string()),
            iommu_group: Some(group),
            numa_node: Some(0),
        }
    }

    #[test]
    fn build_plan_for_vm_with_vfio_gpu_and_no_others_is_empty_or_bind_only() {
        let bdf = "0000:01:00.0";
        let vm_name = "testvm";

        let cfg = minimal_vm_config_with_gpu(bdf);
        let gpu = make_vfio_gpu(bdf, 10);

        let inv = PciInventory {
            functions: vec![gpu],
        };

        let plan = build_plan_for_vm(vm_name, &cfg, &inv).unwrap();

        // For a GPU already vfio-bound and isolated, expect ≤1 bind action.
        assert!(plan.actions.len() <= 1);
        if let Some(action) = plan.actions.get(0) {
            assert_eq!(action.bdf, bdf);
            assert!(matches!(action.kind, VfioActionKind::BindToVfio));
        }
    }
}
