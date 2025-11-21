//! VFIO orchestration layer
//!
//! This module is responsible for turning PCI inventory + VM config into
//! a concrete sequence of VFIO actions (unbind from current driver,
//! bind to vfio-pci) and executing them in a deterministic order,
//! followed by a verification pass to ensure all configured passthrough
//! devices are actually vfio-bound.
//!
//! High-level entrypoint for the state machine:
//!   vfio::stage_pci_devices_for_vm(&VmRuntime)

mod execute;
mod plan;
mod verify;

pub use execute::execute_plan;
pub use plan::{build_plan_for_vm, VfioAction, VfioActionKind, VfioPlan};
pub use verify::verify_vm_vfio_bindings;

use crate::errors::Result;
use crate::model::VmRuntime;
use crate::pci::PciInventory;
use tracing::info;

/// Stage all PCI devices required by this VM for passthrough:
///
/// 1. Build a VFIO action plan from VM config + PCI inventory.
/// 2. Execute the plan (unbind current drivers, bind to vfio-pci).
/// 3. Re-scan PCI inventory and verify that all configured passthrough
///    devices are now bound to vfio-pci.
///
/// This function is called from VmState::PreparePci.
pub fn stage_pci_devices_for_vm(rt: &VmRuntime) -> Result<()> {
    // Phase 5: plan + execute.
    let inv = PciInventory::scan()?;
    let plan = build_plan_for_vm(&rt.name, &rt.cfg, &inv)?;

    info!(
        vm = rt.name.as_str(),
        actions = plan.actions.len(),
        "vfio: staging PCI devices for VM"
    );

    execute_plan(&plan, &inv)?;

    // Phase 6: verification — ensure all configured passthrough devices
    // are now bound to vfio-pci.
    verify_vm_vfio_bindings(&rt.name, &rt.cfg)?;

    Ok(())
}
