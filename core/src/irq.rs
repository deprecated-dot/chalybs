use std::fs;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

/// How long we wait for vfio-related MSI/MSI-X IRQs to appear.
const MSI_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often we poll /proc/interrupts for vfio IRQs.
const MSI_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Wait until we see at least one vfio-related IRQ in /proc/interrupts.
///
/// This is a pragmatic stand-in for “MSI/MSI-X readiness” based on the
/// existing, working Bash behavior: we don’t sleep blindly, we actually
/// wait for the kernel to report interrupts for vfio devices.
pub fn wait_for_msi(_rt: &VmRuntime) -> Result<()> {
    let start = Instant::now();

    loop {
        let irqs = parse_vfio_irqs()?;

        if !irqs.is_empty() {
            info!(count = irqs.len(), ?irqs, "vfio IRQs detected");
            return Ok(());
        }

        if start.elapsed() > MSI_WAIT_TIMEOUT {
            return Err(ChalybsError::Irq(
                "timeout waiting for vfio MSI/MSI-X interrupts".into(),
            ));
        }

        debug!("no vfio IRQs yet, polling again…");
        std::thread::sleep(MSI_POLL_INTERVAL);
    }
}

/// Pin all vfio-related IRQs to the VM cores defined in rt.cpus.vm.
///
/// This is a generic, config-free heuristic based on /proc/interrupts:
/// any line whose description contains "vfio" is treated as a candidate.
pub fn pin_irqs(rt: &VmRuntime) -> Result<()> {
    let irqs = parse_vfio_irqs()?;

    if irqs.is_empty() {
        warn!("no vfio IRQs discovered in /proc/interrupts; nothing to pin");
        return Ok(());
    }

    let vm_cpus = &rt.cpus.vm.cpus;
    if vm_cpus.is_empty() {
        return Err(ChalybsError::Irq(
            "no vm cpus configured; cannot compute IRQ affinity mask".into(),
        ));
    }

    let mask = cpus_to_hex_mask(vm_cpus)?;

    info!(
        ?irqs,
        ?vm_cpus,
        mask = %mask,
        "pinning vfio IRQs to vm cpus"
    );

    for irq in irqs {
        let path = format!("/proc/irq/{}/smp_affinity", irq);
        debug!(irq, path = %path, "writing smp_affinity for IRQ");

        // Kernel accepts a plain hex mask (no 0x prefix, newline okay).
        fs::write(&path, format!("{mask}\n")).map_err(|e| {
            ChalybsError::Irq(format!(
                "failed to write smp_affinity for IRQ {} at {}: {e}",
                irq, path
            ))
        })?;
    }

    Ok(())
}

/// Parse /proc/interrupts and return IRQ numbers whose description contains "vfio".
fn parse_vfio_irqs() -> Result<Vec<u32>> {
    let contents = fs::read_to_string("/proc/interrupts").map_err(|e| {
        ChalybsError::Irq(format!("failed to read /proc/interrupts: {e}"))
    })?;

    let mut irqs = Vec::new();

    for line in contents.lines() {
        // Skip headers or malformed lines.
        if !line.contains("vfio") {
            continue;
        }

        // IRQ number is the first token before ':'.
        if let Some((left, _rest)) = line.split_once(':') {
            let irq_str = left.trim();
            if let Ok(n) = irq_str.parse::<u32>() {
                irqs.push(n);
            }
        }
    }

    irqs.sort_unstable();
    irqs.dedup();

    debug!(?irqs, "parsed vfio IRQs from /proc/interrupts");

    Ok(irqs)
}

/// Convert a list of CPU indices into a hex mask suitable for smp_affinity.
///
/// This uses a u128 mask, which is sufficient for up to 128 CPUs. For your
/// TR boxes, that’s plenty.
fn cpus_to_hex_mask(cpus: &[u32]) -> Result<String> {
    let mut mask: u128 = 0;

    for &cpu in cpus {
        if cpu >= 128 {
            return Err(ChalybsError::Irq(format!(
                "CPU index {} is too large for u128 mask (>=128)",
                cpu
            )));
        }
        mask |= 1u128 << cpu;
    }

    if mask == 0 {
        return Err(ChalybsError::Irq(
            "computed zero IRQ affinity mask from vm cpus".into(),
        ));
    }

    Ok(format!("{:x}", mask))
}
