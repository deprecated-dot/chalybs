mod status;
pub use status::cpuset_status;

use crate::errors::Result;
use crate::model::VmRuntime;

/// cpuset preflight — currently does nothing, but validates the interface
pub fn preflight(_rt: &VmRuntime) -> Result<()> {
    Ok(())
}

/// cpuset creation — (placeholder until we finalize IRQ/thread flow)
pub fn create_cpuset(_rt: &mut VmRuntime) -> Result<()> {
    Ok(())
}

/// cpuset deletion — placeholder
pub fn destroy_cpuset(_rt: &mut VmRuntime) -> Result<()> {
    Ok(())
}
