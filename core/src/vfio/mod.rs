//! VFIO orchestration layer
//!
//! This module is responsible for turning PCI inventory + VM config into
//! a concrete sequence of VFIO actions (unbind from current driver,
//! bind to vfio-pci) and executing them in a deterministic order,
//! followed by a verification pass to ensure all configured passthrough
//! devices are actually vfio-bound.
//!
//! High-level entrypoints for the state machine:
//!   - vfio::stage_pci_devices_for_vm(&mut VmRuntime)
//!   - vfio::restore_pci_devices_for_vm(&VmRuntime)

mod execute;
mod plan;
mod verify;

pub use execute::execute_plan;
pub use plan::{build_plan_for_vm, VfioAction, VfioActionKind, VfioPlan};
pub use verify::verify_vm_vfio_bindings;

use crate::errors::Result;
use crate::model::VmRuntime;
use crate::pci::PciInventory;
use tracing::{info, warn};

/// Stage all PCI devices required by this VM for passthrough:
///
/// 1. Build a VFIO action plan from VM config + PCI inventory.
/// 2. Execute the plan (unbind current drivers, bind to vfio-pci),
///    recording the original driver bindings in the VmRuntime.
/// 3. Re-scan PCI inventory and verify that all configured passthrough
///    devices are now bound to vfio-pci.
///
/// This function is called from VmState::PreparePci.
pub fn stage_pci_devices_for_vm(rt: &mut VmRuntime) -> Result<()> {
    // Phase 5: plan + execute.
    let inv = PciInventory::scan()?;
    let plan = build_plan_for_vm(&rt.name, &rt.cfg, &inv)?;

    info!(
        vm = rt.name.as_str(),
        actions = plan.actions.len(),
        "vfio: staging PCI devices for VM"
    );

    // Execute the plan and record original driver bindings so we can
    // restore them during shutdown.
    execute_plan(&plan, &inv, &mut rt.vfio_transitions)?;

    // Phase 6: verification — ensure all configured passthrough devices
    // are now bound to vfio-pci.
    verify_vm_vfio_bindings(&rt.name, &rt.cfg)?;

    Ok(())
}

/// Restore PCI devices for this VM back to their original driver
/// bindings based on the recorded VFIO transitions in VmRuntime.
///
/// This is best-effort: errors are logged but do not abort the VM
/// shutdown path. The philosophy is that teardown should not panic if
/// restoration is imperfect; operators can inspect logs and repair.
pub fn restore_pci_devices_for_vm(rt: &VmRuntime) -> Result<()> {
    if rt.vfio_transitions.is_empty() {
        info!(
            vm = rt.name.as_str(),
            "vfio: no recorded VFIO transitions; skipping PCI restore"
        );
        return Ok(());
    }

    let inv = match PciInventory::scan() {
        Ok(i) => i,
        Err(e) => {
            warn!(
                vm = rt.name.as_str(),
                error = ?e,
                "vfio: failed to re-scan PCI inventory during restore; \
                 skipping PCI restore but continuing shutdown"
            );
            return Ok(());
        }
    };

    for t in &rt.vfio_transitions {
        let func = match inv.find_by_bdf(&t.bdf) {
            Some(f) => f,
            None => {
                warn!(
                    vm = rt.name.as_str(),
                    bdf = t.bdf.as_str(),
                    "vfio: PCI device from VFIO transition not found in inventory; \
                     cannot restore driver binding"
                );
                continue;
            }
        };

        let from_driver = match &t.from_driver {
            None => {
                // Device was originally unbound; our deterministic policy
                // is to *not* bind it to anything.
                info!(
                    vm = rt.name.as_str(),
                    bdf = t.bdf.as_str(),
                    "vfio: original driver was None; leaving device unbound"
                );
                continue;
            }
            Some(d) if d == "vfio-pci" => {
                // Device was already vfio-bound before staging; no restore.
                info!(
                    vm = rt.name.as_str(),
                    bdf = t.bdf.as_str(),
                    "vfio: device was originally bound to vfio-pci; no restore needed"
                );
                continue;
            }
            Some(d) => d,
        };

        info!(
            vm = rt.name.as_str(),
            bdf = t.bdf.as_str(),
            driver = from_driver.as_str(),
            "vfio: restoring PCI device driver binding"
        );

        if let Err(e) = func.bind_to_driver(from_driver) {
            warn!(
                vm = rt.name.as_str(),
                bdf = t.bdf.as_str(),
                driver = from_driver.as_str(),
                error = ?e,
                "vfio: failed to restore driver binding; continuing shutdown"
            );
        }
    }

    Ok(())
}
