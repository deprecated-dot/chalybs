// core/src/irq/pin.rs

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::config::{DevicesConfig, PciDeviceConfig, VmConfig};
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;
use crate::util::parse_cpu_list;

/// Collect all configured PCI devices (gpu, nvme, nic, usb) into a flat list.
fn collect_devices(cfg: &VmConfig) -> Vec<PciDeviceConfig> {
    let mut out = Vec::new();
    let DevicesConfig {
        gpu,
        nvme,
        nic,
        usb,
    } = &cfg.devices;

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
    Path::new("/sys/bus/pci/devices")
        .join(pci_addr)
        .join("msi_irqs")
}

fn device_numa_node(pci_addr: &str) -> Option<i32> {
    let path = Path::new("/sys/bus/pci/devices")
        .join(pci_addr)
        .join("numa_node");
    match fs::read_to_string(&path) {
        Ok(s) => s.trim().parse::<i32>().ok(),
        Err(_) => None,
    }
}

fn node_cpus(node: i32) -> Result<Vec<u32>> {
    let path = Path::new("/sys/devices/system/node")
        .join(format!("node{node}"))
        .join("cpulist");

    let data = fs::read_to_string(&path).map_err(|e| {
        ChalybsError::Irq(format!(
            "failed to read NUMA node cpulist from {}: {e}",
            path.display()
        ))
    })?;

    parse_cpu_list(data.trim()).map_err(|e| {
        ChalybsError::Irq(format!(
            "failed to parse NUMA node cpulist {}: {e}",
            path.display()
        ))
    })
}

fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let set_b: BTreeSet<u32> = b.iter().copied().collect();
    let mut out = Vec::new();
    for &v in a {
        if set_b.contains(&v) {
            out.push(v);
        }
    }
    out
}

/// Decide which CPUs to use for IRQs of a given device:
///   - If the device has a NUMA node (>=0), intersect that node's CPUs
///     with the VM CPU set.
///   - If no NUMA node, use the full VM CPU set.
///
/// This keeps IRQs and vCPUs on the same NUMA node when possible.
fn target_cpus_for_device(vm_cpus: &[u32], dev: &PciDeviceConfig) -> Result<Vec<u32>> {
    let node = device_numa_node(&dev.pci_address).unwrap_or(-1);
    if node >= 0 {
        let node_cpus = node_cpus(node)?;
        let subset = intersect(vm_cpus, &node_cpus);

        if subset.is_empty() {
            return Err(ChalybsError::Irq(format!(
                "no VM CPUs on NUMA node {} for device {}; vm_cpus={:?}, node_cpus={:?}",
                node, dev.pci_address, vm_cpus, node_cpus
            )));
        }

        info!(
            pci = %dev.pci_address,
            numa_node = node,
            vm_cpus = ?vm_cpus,
            target_cpus = ?subset,
            "using NUMA-local VM CPUs for device IRQs"
        );
        Ok(subset)
    } else {
        info!(
            pci = %dev.pci_address,
            "device has no NUMA node; using full VM CPU set for IRQs"
        );
        Ok(vm_cpus.to_vec())
    }
}

/// Strict IRQ discovery used by the synchronous path.
/// This preserves the old semantics: required devices with missing/empty
/// msi_irqs cause an error.
fn device_irqs(dev: &PciDeviceConfig) -> Result<Vec<u32>> {
    let dir = msi_irqs_dir(&dev.pci_address);
    if !dir.exists() {
        if dev.required {
            return Err(ChalybsError::Irq(format!(
                "required device {} has no msi_irqs directory at {}",
                dev.pci_address,
                dir.display()
            )));
        } else {
            debug!(
                pci = %dev.pci_address,
                "device has no msi_irqs directory; treating as non-MSI/MSI-X"
            );
            return Ok(Vec::new());
        }
    }

    let mut irqs = Vec::new();
    for entry in fs::read_dir(&dir)
        .map_err(|e| {
            ChalybsError::Irq(format!(
                "failed to read msi_irqs for {}: {e}",
                dev.pci_address
            ))
        })?
        .flatten()
    {
        if let Ok(name) = entry.file_name().into_string() {
            if let Ok(irq) = name.parse::<u32>() {
                irqs.push(irq);
            }
        }
    }

    if irqs.is_empty() && dev.required {
        return Err(ChalybsError::Irq(format!(
            "required device {} has empty msi_irqs directory",
            dev.pci_address
        )));
    }

    Ok(irqs)
}

/// Non-fatal IRQ discovery used by the background worker.
/// Never returns an error; just an empty vec if nothing is present yet.
fn device_irqs_best_effort(dev: &PciDeviceConfig) -> Vec<u32> {
    let dir = msi_irqs_dir(&dev.pci_address);
    if !dir.exists() {
        debug!(
            pci = %dev.pci_address,
            path = %dir.display(),
            "msi_irqs directory not present yet"
        );
        return Vec::new();
    }

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
                "failed to read msi_irqs directory (best-effort)"
            );
        }
    }

    irqs
}

fn format_cpu_list(cpus: &[u32]) -> String {
    let mut v = cpus.to_vec();
    v.sort_unstable();
    v.dedup();
    v.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn pin_single_irq(irq: u32, cpus: &[u32]) -> Result<()> {
    let irq_dir = Path::new("/proc/irq").join(irq.to_string());
    if !irq_dir.exists() {
        return Err(ChalybsError::Irq(format!(
            "IRQ {} directory {} does not exist",
            irq,
            irq_dir.display()
        )));
    }

    let list_path = irq_dir.join("smp_affinity_list");
    if !list_path.exists() {
        return Err(ChalybsError::Irq(format!(
            "smp_affinity_list not found for IRQ {} at {} (kernel may be too old or not support cpulist interface)",
            irq,
            list_path.display()
        )));
    }

    let val = format_cpu_list(cpus);

    fs::write(&list_path, val.as_bytes()).map_err(|e| {
        ChalybsError::Irq(format!(
            "failed to write IRQ affinity for {} to {}: {e}",
            irq,
            list_path.display()
        ))
    })?;

    info!(
        irq,
        cpus = %val,
        path = %list_path.display(),
        "pinned IRQ to CPUs"
    );

    Ok(())
}

/// Background worker body: best-effort discovery + pinning.
/// This never blocks VM bring-up and never returns errors to the caller.
///
/// Semantics:
///   - For each device:
///       * Determine target CPUs (NUMA-aware).
///       * Poll msi_irqs until IRQs appear or we give up after a
///         bounded number of attempts.
///       * When IRQs appear, pin them.
///   - If IRQs never appear, we log a warning and move on.
fn irq_worker(vm_cpus: Vec<u32>, devices: Vec<PciDeviceConfig>) {
    if devices.is_empty() {
        info!("irq worker: no PCI devices configured; exiting");
        return;
    }

    // Polling parameters: light-weight, non-blocking w.r.t. VM lifecycle.
    const MAX_ITER: u32 = 2000; // 2000 * 5ms = 10 seconds per device
    const SLEEP_MS: u64 = 5;

    for dev in devices {
        let target_cpus = match target_cpus_for_device(&vm_cpus, &dev) {
            Ok(cpus) => cpus,
            Err(e) => {
                warn!(
                    pci = %dev.pci_address,
                    error = %e,
                    "irq worker: failed to determine target CPUs for device; skipping"
                );
                continue;
            }
        };

        let mut irqs: Vec<u32> = Vec::new();

        for attempt in 0..MAX_ITER {
            irqs = device_irqs_best_effort(&dev);
            if !irqs.is_empty() {
                info!(
                    pci = %dev.pci_address,
                    ?irqs,
                    attempts = attempt + 1,
                    "irq worker: MSI/MSI-X IRQs discovered; pinning"
                );
                break;
            }

            thread::sleep(Duration::from_millis(SLEEP_MS));
        }

        if irqs.is_empty() {
            warn!(
                pci = %dev.pci_address,
                max_attempts = MAX_ITER,
                "irq worker: no MSI/MSI-X IRQs discovered; leaving device IRQs unpinned"
            );
            continue;
        }

        let cpu_list_str = format_cpu_list(&target_cpus);

        for irq in irqs {
            if let Err(e) = pin_single_irq(irq, &target_cpus) {
                warn!(
                    pci = %dev.pci_address,
                    irq,
                    cpus = %cpu_list_str,
                    error = %e,
                    "irq worker: failed to pin IRQ"
                );
            }
        }
    }

    info!("irq worker: completed IRQ discovery and pinning for all devices");
}

/// Public entry point used by VmStateMachine for asynchronous IRQ pinning.
///
/// This:
///   - Collects devices from the VM config.
///   - Clones the VM CPU list from the runtime.
///   - Spawns a background worker thread that performs best-effort
///     IRQ discovery and pinning without blocking VM bring-up.
pub fn spawn_irq_pin_worker(rt: &VmRuntime) -> Result<()> {
    let devices = collect_devices(&rt.cfg);

    if devices.is_empty() {
        info!("no PCI devices configured; skipping IRQ pinning worker");
        return Ok(());
    }

    let vm_cpus = rt.cpus.vm.cpus.clone();

    info!(
        device_count = devices.len(),
        "spawning IRQ pinning worker thread"
    );

    thread::spawn(move || {
        irq_worker(vm_cpus, devices);
    });

    Ok(())
}

/// Public synchronous entry point (unchanged semantics).
///
/// For each configured PCI device:
///   - Determine NUMA-local VM CPUs
///   - Discover MSI/MSI-X IRQs (strictly; required devices must have IRQs)
///   - Pin each IRQ to that CPU set
///
/// This is useful for manual tools/tests; VmStateMachine should prefer
/// `spawn_irq_pin_worker` for non-blocking behavior.
pub fn pin_irqs(rt: &VmRuntime) -> Result<()> {
    let devices = collect_devices(&rt.cfg);

    if devices.is_empty() {
        info!("no PCI devices configured; skipping IRQ pinning");
        return Ok(());
    }

    let vm_cpus = rt.cpus.vm.cpus.clone();

    for dev in &devices {
        let target_cpus = target_cpus_for_device(&vm_cpus, dev)?;

        let irqs = device_irqs(dev)?;
        if irqs.is_empty() {
            info!(
                pci = %dev.pci_address,
                "no IRQs discovered for device (non-MSI/MSI-X or not required)"
            );
            continue;
        }

        for irq in irqs {
            pin_single_irq(irq, &target_cpus)?;
        }
    }

    Ok(())
}
