use tracing::info;

use crate::errors::{ChalybsError, Result};
use crate::model::VfioTransition;
use crate::pci::PciInventory;

use super::{VfioActionKind, VfioPlan};

/// Execute a previously built VFIO action plan.
///
/// This performs the actual sysfs writes by delegating to the
/// PciFunction helpers in `pci.rs`. Any driver transitions that would
/// need to be restored later are recorded into `transitions`.
pub fn execute_plan(
    plan: &VfioPlan,
    inv: &PciInventory,
    transitions: &mut Vec<VfioTransition>,
) -> Result<()> {
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

                // Record the original driver binding *before* we bind to
                // vfio-pci, so that shutdown can restore it later.
                //
                // If the device is already bound to vfio-pci in this
                // inventory snapshot, there is nothing to restore.
                if !matches!(func.driver.as_deref(), Some("vfio-pci")) {
                    transitions.push(VfioTransition {
                        bdf: action.bdf.clone(),
                        from_driver: func.driver.clone(),
                        iommu_group: func.iommu_group,
                    });
                }

                func.bind_to_vfio_pci()?;
            }
        }
    }

    Ok(())
}

