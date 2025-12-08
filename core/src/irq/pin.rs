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

/// Check if a process with the given PID appears to exist.
///
/// This is used as the *only* stop condition for the IRQ worker
/// besides successfully discovering MSI/MSI-X IRQs. There is no
/// heuristic timeout; as long as QEMU is alive, we keep polling
/// deterministically for IRQs.
fn process_exists(pid: i32) -> bool {
    let path = Path::new("/proc").join(pid.to_string());
    path.exists()
}

/// Parse "0000:bb:dd.f" into ("0000:bb:dd", func).
///
/// We treat the function number as decimal (0–7), matching kernel BDF
/// formatting. Returns None on malformed BDFs.
fn parse_bdf_slot_and_func(bdf: &str) -> Option<(String, u8)> {
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

    // Enforce widths for the slot portion; func can be 0–7 with no padding.
    if domain_str.len() != 4 || bus_str.len() != 2 || dev_str.len() != 2 || func_str.is_empty() {
        return None;
    }

    // Domain/bus/dev are hex; function is decimal.
    let domain = u16::from_str_radix(domain_str, 16).ok()?;
    let bus = u8::from_str_radix(bus_str, 16).ok()?;
    let dev = u8::from_str_radix(dev_str, 16).ok()?;
    let func = func_str.parse::<u8>().ok()?;

    Some((format!("{domain:04x}:{bus:02x}:{dev:02x}"), func))
}

/// Determine if a configured PCI device looks like an "auxiliary GPU
/// function" (typically HDMI/audio) for IRQ pinning purposes.
///
/// Heuristic, but *config-only* and deterministic:
///   - Consider only devices configured under `devices.gpu`.
///   - Group configured GPU BDFs by (domain,bus,slot).
///   - If a slot has multiple functions and function 0 is present,
///     then any non-zero function in that slot is treated as auxiliary.
///
/// This lets us special-case GPU HDMI/audio functions without touching
/// global PCI inventory and without guessing based on class codes here.
fn is_aux_gpu_function(dev: &PciDeviceConfig, cfg: &VmConfig) -> bool {
    let (slot, func) = match parse_bdf_slot_and_func(&dev.pci_address) {
        Some(v) => v,
        None => return false,
    };

    let gpu_cfgs = match cfg.devices.gpu.as_ref() {
        Some(v) if !v.is_empty() => v,
        _ => return false,
    };

    let mut same_slot_count = 0usize;
    let mut has_func0 = false;

    for g in gpu_cfgs {
        if let Some((g_slot, g_func)) = parse_bdf_slot_and_func(&g.pci_address) {
            if g_slot == slot {
                same_slot_count += 1;
                if g_func == 0 {
                    has_func0 = true;
                }
            }
        }
    }

    // Only treat as aux if:
    //   - there is more than one GPU function configured for this slot, and
    //   - function 0 is present, and
    //   - this device is a non-zero function in that slot.
    same_slot_count > 1 && has_func0 && func != 0
}

/// Internal per-device state used by the IRQ worker.
struct DeviceWork {
    dev: PciDeviceConfig,
    target_cpus: Vec<u32>,
    is_aux_gpu: bool,
    done: bool,
}

/// Background worker body: best-effort discovery + pinning.
///
/// Deterministic semantics:
///   - Precompute target CPUs + aux-GPU classification for all devices.
///   - While QEMU exists:
///       * Scan all not-yet-done devices:
///           - If MSI/MSI-X IRQs appear, pin them and mark device done.
///       * If no device made progress on this pass, sleep briefly.
///   - If QEMU exits before all devices are done:
///       * For primary devices → WARN and leave IRQs unpinned.
///       * For auxiliary GPU functions (.1 HDMI/audio) → INFO and leave as expected.
///
/// There is **no max-attempt cap**. The only reasons this worker stops are:
///   - QEMU has exited, or
///   - MSI/MSI-X IRQs were successfully discovered and pinned for all
///     configured devices for which we could compute target CPUs.
fn irq_worker(vm_cpus: Vec<u32>, devices: Vec<PciDeviceConfig>, qemu_pid: i32, vm_cfg: VmConfig) {
    if devices.is_empty() {
        info!("irq worker: no PCI devices configured; exiting");
        return;
    }

    const SLEEP_MS: u64 = 5;

    // Precompute per-device work state.
    let mut work_items: Vec<DeviceWork> = Vec::new();

    for dev in devices {
        // *** Minimal change: skip auxiliary GPU functions entirely ***
        let is_aux = is_aux_gpu_function(&dev, &vm_cfg);
        if is_aux {
            info!(
                pci = %dev.pci_address,
                "irq worker: treating device as auxiliary GPU function (likely HDMI/audio); \
                 skipping IRQ pinning for this device"
            );
            continue;
        }

        match target_cpus_for_device(&vm_cpus, &dev) {
            Ok(cpus) => {
                work_items.push(DeviceWork {
                    dev,
                    target_cpus: cpus,
                    is_aux_gpu: false,
                    done: false,
                });
            }
            Err(e) => {
                warn!(
                    pci = %dev.pci_address,
                    error = %e,
                    "irq worker: failed to determine target CPUs for device; skipping"
                );
            }
        }
    }

    if work_items.is_empty() {
        info!("irq worker: no eligible PCI devices after CPU/heuristic filtering; exiting");
        return;
    }

    let total_devices = work_items.len();
    let mut completed_devices: usize = 0;
    let mut aborted_early = false;

    while completed_devices < total_devices {
        // If QEMU is gone, stop polling and emit per-device summaries.
        if !process_exists(qemu_pid) {
            aborted_early = true;

            for w in work_items.iter().filter(|w| !w.done) {
                if w.is_aux_gpu {
                    info!(
                        pci = %w.dev.pci_address,
                        pid = qemu_pid,
                        "irq worker: QEMU process exited before MSI/MSI-X IRQs were discovered \
                         for auxiliary GPU function; leaving device IRQs unpinned (expected for HDMI/audio)"
                    );
                } else {
                    warn!(
                        pci = %w.dev.pci_address,
                        pid = qemu_pid,
                        "irq worker: QEMU process exited before MSI/MSI-X IRQs were discovered; \
                         leaving device IRQs unpinned"
                    );
                }
            }

            break;
        }

        let mut made_progress = false;

        for w in work_items.iter_mut().filter(|w| !w.done) {
            let irqs = device_irqs_best_effort(&w.dev);
            if irqs.is_empty() {
                continue;
            }

            made_progress = true;

            if w.is_aux_gpu {
                info!(
                    pci = %w.dev.pci_address,
                    ?irqs,
                    "irq worker: MSI/MSI-X IRQs discovered for auxiliary GPU function; pinning"
                );
            } else {
                info!(
                    pci = %w.dev.pci_address,
                    ?irqs,
                    "irq worker: MSI/MSI-X IRQs discovered; pinning"
                );
            }

            let cpu_list_str = format_cpu_list(&w.target_cpus);

            for irq in irqs {
                if let Err(e) = pin_single_irq(irq, &w.target_cpus) {
                    warn!(
                        pci = %w.dev.pci_address,
                        irq,
                        cpus = %cpu_list_str,
                        error = %e,
                        "irq worker: failed to pin IRQ"
                    );
                }
            }

            w.done = true;
            completed_devices += 1;
        }

        if completed_devices >= total_devices {
            break;
        }

        if !made_progress {
            thread::sleep(Duration::from_millis(SLEEP_MS));
        }
    }

    if aborted_early {
        warn!(
            completed = completed_devices,
            total = total_devices,
            "irq worker: exiting before all devices were processed (QEMU exited early)"
        );
    } else {
        info!(
            device_count = total_devices,
            "irq worker: completed IRQ discovery and pinning for all devices"
        );

        // Deterministic post-IRQ DDC hook:
        // If a DDC peripheral is configured, switch to the VM input
        // exactly once, at the moment IRQ pinning is known-complete.
        if let Some(periph_cfg) = vm_cfg.peripherals {
            if let Some(ddc_cfg) = periph_cfg.ddc {
                if let Err(e) = crate::peripherals::ddc::switch_to_vm_input_after_irq(ddc_cfg) {
                    warn!(
                        error = %e,
                        "irq worker: DDC post-IRQ input switch failed"
                    );
                }
            }
        }
    }
}

/// Public entry point used by VmStateMachine for asynchronous IRQ pinning.
///
/// This:
///   - Collects devices from the VM config.
///   - Clones the VM CPU list from the runtime.
///   - Captures the QEMU pid from the runtime.
///   - Clones the VmConfig for auxiliary GPU classification.
///   - Spawns a background worker thread that performs best-effort
///     IRQ discovery and pinning without blocking VM bring-up.
///
/// Semantics:
///   - As long as QEMU is alive, the worker will continue polling for
///     MSI/MSI-X IRQs; there is no heuristic timeout.
///   - Auxiliary GPU functions (.1 HDMI/audio) are classified from
///     config; missing MSI/MSI-X on those devices is logged as INFO,
///     not WARNING, and is treated as expected.
///   - If QEMU exits before IRQs appear, the worker stops.
pub fn spawn_irq_pin_worker(rt: &VmRuntime) -> Result<()> {
    let devices = collect_devices(&rt.cfg);

    if devices.is_empty() {
        info!("no PCI devices configured; skipping IRQ pinning worker");
        return Ok(());
    }

    let vm_cpus = rt.cpus.vm.cpus.clone();
    let vm_cfg = rt.cfg.clone();

    let qemu_pid = match rt.qemu {
        Some(ref q) => q.pid,
        None => {
            warn!("spawn_irq_pin_worker: no QEMU process recorded in runtime; skipping IRQ pinning worker");
            return Ok(());
        }
    };

    info!(
        device_count = devices.len(),
        pid = qemu_pid,
        "spawning IRQ pinning worker thread"
    );

    thread::spawn(move || {
        irq_worker(vm_cpus, devices, qemu_pid, vm_cfg);
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
