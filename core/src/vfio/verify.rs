//! VFIO post-staging verification.
//!
//! After we execute the VFIO action plan (unbind current drivers,
//! bind to vfio-pci), we re-scan PCI inventory and verify that all
//! configured passthrough devices are now bound to vfio-pci.
//!
//! This module is deliberately read-only with respect to sysfs; it
//! consumes the `PciInventory` abstraction.

use tracing::{debug, info};

use crate::config::{PciDeviceConfig, VmConfig};
use crate::errors::{ChalybsError, Result};
use crate::pci::{PciFunction, PciInventory};

/// Public entrypoint: verify that all configured passthrough devices
/// for this VM are now bound to vfio-pci.
///
/// This performs a fresh PCI inventory scan and delegates to
/// `verify_with_inventory` to keep tests hermetic.
pub fn verify_vm_vfio_bindings(vm_name: &str, cfg: &VmConfig) -> Result<()> {
    let inv = PciInventory::scan()?;
    verify_with_inventory(vm_name, cfg, &inv)
}

/// Internal helper: verify bindings using a provided inventory.
/// Exposed to tests but not re-exported at the module root.
fn verify_with_inventory(vm_name: &str, cfg: &VmConfig, inv: &PciInventory) -> Result<()> {
    // GPUs: must be present, be actual display controllers, and be
    // bound to vfio-pci.
    verify_device_list(
        vm_name,
        "GPU",
        cfg.devices.gpu.as_ref(),
        inv,
        Some(DeviceKind::Gpu),
    )?;

    // Generic PCI devices: NVMe, NIC, USB.
    verify_device_list(
        vm_name,
        "NVMe",
        cfg.devices.nvme.as_ref(),
        inv,
        Some(DeviceKind::Generic),
    )?;

    verify_device_list(
        vm_name,
        "NIC",
        cfg.devices.nic.as_ref(),
        inv,
        Some(DeviceKind::Generic),
    )?;

    verify_device_list(
        vm_name,
        "USB",
        cfg.devices.usb.as_ref(),
        inv,
        Some(DeviceKind::Generic),
    )?;

    info!(
        vm = vm_name,
        "vfio: verified vfio-pci bindings for all configured passthrough devices"
    );
    Ok(())
}

/// Logical kind of device — used for extra defensive checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    /// A GPU or GPU-like PCI device (class 0x03xxxx), or members of a
    /// GPU complex (e.g. audio functions sharing a slot with a GPU).
    Gpu,
    /// Generic PCI passthrough device.
    Generic,
}

/// Verify a device category (GPU, NVMe, NIC, USB).
fn verify_device_list(
    vm_name: &str,
    kind_label: &str,
    cfgs_opt: Option<&Vec<PciDeviceConfig>>,
    inv: &PciInventory,
    kind: Option<DeviceKind>,
) -> Result<()> {
    let cfgs = match cfgs_opt {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(()),
    };

    // resolve_configured() enforces required= true semantics and skips
    // optional devices that are missing. This ensures we only verify
    // devices that "exist" in this inventory context.
    let funcs = inv.resolve_configured(cfgs)?;

    for func in funcs {
        verify_single_device(vm_name, kind_label, kind, inv, func)?;
    }

    Ok(())
}

/// Verify one PCI function: correct class (for GPUs) and vfio-pci binding.
fn verify_single_device(
    vm_name: &str,
    kind_label: &str,
    kind: Option<DeviceKind>,
    inv: &PciInventory,
    func: &PciFunction,
) -> Result<()> {
    let bdf = func.bdf.as_str();

    // Additional defensive checks for GPU entries.
    if let Some(DeviceKind::Gpu) = kind {
        if !func.is_display_controller() && !inv.is_bdf_in_gpu_complex(bdf) {
            return Err(ChalybsError::Vfio(format!(
                "VM {vm_name}: device {bdf} configured as GPU, \
                 but PCI class base is not 0x03 (display controller)"
            )));
        }
    }

    match func.driver.as_deref() {
        Some("vfio-pci") => {
            debug!(
                vm = vm_name,
                bdf = bdf,
                kind = kind_label,
                "vfio: verified device is bound to vfio-pci"
            );
            Ok(())
        }
        Some(other) => Err(ChalybsError::Vfio(format!(
            "VM {vm_name}: {kind_label} device {bdf} is bound to `{other}`, \
             expected `vfio-pci` after VFIO staging"
        ))),
        None => Err(ChalybsError::Vfio(format!(
            "VM {vm_name}: {kind_label} device {bdf} has no bound driver, \
             expected `vfio-pci` after VFIO staging"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CpuConfig, DevicesConfig, GpuPolicyConfig, IsolationPolicyConfig, NumaConfig,
        PciDeviceConfig, QemuConfig,
    };

    fn minimal_vm_config_for_devices(
        gpu_bdfs: &[&str],
        nvme_bdfs: &[&str],
        nic_bdfs: &[&str],
        usb_bdfs: &[&str],
    ) -> VmConfig {
        let mk_list = |bdfs: &[&str]| {
            if bdfs.is_empty() {
                None
            } else {
                Some(
                    bdfs.iter()
                        .map(|bdf| PciDeviceConfig {
                            pci_address: (*bdf).to_string(),
                            required: true,
                            level: None,
                        })
                        .collect(),
                )
            }
        };

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
                gpu: mk_list(gpu_bdfs),
                nvme: mk_list(nvme_bdfs),
                nic: mk_list(nic_bdfs),
                usb: mk_list(usb_bdfs),
            },
            gpu: GpuPolicyConfig {
                allow_single_gpu: false,
                force_use_igpu: false,
            },
            isolation: IsolationPolicyConfig::default(),
            peripherals: None,
        }
    }

    fn make_gpu_vfio(bdf: &str) -> PciFunction {
        PciFunction {
            bdf: bdf.to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000, // display controller
            driver: Some("vfio-pci".to_string()),
            iommu_group: Some(10),
            numa_node: Some(0),
        }
    }

    fn make_nvme_vfio(bdf: &str) -> PciFunction {
        PciFunction {
            bdf: bdf.to_string(),
            vendor_id: 0x8086,
            device_id: 0x1234,
            class: 0x010802, // NVMe
            driver: Some("vfio-pci".to_string()),
            iommu_group: Some(20),
            numa_node: Some(0),
        }
    }

    #[test]
    fn verify_with_inventory_succeeds_when_all_devices_are_vfio_bound() {
        let gpu_bdf = "0000:01:00.0";
        let nvme_bdf = "0000:02:00.0";

        let cfg = minimal_vm_config_for_devices(&[gpu_bdf], &[nvme_bdf], &[], &[]);

        let inv = PciInventory {
            functions: vec![make_gpu_vfio(gpu_bdf), make_nvme_vfio(nvme_bdf)],
        };

        verify_with_inventory("testvm", &cfg, &inv).unwrap();
    }

    #[test]
    fn verify_with_inventory_fails_if_gpu_not_vfio_bound() {
        let gpu_bdf = "0000:01:00.0";
        let cfg = minimal_vm_config_for_devices(&[gpu_bdf], &[], &[], &[]);

        let func = PciFunction {
            bdf: gpu_bdf.to_string(),
            vendor_id: 0x1234,
            device_id: 0x5678,
            class: 0x030000,
            driver: Some("amdgpu".to_string()),
            iommu_group: Some(10),
            numa_node: Some(0),
        };

        let inv = PciInventory {
            functions: vec![func],
        };

        let err = verify_with_inventory("testvm", &cfg, &inv).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("expected `vfio-pci`"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn verify_with_inventory_fails_if_gpu_class_not_display() {
        let gpu_bdf = "0000:01:00.0";
        let cfg = minimal_vm_config_for_devices(&[gpu_bdf], &[], &[], &[]);

        // Misconfigured: marked as GPU in config, but class is NIC-like.
        let func = PciFunction {
            bdf: gpu_bdf.to_string(),
            vendor_id: 0x1234,
            device_id: 0xabcd,
            class: 0x020000, // network controller, not display
            driver: Some("vfio-pci".to_string()),
            iommu_group: Some(10),
            numa_node: Some(0),
        };

        let inv = PciInventory {
            functions: vec![func],
        };

        let err = verify_with_inventory("testvm", &cfg, &inv).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("configured as GPU") && msg.contains("class base is not 0x03"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn verify_with_inventory_fails_if_required_gpu_missing_from_inventory() {
        let gpu_bdf = "0000:05:00.0";
        let cfg = minimal_vm_config_for_devices(&[gpu_bdf], &[], &[], &[]);

        // Inventory is *empty* — required GPU does not resolve.
        let inv = PciInventory { functions: vec![] };

        // We don't care about exact error text, only that this is an error.
        let res = verify_with_inventory("testvm", &cfg, &inv);
        assert!(
            res.is_err(),
            "expected error when required GPU is missing from inventory"
        );
    }
}
