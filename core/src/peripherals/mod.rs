use crate::errors::Result;
use crate::model::VmRuntime;

pub mod ddc;
pub mod looking_glass;
pub mod spice;
pub mod tasmota;

pub trait PeripheralHook {
    fn vm_up(&self, rt: &mut VmRuntime) -> Result<()>;
    fn vm_down(&self, rt: &mut VmRuntime) -> Result<()>;
}

pub fn apply_vm_up(rt: &mut VmRuntime) -> Result<()> {
    // FIX: eliminate immutable borrow of rt.cfg.peripherals
    let periph_cfg = rt.cfg.peripherals.clone();

    if let Some(cfg) = periph_cfg {
        // ---------------------------------------------------------------------
        // Tasmota and Looking Glass always run immediately (same behavior).
        // SPICE follows the same pattern: immediate wiring, no gating.
        // ---------------------------------------------------------------------
        if let Some(t) = cfg.tasmota {
            tasmota::TasmotaHook::new(t).vm_up(rt)?;
        }

        // ---------------------------------------------------------------------
        // DDC firing is *gated* on IRQ pinning being complete.
        //
        // This is the only change:
        //   - No timers
        //   - No heuristics
        //   - No polling loops
        //   - No changes to IRQ worker
        //
        // Once irq_pinning_complete == true, DDC fires exactly once.
        // ---------------------------------------------------------------------
        if let Some(d) = cfg.ddc {
            if rt
                .irq_pinning_complete
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                ddc::DdcHook::new(d).vm_up(rt)?;
            } else {
                // We simply skip for now — deterministic do-nothing.
                // Daemon/TUI can call apply_vm_up() again when the flag flips.
                rt.push_info("peripherals/ddc: IRQ pinning not yet complete; deferring DDC vm_up");
            }
        }

        if let Some(lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg).vm_up(rt)?;
        }

        if let Some(s) = cfg.spice {
            spice::SpiceHook::new(s).vm_up(rt)?;
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

        // Downhook is unconditional — same semantics as before.
        if let Some(d) = cfg.ddc {
            ddc::DdcHook::new(d).vm_down(rt)?;
        }

        if let Some(lg) = cfg.looking_glass {
            looking_glass::LgHook::new(lg).vm_down(rt)?;
        }

        if let Some(s) = cfg.spice {
            spice::SpiceHook::new(s).vm_down(rt)?;
        }
    }

    Ok(())
}
