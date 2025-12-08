use super::PeripheralHook;
use crate::config::SpiceConfig;
use crate::errors::Result;
use crate::model::VmRuntime;
use tracing::info;

/// SPICE peripheral hook.
///
/// This mirrors the existing peripheral model:
///   - Config lives under [vm.<name>.peripherals.spice].
///   - QEMU wiring is handled by the core QEMU builder.
///   - Hook is for VM lifecycle events / logging only.
pub struct SpiceHook {
    cfg: SpiceConfig,
}

impl SpiceHook {
    pub fn new(cfg: SpiceConfig) -> Self {
        Self { cfg }
    }
}

impl PeripheralHook for SpiceHook {
    fn vm_up(&self, rt: &mut VmRuntime) -> Result<()> {
        if !self.cfg.enabled {
            info!(
                vm = %rt.name,
                "spice: peripheral present but disabled; skipping vm_up"
            );
            return Ok(());
        }

        info!(
            vm = %rt.name,
            port = self.cfg.port,
            addr = %self.cfg.addr,
            "spice: VM up; SPICE peripheral enabled"
        );
        Ok(())
    }

    fn vm_down(&self, rt: &mut VmRuntime) -> Result<()> {
        if !self.cfg.enabled {
            info!(
                vm = %rt.name,
                "spice: peripheral present but disabled; skipping vm_down"
            );
            return Ok(());
        }

        info!(
            vm = %rt.name,
            port = self.cfg.port,
            addr = %self.cfg.addr,
            "spice: VM down; SPICE peripheral enabled"
        );
        Ok(())
    }
}
