use super::PeripheralHook;
use crate::config::LookingGlassConfig;
use crate::errors::Result;
use crate::model::VmRuntime;
use tracing::info;

/// Stub for Looking Glass hooks.
/// In future: ensure shm exists, permissions are correct, etc.
pub struct LgHook {
    cfg: LookingGlassConfig,
}

impl LgHook {
    pub fn new(cfg: LookingGlassConfig) -> Self {
        Self { cfg }
    }
}

impl PeripheralHook for LgHook {
    fn vm_up(&self, _rt: &mut VmRuntime) -> Result<()> {
        info!(shm = %self.cfg.shm_name, "Looking Glass VM up (stub)");
        Ok(())
    }

    fn vm_down(&self, _rt: &mut VmRuntime) -> Result<()> {
        info!(shm = %self.cfg.shm_name, "Looking Glass VM down (stub)");
        Ok(())
    }
}
