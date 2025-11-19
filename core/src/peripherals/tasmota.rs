use tracing::info;
use crate::errors::{Result, ChalybsError};
use crate::config::TasmotaConfig;
use crate::model::VmRuntime;
use super::PeripheralHook;

/// Very simple blocking HTTP hook for Tasmota relay.
pub struct TasmotaHook {
    cfg: TasmotaConfig,
}

impl TasmotaHook {
    pub fn new(cfg: TasmotaConfig) -> Self {
        Self { cfg }
    }

    fn send_command(&self, command: &str) -> Result<()> {
        let url = format!("{}?cmnd={}", self.cfg.url, urlencoding::encode(command));
        let res = reqwest::blocking::get(&url)
            .map_err(|e| ChalybsError::Peripheral(format!("tasmota request error: {e}")))?;

        if !res.status().is_success() {
            return Err(ChalybsError::Peripheral(format!(
                "tasmota responded with status {}",
                res.status()
            )));
        }

        Ok(())
    }
}

impl PeripheralHook for TasmotaHook {
    fn vm_up(&self, _rt: &VmRuntime) -> Result<()> {
        info!("Tasmota: VM up → power on KVM relay");
        self.send_command(&self.cfg.on_command)
    }

    fn vm_down(&self, _rt: &VmRuntime) -> Result<()> {
        info!("Tasmota: VM down → power off KVM relay");
        self.send_command(&self.cfg.off_command)
    }
}
