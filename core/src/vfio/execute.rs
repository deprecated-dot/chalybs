use tracing::info;

use crate::errors::{ChalybsError, Result};
use crate::pci::PciInventory;

use super::{VfioActionKind, VfioPlan};

/// Execute a previously built VFIO action plan.
///
/// This performs the actual sysfs writes by delegating to the
/// PciFunction helpers in `pci.rs`.
pub fn execute_plan(plan: &VfioPlan, inv: &PciInventory) -> Result<()> {
    for action in &plan.actions {
        let func = inv.find_by_bdf(&action.bdf).ok_or_else(|| {
            ChalybsError::Vfio(format!(
                "VFIO plan references unknown PCI device {}",
                action.bdf
            ))
        })?;

        match action.kind {
            VfioActionKind::UnbindFromCurrentDriver => {
                info!(
                    vm = plan.vm_name.as_str(),
                    bdf = action.bdf.as_str(),
                    reason = action.reason.as_str(),
                    "vfio: unbinding device from current driver"
                );
                func.unbind_current_driver()?;
            }
            VfioActionKind::BindToVfio => {
                info!(
                    vm = plan.vm_name.as_str(),
                    bdf = action.bdf.as_str(),
                    reason = action.reason.as_str(),
                    "vfio: binding device to vfio-pci"
                );
                func.bind_to_vfio_pci()?;
            }
        }
    }

    Ok(())
}
