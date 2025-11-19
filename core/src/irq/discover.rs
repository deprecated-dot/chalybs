use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tracing::{debug, info};

use crate::config::{DevicesConfig, PciDeviceConfig, VmConfig};
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

/// Collect all configured PCI devices (gpu, nvme, nic, usb) into a flat list.
fn collect_devices(cfg: &VmConfig) -> Vec<PciDeviceConfig> {
    let mut out = Vec::new();
    let DevicesConfig { gpu, nvme, nic, usb } = &cfg.devices;

    if let Some(v) = gpu {
        out.extend(v.clone());
    }
    if let Some(v) = nvme {
        out.extend(v.clone());
    }
    if let Some(v) = nic {
        out.extend(v.clone());
    }
    if let Some(v) = usb {
        out.extend(v.clone());
    }

    out
}

fn msi_irqs_dir(pci_addr: &str) -> PathBuf {
    Path::new("/sys/bus/pci/devices").join(pci_addr).join("msi_irqs")
}

/// Wait for a single PCI device's MSI/MSI-X IRQs to appear.
/// If `required` is true, timeout is an error. If false, we log and continue.
fn wait_for_device_msi(dev: &PciDeviceConfig) -> Result<()> {
    let dir = msi_irqs_dir(&dev.pci_address);

    const MAX_ITER: u32 = 400;
    const SLEEP_MS: u64 = 5;

    info!(
        pci = %dev.pci_address,
        required = dev.required,
        "waiting for MSI/MSI-X IRQs"
    );

    for attempt in 0..MAX_ITER {
        if dir.exists() {
            // Try to read IRQ entries
            let mut irqs = Vec::new();
            match fs::read_dir(&dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        if let Ok(name) = entry.file_name().into_string() {
                            if let Ok(irq) = name.parse::<u32>() {
                                irqs.push(irq);
                            }
                        }
                    }
                }
                Err(e) => {
                    debug!(
                        pci = %dev.pci_address,
                        err = %e,
                        "failed to read msi_irqs directory"
                    );
                }
            }

            if !irqs.is_empty() {
                info!(
                    pci = %dev.pci_address,
                    ?irqs,
                    "device MSI/MSI-X IRQs are ready"
                );
                return Ok(());
            }
        } else {
            debug!(
                pci = %dev.pci_address,
                path = %dir.display(),
                "msi_irqs directory not present yet"
            );
        }

        thread::sleep(Duration::from_millis(SLEEP_MS));
        if attempt == MAX_ITER - 1 {
            break;
        }
    }

    if dev.required {
        Err(ChalybsError::Irq(format!(
            "timed out waiting for MSI/MSI-X IRQs for required PCI device {} ({} attempts)",
            dev.pci_address, MAX_ITER
        )))
    } else {
        info!(
            pci = %dev.pci_address,
            "no MSI/MSI-X IRQs discovered but device not marked as required; continuing"
        );
        Ok(())
    }
}

/// Public entry point used by VmStateMachine.
/// Ensures all configured devices with MSI/MSI-X have their IRQs registered
/// before we move on to pinning.
pub fn wait_for_msi(rt: &VmRuntime) -> Result<()> {
    let devices = collect_devices(&rt.cfg);

    if devices.is_empty() {
        info!("no PCI devices configured; skipping MSI wait");
        return Ok(());
    }

    for dev in &devices {
        wait_for_device_msi(dev)?;
    }

    Ok(())
}
