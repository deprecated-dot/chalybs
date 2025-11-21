//! VFIO orchestration layer
//!
//! This module is responsible for turning PCI inventory + VM config into
//! a concrete sequence of VFIO actions (unbind from current driver,
//! bind to vfio-pci) and executing them in a deterministic order.
//!
//! High-level entrypoint for the state machine:
//!   vfio::stage_pci_devices_for_vm(&VmRuntime)

mod plan;
mod execute;

pub use plan::{VfioAction, VfioActionKind, VfioPlan, build_plan_for_vm};
pub use execute::execute_plan;

use crate::errors::Result;
use crate::model::VmRuntime;
use crate::pci::PciInventory;
use tracing::info;

/// Stage all PCI devices required by this VM for passthrough:
///
/// 1. Build a VFIO action plan from VM config + PCI inventory.
/// 2. Execute the plan in a safe, ordered fashion.
/// 3. Abort before QEMU launch if anything fails.
///
/// This function is called from VmState::PreparePci.
pub fn stage_pci_devices_for_vm(rt: &VmRuntime) -> Result<()> {
    let inv = PciInventory::scan()?;
    let plan = build_plan_for_vm(&rt.name, &rt.cfg, &inv)?;

    info!(
        vm = rt.name.as_str(),
        actions = plan.actions.len(),
        "vfio: staging PCI devices for VM"
    );

    execute_plan(&plan, &inv)
}
