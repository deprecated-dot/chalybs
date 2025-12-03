use std::process::Command;

use tracing::{info, warn};

use crate::config::DdcConfig;
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

use super::PeripheralHook;

/// DDC/CI implementation using the `ddcutil` CLI.
///
/// This assumes that `ddcutil` is installed and available in PATH.
/// We use VCP code 0x60 (input source) and treat the configured
/// `vm_input` / `host_input` values as the raw DDC values to send.
pub struct DdcHook {
    cfg: DdcConfig,
}

impl DdcHook {
    pub fn new(cfg: DdcConfig) -> Self {
        Self { cfg }
    }

    fn set_input(&self, input: u8) -> Result<()> {
        info!(
            bus = self.cfg.monitor_i2c_bus,
            input, "DDC: switching monitor input via ddcutil"
        );

        let bus_str = self.cfg.monitor_i2c_bus.to_string();

        let status = Command::new("ddcutil")
            .arg("--bus")
            .arg(&bus_str)
            .arg("setvcp")
            .arg("0x60")
            .arg(format!("0x{:02x}", input))
            .status()
            .map_err(|e| {
                ChalybsError::Peripheral(format!("ddcutil: failed to spawn process: {e}"))
            })?;

        if !status.success() {
            let msg = format!("ddcutil: setvcp failed with status {status}");
            if self.cfg.fatal_on_error {
                return Err(ChalybsError::Peripheral(msg));
            } else {
                warn!("{msg}");
            }
        }

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
