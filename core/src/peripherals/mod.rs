use crate::errors::Result;
use crate::model::VmRuntime;

pub mod ddc;
pub mod looking_glass;
pub mod tasmota;

pub trait PeripheralHook {
    fn vm_up(&self, rt: &mut VmRuntime) -> Result<()>;
    fn vm_down(&self, rt: &mut VmRuntime) -> Result<()>;
}

pub fn apply_vm_up(rt: &mut VmRuntime) -> Result<()> {
    // FIX: eliminate immutable borrow of rt.cfg.peripherals
    let periph_cfg = rt.cfg.peripherals.clone();

    if let Some(cfg) = periph_cfg {
        if let Some(t) = cfg.tasmota {
            tasmota::TasmotaHook::new(t).vm_up(rt)?;
        }
        if let Some(d) = cfg.ddc {
            ddc::DdcHook::new(d).vm_up(rt)?;
        }
        if let Some(lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg).vm_up(rt)?;
        }
    }
    Ok(())
}

pub fn apply_vm_down(rt: &mut VmRuntime) -> Result<()> {
    // FIX: eliminate immutable borrow of rt.cfg.peripherals
    let periph_cfg = rt.cfg.peripherals.clone();

    if let Some(cfg) = periph_cfg {
        if let Some(t) = cfg.tasmota {
            tasmota::TasmotaHook::new(t).vm_down(rt)?;
        }
        if let Some(d) = cfg.ddc {
            ddc::DdcHook::new(d).vm_down(rt)?;
        }
        if let Some(lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg).vm_down(rt)?;
        }
    }
    Ok(())
}
