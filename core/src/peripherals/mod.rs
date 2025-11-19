use crate::errors::Result;
use crate::model::VmRuntime;

pub mod tasmota;
pub mod ddc;
pub mod looking_glass;

pub trait PeripheralHook {
    fn vm_up(&self, rt: &VmRuntime) -> Result<()>;
    fn vm_down(&self, rt: &VmRuntime) -> Result<()>;
}

pub fn apply_vm_up(rt: &VmRuntime) -> Result<()> {
    if let Some(ref cfg) = rt.cfg.peripherals {
        if let Some(ref t) = cfg.tasmota {
            tasmota::TasmotaHook::new(t.clone()).vm_up(rt)?;
        }
        if let Some(ref d) = cfg.ddc {
            ddc::DdcHook::new(d.clone()).vm_up(rt)?;
        }
        if let Some(ref lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg.clone()).vm_up(rt)?;
        }
    }
    Ok(())
}

pub fn apply_vm_down(rt: &VmRuntime) -> Result<()> {
    if let Some(ref cfg) = rt.cfg.peripherals {
        if let Some(ref t) = cfg.tasmota {
            tasmota::TasmotaHook::new(t.clone()).vm_down(rt)?;
        }
        if let Some(ref d) = cfg.ddc {
            ddc::DdcHook::new(d.clone()).vm_down(rt)?;
        }
        if let Some(ref lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg.clone()).vm_down(rt)?;
        }
    }
    Ok(())
}
