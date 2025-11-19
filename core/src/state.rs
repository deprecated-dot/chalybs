use tracing::{info, instrument};

use crate::errors::{Result, ChalybsError};
use crate::model::VmRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Init,
    Validate,
    ReserveCpus,
    LaunchQemu,
    DetectThreads,
    PinVcpus,
    DetectMsi,
    PinIrqs,
    PeripheralHooks,
    Steady,
    Shutdown,
    Cleanup,
    Idle,
}

pub struct VmStateMachine {
    pub state: VmState,
    pub rt: VmRuntime,
}

impl VmStateMachine {
    pub fn new(rt: VmRuntime) -> Self {
        Self {
            state: VmState::Init,
            rt,
        }
    }

    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn run_until_steady(&mut self) -> Result<()> {
        loop {
            match self.state {
                VmState::Init => {
                    info!("state=Init");
                    self.state = VmState::Validate;
                }

                VmState::Validate => {
                    info!("state=Validate");
                    crate::cpuset::preflight(&self.rt)?;
                    crate::qemu::preflight(&self.rt)?;
                    self.state = VmState::ReserveCpus;
                }

                VmState::ReserveCpus => {
                    info!("state=ReserveCpus");
                    crate::cpuset::create_cpuset(&mut self.rt)?;
                    self.state = VmState::LaunchQemu;
                }

                VmState::LaunchQemu => {
                    info!("state=LaunchQemu");
                    crate::qemu::launch(&mut self.rt)?;
                    self.state = VmState::DetectThreads;
                }

                VmState::DetectThreads => {
                    info!("state=DetectThreads");
                    crate::affinity::wait_for_qemu_threads(&self.rt)?;
                    self.state = VmState::PinVcpus;
                }

                VmState::PinVcpus => {
                    info!("state=PinVcpus");
                    crate::affinity::pin_vcpus(&self.rt)?;
                    self.rt.pinned_threads = true;
                    self.state = VmState::DetectMsi;
                }

                VmState::DetectMsi => {
                    info!("state=DetectMsi");
                    crate::irq::wait_for_msi(&self.rt)?;
                    self.state = VmState::PinIrqs;
                }

                VmState::PinIrqs => {
                    info!("state=PinIrqs");
                    crate::irq::pin_irqs(&self.rt)?;
                    self.rt.pinned_irqs = true;
                    self.state = VmState::PeripheralHooks;
                }

                VmState::PeripheralHooks => {
                    info!("state=PeripheralHooks");
                    crate::peripherals::apply_vm_up(&self.rt)?;
                    self.state = VmState::Steady;
                }

                VmState::Steady => {
                    info!("VM reached steady-state");
                    return Ok(());
                }

                s => {
                    return Err(ChalybsError::State(format!(
                        "unexpected state in run_until_steady(): {s:?}"
                    )));
                }
            }
        }
    }

    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn run_shutdown(&mut self) -> Result<()> {
        match self.state {
            VmState::Steady | VmState::PinIrqs | VmState::PeripheralHooks => {
                self.state = VmState::Shutdown;
            }
            _ => {}
        }

        info!("state=Shutdown");
        crate::qemu::shutdown(&mut self.rt)?;

        info!("state=Cleanup");
        crate::cpuset::destroy_cpuset(&mut self.rt)?;

        self.state = VmState::Idle;
        Ok(())
    }
}
