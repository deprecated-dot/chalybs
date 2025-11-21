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
/// This performs a fresh PCI inventory scan and delegates to the
/// internal `verify_with_inventory` to keep tests hermetic.
pub fn verify_vm_vfio_bindings(vm_name: &str, cfg: &VmConfig) -> Result<()> {
    let inv = PciInventory::scan()?;
    verify_with_inventory(vm_name, cfg, &inv)
}

/// Internal helper: verify bindings using a provided inventory.
/// This is exposed to tests but not re-exported from the module tree.
fn verify_with_inventory(
    vm_name: &str,
    cfg: &VmConfig,
    inv: &PciInventory,
) -> Result<()> {
    // GPUs: must be present and bound to vfio-pci, and must actually be
    // display controllers (defensive check against config mistakes).
    verify_device_list(
        vm_name,
        "GPU",
        cfg.devices.gpu.as_ref(),
        inv,
        Some(DeviceKind::Gpu),
    )?;

    // Non-GPU devices: NVMe, NIC, USB — currently we only require that
    // they resolve and are bound to vfio-pci if present.
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

/// Logical kind of device, used for additional sanity checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    /// GPU-like device (display controller). Must have base class 0x03.
    Gpu,
    /// Any other PCI function we treat generically.
    Generic,
}

/// Shared verification logic for a list of configured devices of a
/// given kind (GPU, NVMe, NIC, USB).
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

    // resolve_configured() enforces presence for required devices and
    // skips optional devices that are missing.
    let funcs = inv.resolve_configured(cfgs)?;

    for func in funcs {
        verify_single_device(vm_name, kind_label, kind, func)?;
    }

    Ok(())
}

/// Verify a single PCI function's binding.
fn verify_single_device(
    vm_name: &str,
    kind_label: &str,
    kind: Option<DeviceKind>,
    func: &PciFunction,
) -> Result<()> {
    let bdf = func.bdf.as_str();

    if let Some(DeviceKind::Gpu) = kind {
        // Defensive check: ensure the device really is a display
        // controller; catch miswired configs early.
        if !func.is_display_controller() {
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
        CpuConfig, DevicesConfig, GpuPolicyConfig, NumaConfig, QemuConfig,
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
                    bdfs
                        .iter()
                        .map(|bdf| PciDeviceConfig {
                            pci_address: (*bdf).to_string(),
                            required: true,
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
                args: "".to_string(),
                num_vcpus: 2,
                mem_mb: 2048,
                hugepages: false,
                ovmf_code: "/usr/share/OVMF/OVMF_CODE.fd".to_string(),
                ovmf_vars: "/var/lib/libvirt/qemu/nvram/test_VARS.fd".to_string(),
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

        let cfg = minimal_vm_config_for_devices(
            &[gpu_bdf],
            &[nvme_bdf],
            &[],
            &[],
        );

        let inv = PciInventory {
            functions: vec![
                make_gpu_vfio(gpu_bdf),
                make_nvme_vfio(nvme_bdf),
            ],
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
}
