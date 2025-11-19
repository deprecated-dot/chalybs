use tracing::info;
use crate::errors::Result;
use crate::config::DdcConfig;
use crate::model::VmRuntime;
use super::PeripheralHook;

/// Stub DDC/CI implementation.
/// Replace with real i2c/ddcset invocation or bindings later.
pub struct DdcHook {
    cfg: DdcConfig,
}

impl DdcHook {
    pub fn new(cfg: DdcConfig) -> Self {
        Self { cfg }
    }

    fn set_input(&self, _input: u8) -> Result<()> {
        info!(
            bus = self.cfg.monitor_i2c_bus,
            input = _input,
            "DDC: switching monitor input (stub)"
        );
        Ok(())
    }
}

impl PeripheralHook for DdcHook {
    fn vm_up(&self, _rt: &VmRuntime) -> Result<()> {
        self.set_input(self.cfg.vm_input)
    }

    fn vm_down(&self, _rt: &VmRuntime) -> Result<()> {
        self.set_input(self.cfg.host_input)
    }
}
