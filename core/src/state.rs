use tracing::{info, instrument};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Init,
    Validate,
    PrepareHugepages,
    PreparePci,
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

    // ---------------------------------------------------------------------
    // Segmented bring-up: one step towards Steady
    // ---------------------------------------------------------------------

    /// Advance the VM state machine by at most one state towards `Steady`.
    ///
    /// This executes a single state's work (e.g. Validate, PrepareHugepages,
    /// PreparePci, ReserveCpus, ...) and then transitions to the next state.
    ///
    /// Returns the new state after this step.
    ///
    /// Daemon/TUI usage pattern:
    ///   - call `step()` once per tick for a VM that is not yet Steady
    ///   - stop when this returns `VmState::Steady`
    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn step(&mut self) -> Result<VmState> {
        match self.state {
            VmState::Init => {
                info!("state=Init");
                self.rt.push_system("state=Init");
                self.state = VmState::Validate;
            }

            VmState::Validate => {
                info!("state=Validate");
                self.rt.push_system("state=Validate");

                // cpuset + QEMU path sanity
                crate::cpuset::preflight(&self.rt)?;
                self.rt
                    .push_info("cpuset: preflight checks completed successfully");

                crate::qemu::preflight(&self.rt)?;
                self.rt
                    .push_info("qemu: preflight checks completed successfully");

                // PCI / GPU policy preflight
                //
                // NOTE: policy lives under config::pci (consuming the
                // crate::pci inventory module), so the correct path is:
                //   crate::config::pci::preflight_gpu_policy(...)
                crate::config::pci::preflight_gpu_policy(&self.rt.name, &self.rt.cfg)?;
                self.rt
                    .push_info("pci: GPU policy preflight completed successfully");

                // First writey phase: hugepage provisioning.
                // This is intentionally placed before any VFIO or cpuset
                // mutations so that hugepage failures fail-fast without
                // leaving partially staged devices or cpusets behind.
                self.state = VmState::PrepareHugepages;
            }

            VmState::PrepareHugepages => {
                info!("state=PrepareHugepages");
                self.rt.push_system("state=PrepareHugepages");

                // Phase 12: deterministic hugepage provisioning.
                //
                // Behavior:
                //   - If qemu.hugepages = false → no-op (logs and returns Ok).
                //   - Otherwise:
                //       * pick NUMA node (config / topology)
                //       * compute required 2MiB pages from mem_mb
                //       * write node-local nr_hugepages
                //       * record outcome in VmRuntime.{hugepages_*}
                //
                // Semantics mirror the existing Bash suite; no QEMU CLI
                // changes are made here. QEMU flags remain under operator
                // control via vm.qemu.args/post_args.
                crate::hugepages::provision_for_vm(&mut self.rt)?;

                self.state = VmState::PreparePci;
            }

            VmState::PreparePci => {
                info!("state=PreparePci");
                self.rt.push_system("state=PreparePci");

                // Phase 5–7: perform VFIO staging for all PCI devices
                // that this VM wants to passthrough. This builds a VFIO
                // action plan and executes it (unbind current drivers,
                // bind to vfio-pci), recording original driver bindings
                // in VmRuntime for later restoration at shutdown.
                //
                // All safety policy is enforced earlier (e.g. single-GPU
                // checks, unbind feasibility). Any failure here aborts
                // before QEMU launch.
                crate::vfio::stage_pci_devices_for_vm(&mut self.rt)?;
                self.rt
                    .push_info("vfio: PCI staging completed for VM passthrough devices");

                self.state = VmState::ReserveCpus;
            }

            VmState::ReserveCpus => {
                info!("state=ReserveCpus");
                self.rt.push_system("state=ReserveCpus");

                // Phase 10 (CPU): reserve CPUs and create cpuset hierarchy.
                // This is now routed through the Phase 10 CPU orchestrator
                // to keep state-machine call sites stable as CPU handling
                // evolves.
                crate::cpu::reserve_cpus(&mut self.rt)?;
                self.rt
                    .push_info("cpuset: VM/host cpuset hierarchy created");

                self.state = VmState::LaunchQemu;
            }

            VmState::LaunchQemu => {
                info!("state=LaunchQemu");
                self.rt.push_system("state=LaunchQemu");

                crate::qemu::launch(&mut self.rt)?;

                if let Some(qemu) = self.rt.qemu.as_ref() {
                    self.rt
                        .push_info(format!("qemu: launched with pid {}", qemu.pid));
                } else {
                    // This should not normally happen; log as a semantic error
                    // without changing control flow.
                    self.rt
                        .push_warning("qemu: launch completed but QemuState pid not recorded");
                }

                self.state = VmState::DetectThreads;
            }

            VmState::DetectThreads => {
                info!("state=DetectThreads");
                self.rt.push_system("state=DetectThreads");

                // Phase 10 (CPU): wait for QEMU threads via orchestrator.
                crate::cpu::wait_for_qemu_threads(&self.rt)?;
                self.rt
                    .push_info("affinity: QEMU threads discovered for pinning");

                self.state = VmState::PinVcpus;
            }

            VmState::PinVcpus => {
                info!("state=PinVcpus");
                self.rt.push_system("state=PinVcpus");

                // Phase 10 (CPU): pin vCPU threads via orchestrator.
                crate::cpu::pin_vcpus(&self.rt)?;
                self.rt.pinned_threads = true;
                self.rt
                    .push_info("affinity: vCPU threads pinned to VM cpuset");

                self.state = VmState::DetectMsi;
            }

            VmState::DetectMsi => {
                info!("state=DetectMsi");
                self.rt.push_system("state=DetectMsi");

                // Phase 11 (IRQ): spawn background worker that will
                // deterministically hook MSI/MSI-X once the guest driver
                // brings them up, without blocking VM bring-up here.
                crate::irq::spawn_irq_pin_worker(&self.rt)?;
                self.rt.push_info(
                    "irq: background worker spawned for MSI/MSI-X detection and IRQ pinning",
                );

                self.state = VmState::PinIrqs;
            }

            VmState::PinIrqs => {
                info!("state=PinIrqs");
                self.rt.push_system("state=PinIrqs");

                // IRQ pinning itself is now handled asynchronously by the
                // worker started in DetectMsi. From the state machine's
                // perspective, pinning has been requested and we can
                // proceed with the remaining bring-up.
                self.rt.pinned_irqs = true;
                self.rt
                    .push_info("irq: IRQ pinning delegated to background worker");

                self.state = VmState::PeripheralHooks;
            }

            VmState::PeripheralHooks => {
                info!("state=PeripheralHooks");
                self.rt.push_system("state=PeripheralHooks");

                crate::peripherals::apply_vm_up(&self.rt)?;
                self.rt
                    .push_info("peripherals: VM up hooks applied successfully");

                self.state = VmState::Steady;
            }

            VmState::Steady => {
                // Idempotent: nothing further to do on bring-up.
                info!("VM already in steady-state");
                self.rt.push_system("state=Steady");
                self.rt
                    .push_info("vm: already in steady-state (all bring-up phases complete)");
            }

            // Shutdown / Cleanup / Idle are not valid targets for the
            // bring-up `step()` path.
            s @ VmState::Shutdown | s @ VmState::Cleanup | s @ VmState::Idle => {
                return Err(ChalybsError::State(format!(
                    "step() called in invalid bring-up state: {s:?}"
                )));
            }
        }

        Ok(self.state)
    }

    // ---------------------------------------------------------------------
    // Backwards-compatible blocking bring-up
    // ---------------------------------------------------------------------

    /// Blocking wrapper: advance the VM through all bring-up states
    /// until it reaches `Steady` or an error occurs.
    ///
    /// This preserves the original synchronous semantics while internally
    /// using the segmented `step()` API.
    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn run_until_steady(&mut self) -> Result<()> {
        loop {
            match self.state {
                VmState::Steady => {
                    info!("VM reached steady-state");
                    self.rt.push_system("state=Steady");
                    self.rt
                        .push_info("vm: reached steady-state (all bring-up phases complete)");
                    return Ok(());
                }
                VmState::Shutdown | VmState::Cleanup | VmState::Idle => {
                    return Err(ChalybsError::State(format!(
                        "run_until_steady() called in invalid state: {:?}",
                        self.state
                    )));
                }
                _ => {
                    self.step()?;
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // Segmented shutdown: one step towards Idle
    // ---------------------------------------------------------------------

    /// Advance the shutdown sequence by at most one step towards `Idle`.
    ///
    /// Semantics:
    ///   - From Steady / PinIrqs / PeripheralHooks → enter Shutdown
    ///   - In Shutdown → request QEMU shutdown, then transition to Cleanup
    ///   - In Cleanup  → restore PCI drivers + destroy cpusets → Idle
    ///   - In Idle     → no-op
    ///
    /// Returns the new state after this step.
    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn step_shutdown(&mut self) -> Result<VmState> {
        match self.state {
            // If we’re in Steady or close to it, transition into Shutdown.
            VmState::Steady | VmState::PinIrqs | VmState::PeripheralHooks => {
                info!("state=Shutdown");
                self.rt.push_system("state=Shutdown");

                crate::qemu::shutdown(&mut self.rt)?;
                self.rt
                    .push_info("qemu: shutdown requested for VM instance");

                // After requesting shutdown, we model the next step as
                // Shutdown (restore PCI + cpuset cleanup in next arm).
                self.state = VmState::Shutdown;
            }

            VmState::Shutdown => {
                // Phase 7: restore PCI driver bindings for all devices that were
                // staged to vfio-pci for this VM. This is best-effort: failures
                // are logged but do not abort shutdown.
                info!("state=RestorePci");
                self.rt
                    .push_system("state=RestorePci (restore PCI driver bindings)");
                crate::vfio::restore_pci_devices_for_vm(&self.rt)?;
                self.rt
                    .push_info("vfio: restore of PCI driver bindings requested for VM devices");

                info!("state=Cleanup");
                self.rt.push_system("state=Cleanup");

                // Phase 10 (CPU): cleanup cpuset hierarchy via orchestrator.
                crate::cpu::cleanup_cpus(&mut self.rt)?;
                self.rt
                    .push_info("cpuset: VM/host cpuset hierarchy cleaned up");

                self.state = VmState::Cleanup;
            }

            VmState::Cleanup => {
                // Final transition to Idle; no additional work beyond what was
                // done in the previous Cleanup step.
                self.state = VmState::Idle;
                self.rt
                    .push_system("state=Idle (VM shutdown sequence completed)");
            }

            VmState::Idle => {
                // Idempotent: already fully shut down.
                info!("VM already in Idle; shutdown step is a no-op");
                self.rt
                    .push_system("state=Idle (shutdown already completed)");
            }

            // For earlier bring-up states, we attempt a best-effort shutdown by
            // transitioning into Shutdown and performing the QEMU shutdown path.
            other => {
                info!("state=Shutdown (from {:?})", other);
                self.rt
                    .push_system(format!("state=Shutdown (from {:?})", other));

                crate::qemu::shutdown(&mut self.rt)?;
                self.rt
                    .push_info("qemu: shutdown requested for VM instance (from non-steady state)");

                self.state = VmState::Shutdown;
            }
        }

        Ok(self.state)
    }

    // ---------------------------------------------------------------------
    // Backwards-compatible blocking shutdown
    // ---------------------------------------------------------------------

    /// Blocking wrapper: advance the VM through shutdown until it
    /// reaches `Idle` or an error occurs.
    ///
    /// This preserves the original synchronous semantics while internally
    /// using the segmented `step_shutdown()` API.
    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn run_shutdown(&mut self) -> Result<()> {
        loop {
            match self.state {
                VmState::Idle => {
                    // Already fully shut down.
                    return Ok(());
                }
                _ => {
                    self.step_shutdown()?;
                    if self.state == VmState::Idle {
                        return Ok(());
                    }
                }
            }
        }
    }
}
