//! VFIO orchestration layer
//!
//! This module is responsible for turning PCI inventory + VM config into
//! a concrete sequence of VFIO actions (unbind from current driver,
//! bind to vfio-pci) and executing them in a deterministic order,
//! followed by a verification pass to ensure all configured passthrough
//! devices are actually vfio-bound, and finally restoring original
//! driver bindings during shutdown.
//!
//! High-level entrypoints for the state machine:
//!   - vfio::stage_pci_devices_for_vm(&mut VmRuntime)
//!   - vfio::restore_pci_devices_for_vm(&VmRuntime)
//!
//! Phases covered here:
//!   - Phase 5: VFIO action plan builder + execution
//!   - Phase 6: VFIO post-execution verification
//!   - Phase 7: VFIO restoration (deterministic teardown)
//!   - Phase 8: Device isolation policy gate (IOMMU-group focused)

mod execute;
mod isolation;
mod plan;
mod verify;

pub use execute::execute_plan;
pub use plan::{build_plan_for_vm, VfioAction, VfioActionKind, VfioPlan};
pub use verify::verify_vm_vfio_bindings;

use crate::errors::Result;
use crate::model::VmRuntime;
use crate::pci::{PciFunction, PciInventory};
use tracing::{debug, info, warn};

/// Mode for VFIO restore execution.
///
/// Live   → perform actual sysfs writes to restore drivers.
/// DryRun → plan and log restores, but do not touch sysfs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    Live,
    DryRun,
}

/// Result classification for a single restore attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreResultKind {
    /// Restore was executed successfully (only in Live mode).
    Succeeded,
    /// Restore was planned but not executed because this was a dry run.
    DryRunPlanned,
    /// Device was originally unbound; no restore performed.
    SkippedOriginallyUnbound,
    /// Device was originally bound to vfio-pci; no restore performed.
    SkippedOriginallyVfio,
    /// PCI device from transitions could not be found in current inventory.
    SkippedDeviceMissing,
    /// Restore failed even after attempting to bind to the original driver.
    Failed,
}

/// Per-device restore outcome.
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    pub bdf: String,
    pub original_driver: Option<String>,
    pub target_driver: Option<String>,
    pub iommu_group: Option<u32>,
    pub result: RestoreResultKind,
    pub reason: Option<String>,
}

/// Aggregate restore summary for a VM.
#[derive(Debug, Clone)]
pub struct RestoreSummary {
    pub vm_name: String,
    pub mode: RestoreMode,
    pub outcomes: Vec<RestoreOutcome>,
}

impl RestoreSummary {
    /// Number of devices whose restore succeeded (Live mode only).
    pub fn restored_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.result, RestoreResultKind::Succeeded))
            .count()
    }

    /// Number of devices whose restore failed.
    pub fn failed_count(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.result, RestoreResultKind::Failed))
            .count()
    }
}

/// Stage all PCI devices required by this VM for passthrough:
///
/// 1. Build a VFIO action plan from VM config + PCI inventory.
/// 2. Run the Phase 8 isolation policy gate for this VM.
/// 3. Execute the plan (unbind current drivers, bind to vfio-pci),
///    recording the original driver bindings in the VmRuntime.
/// 4. Re-scan PCI inventory and verify that all configured passthrough
///    devices are now bound to vfio-pci.
///
/// This function is called from VmState::PreparePci.
pub fn stage_pci_devices_for_vm(rt: &mut VmRuntime) -> Result<()> {
    // Phase 5: plan.
    let inv = PciInventory::scan()?;
    let plan = build_plan_for_vm(&rt.name, &rt.cfg, &inv)?;

    info!(
        vm = rt.name.as_str(),
        actions = plan.actions.len(),
        "vfio: staging PCI devices for VM"
    );

    // Phase 8: device isolation policy gate.
    //
    // This is a pure, read-only check that consumes VmConfig and the
    // current PCI inventory. Depending on the per-VM isolation mode,
    // violations are either logged (Audit) or treated as hard errors
    // (Enforce). In Disabled mode, this is a no-op.
    isolation::evaluate_isolation_for_vm(&rt.name, &rt.cfg, &inv)?;

    // Phase 5: execute the plan and record original driver bindings so
    // we can restore them during shutdown.
    execute_plan(&plan, &inv, &mut rt.vfio_transitions)?;

    // Phase 6: verification — ensure all configured passthrough devices
    // are now bound to vfio-pci.
    verify_vm_vfio_bindings(&rt.name, &rt.cfg)?;

    Ok(())
}

/// Public helper: dry-run restore for diagnostics / tooling.
///
/// This will run the full restore sequencing and outcome tracking but
/// will NOT touch sysfs. Intended for future CLI/daemon integrations.
pub fn simulate_restore_pci_devices_for_vm(rt: &VmRuntime) -> Result<RestoreSummary> {
    restore_impl(rt, RestoreMode::DryRun)
}

/// Restore PCI devices for this VM back to their original driver
/// bindings based on the recorded VFIO transitions in VmRuntime.
///
/// This is best-effort: errors are logged but do not abort the VM
/// shutdown path. The philosophy is that teardown should not panic if
/// restoration is imperfect; operators can inspect logs and repair.
///
/// This is the Phase 7 "Live" path used by the state machine.
pub fn restore_pci_devices_for_vm(rt: &VmRuntime) -> Result<()> {
    let summary = restore_impl(rt, RestoreMode::Live)?;

    let restored = summary.restored_count();
    let failed = summary.failed_count();
    let total = summary.outcomes.len();

    info!(
        vm = rt.name.as_str(),
        mode = ?summary.mode,
        total_devices = total,
        restored_devices = restored,
        failed_devices = failed,
        "vfio: PCI restore summary"
    );

    Ok(())
}

/// Internal implementation shared by Live + DryRun modes.
fn restore_impl(rt: &VmRuntime, mode: RestoreMode) -> Result<RestoreSummary> {
    let mut summary = RestoreSummary {
        vm_name: rt.name.clone(),
        mode,
        outcomes: Vec::new(),
    };

    if rt.vfio_transitions.is_empty() {
        info!(
            vm = rt.name.as_str(),
            "vfio: no recorded VFIO transitions; skipping PCI restore"
        );
        return Ok(summary);
    }

    let inv = match PciInventory::scan() {
        Ok(i) => i,
        Err(e) => {
            // Preserve existing semantics: best-effort, do not abort
            // shutdown if inventory cannot be read at restore time.
            warn!(
                vm = rt.name.as_str(),
                error = ?e,
                "vfio: failed to re-scan PCI inventory during restore; \
                 skipping PCI restore but continuing shutdown"
            );
            return Ok(summary);
        }
    };

    // First, classify transitions by device kind using the *current*
    // inventory to determine class (GPU, NVMe, NIC, USB, misc).
    let mut gpu: Vec<(&crate::model::VfioTransition, &PciFunction)> = Vec::new();
    let mut nvme: Vec<(&crate::model::VfioTransition, &PciFunction)> = Vec::new();
    let mut nic: Vec<(&crate::model::VfioTransition, &PciFunction)> = Vec::new();
    let mut usb: Vec<(&crate::model::VfioTransition, &PciFunction)> = Vec::new();
    let mut misc: Vec<(&crate::model::VfioTransition, &PciFunction)> = Vec::new();
    let mut missing: Vec<&crate::model::VfioTransition> = Vec::new();

    for t in &rt.vfio_transitions {
        match inv.find_by_bdf(&t.bdf) {
            Some(func) => {
                if func.is_display_controller() {
                    gpu.push((t, func));
                } else if func.is_nvme() {
                    nvme.push((t, func));
                } else if func.is_network_controller() {
                    nic.push((t, func));
                } else if func.is_usb_controller() {
                    usb.push((t, func));
                } else {
                    misc.push((t, func));
                }
            }
            None => {
                missing.push(t);
            }
        }
    }

    // Handle devices that disappeared between staging and shutdown.
    for t in missing {
        warn!(
            vm = rt.name.as_str(),
            bdf = t.bdf.as_str(),
            "vfio: PCI device from VFIO transition not found in inventory; \
             cannot restore driver binding"
        );

        summary.outcomes.push(RestoreOutcome {
            bdf: t.bdf.clone(),
            original_driver: t.from_driver.clone(),
            target_driver: t.from_driver.clone(),
            iommu_group: t.iommu_group,
            result: RestoreResultKind::SkippedDeviceMissing,
            reason: Some(
                "device from VFIO transition not found in inventory at restore time".to_string(),
            ),
        });
    }

    // Class-aware restore ordering: GPU → NVMe → NIC → USB → misc.
    restore_group(rt, "GPU", true, &gpu, mode, &mut summary);
    restore_group(rt, "NVMe", false, &nvme, mode, &mut summary);
    restore_group(rt, "NIC", false, &nic, mode, &mut summary);
    restore_group(rt, "USB", false, &usb, mode, &mut summary);
    restore_group(rt, "PCI", false, &misc, mode, &mut summary);

    // Optional automatic PCI bus rescan after restore. This is enabled
    // by default; failures are logged but do not abort shutdown.
    if matches!(mode, RestoreMode::Live) {
        info!(
            vm = rt.name.as_str(),
            "vfio: triggering PCI bus rescan after driver restore"
        );
        if let Err(e) = crate::pci::rescan_pci_bus() {
            warn!(
                vm = rt.name.as_str(),
                error = ?e,
                "vfio: PCI bus rescan after restore failed; continuing shutdown"
            );
        }
    }

    Ok(summary)
}

/// Restore a single device group (GPU, NVMe, NIC, USB, misc) according
/// to the selected restore mode.
fn restore_group(
    rt: &VmRuntime,
    kind_label: &str,
    is_gpu_group: bool,
    items: &[(&crate::model::VfioTransition, &PciFunction)],
    mode: RestoreMode,
    summary: &mut RestoreSummary,
) {
    for (t, func) in items {
        let bdf = t.bdf.as_str();

        // Normalize original driver string minimally (trim whitespace)
        // but preserve case to match sysfs driver directory names.
        let original_driver_opt = t
            .from_driver
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let iommu_group = t.iommu_group.or(func.iommu_group);

        if original_driver_opt.is_none() {
            info!(
                vm = rt.name.as_str(),
                bdf = bdf,
                kind = kind_label,
                "vfio: original driver was None; leaving device unbound"
            );

            summary.outcomes.push(RestoreOutcome {
                bdf: bdf.to_string(),
                original_driver: None,
                target_driver: None,
                iommu_group,
                result: RestoreResultKind::SkippedOriginallyUnbound,
                reason: Some("original driver was None; leaving device unbound".to_string()),
            });

            continue;
        }

        let original_driver = original_driver_opt.unwrap();

        if original_driver == "vfio-pci" {
            // Device was already vfio-bound before staging; no restore.
            info!(
                vm = rt.name.as_str(),
                bdf = bdf,
                kind = kind_label,
                "vfio: device was originally bound to vfio-pci; no restore needed"
            );

            summary.outcomes.push(RestoreOutcome {
                bdf: bdf.to_string(),
                original_driver: Some(original_driver.clone()),
                target_driver: Some(original_driver.clone()),
                iommu_group,
                result: RestoreResultKind::SkippedOriginallyVfio,
                reason: Some(
                    "device was originally bound to vfio-pci; no restore needed".to_string(),
                ),
            });

            continue;
        }

        if is_gpu_group {
            maybe_vendor_reset_pre(&rt.name, func);
        }

        match mode {
            RestoreMode::DryRun => {
                info!(
                    vm = rt.name.as_str(),
                    bdf = bdf,
                    driver = original_driver.as_str(),
                    kind = kind_label,
                    "vfio: [dry-run] would restore PCI device driver binding"
                );

                summary.outcomes.push(RestoreOutcome {
                    bdf: bdf.to_string(),
                    original_driver: Some(original_driver.clone()),
                    target_driver: Some(original_driver.clone()),
                    iommu_group,
                    result: RestoreResultKind::DryRunPlanned,
                    reason: Some("dry-run: no sysfs writes performed".to_string()),
                });
            }
            RestoreMode::Live => {
                info!(
                    vm = rt.name.as_str(),
                    bdf = bdf,
                    driver = original_driver.as_str(),
                    kind = kind_label,
                    "vfio: restoring PCI device driver binding"
                );

                let res = func.bind_to_driver(&original_driver);

                match res {
                    Ok(()) => {
                        if is_gpu_group {
                            maybe_vendor_reset_post(&rt.name, func);
                        }

                        summary.outcomes.push(RestoreOutcome {
                            bdf: bdf.to_string(),
                            original_driver: Some(original_driver.clone()),
                            target_driver: Some(original_driver.clone()),
                            iommu_group,
                            result: RestoreResultKind::Succeeded,
                            reason: None,
                        });
                    }
                    Err(e) => {
                        warn!(
                            vm = rt.name.as_str(),
                            bdf = bdf,
                            driver = original_driver.as_str(),
                            error = ?e,
                            "vfio: failed to restore driver binding; continuing shutdown"
                        );

                        summary.outcomes.push(RestoreOutcome {
                            bdf: bdf.to_string(),
                            original_driver: Some(original_driver.clone()),
                            target_driver: Some(original_driver.clone()),
                            iommu_group,
                            result: RestoreResultKind::Failed,
                            reason: Some(format!("failed to restore driver: {e}")),
                        });
                    }
                }
            }
        }
    }
}

/// Placeholder hook for GPU vendor-reset integration.
///
/// Currently this is a no-op that simply logs at debug level. Phase 8
/// can wire in vendor-specific reset logic here.
fn maybe_vendor_reset_pre(vm_name: &str, func: &PciFunction) {
    debug!(
        vm = vm_name,
        bdf = func.bdf.as_str(),
        "vfio: [hook] vendor-reset pre-restore (no-op placeholder)"
    );
}

/// Placeholder hook for GPU vendor-reset integration (post-restore).
fn maybe_vendor_reset_post(vm_name: &str, func: &PciFunction) {
    debug!(
        vm = vm_name,
        bdf = func.bdf.as_str(),
        "vfio: [hook] vendor-reset post-restore (no-op placeholder)"
    );
}
