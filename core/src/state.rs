use tracing::{info, instrument};

use crate::cpuplan::{build_cpu_plan, validate_cpu_plan, CpuPlanInputs, CpuPlanValidationError};
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
                crate::config::pci::preflight_gpu_policy(&self.rt.name, &self.rt.cfg)?;
                self.rt
                    .push_info("pci: GPU policy preflight completed successfully");

                // -----------------------------------------------------------------
                // CPU plan construction + structural validation (Option B: hard error)
                //
                // This is the point where we fold:
                //   - host CPU identity (CPUID)
                //   - host NUMA topology (sysfs)
                //   - VM CPU layout (config)
                //
                // into a CpuPlan and *require* structural consistency.
                //
                // Semantics:
                //   - If CPUID or NUMA topology cannot be obtained at all
                //     (e.g. non-x86 host, missing sysfs), we log and proceed
                //     without a CpuPlan (cpu_plan remains None).
                //   - If a CpuPlan *can* be built but validation yields
                //     findings, we treat that as a deterministic configuration
                //     error and abort bring-up here.
                // -----------------------------------------------------------------

                // 1) Host CPU identity via CPUID (detect layer).
                if let Some(ident) = crate::cpu::detect::detect_cpu_identity() {
                    let arch = crate::cpu::detect::classify_cpu(&ident);

                    // 2) NUMA topology from sysfs.
                    match crate::cpu::detect::HostNumaTopology::from_sysfs() {
                        Ok(topo) => {
                            // 3) Build an immutable CpuPlan.
                            let inputs = CpuPlanInputs::new(
                                ident.clone(),
                                arch,
                                topo.clone(),
                                self.rt.cpus.clone(),
                            );
                            let plan = build_cpu_plan(inputs);

                            // 4) Validate the plan structurally.
                            let findings = validate_cpu_plan(&plan);

                            if !findings.is_empty() {
                                // Hard-error semantics: convert findings into a
                                // deterministic error and abort bring-up.
                                let mut msgs = Vec::new();

                                for f in &findings {
                                    match f {
                                        CpuPlanValidationError::HostCpuOutsideTopology { cpu } => {
                                            msgs.push(format!(
                                                "host CPU {} listed in host_cpus is not present \
                                                 in any discovered NUMA node",
                                                cpu
                                            ));
                                        }
                                        CpuPlanValidationError::VmCpuOutsideTopology { cpu } => {
                                            msgs.push(format!(
                                                "VM CPU {} listed in vm_cpus is not present in \
                                                 any discovered NUMA node",
                                                cpu
                                            ));
                                        }
                                    }
                                }

                                let msg = format!(
                                    "cpuplan: structural CPU plan validation failed; VM bring-up \
                                     aborted. Findings: {}",
                                    msgs.join("; ")
                                );

                                self.rt.push_error(msg.clone());
                                return Err(ChalybsError::State(msg));
                            }

                            // No findings: record the plan and continue.
                            self.rt.cpu_plan = Some(plan);
                            self.rt.push_info(
                                "cpuplan: host CPU identity, NUMA topology, and VM CPU layout \
                                 validated successfully",
                            );
                        }
                        Err(e) => {
                            // Topology unavailable: log and proceed without CpuPlan.
                            self.rt.push_warning(format!(
                                "cpuplan: failed to read host NUMA topology from sysfs; \
                                 skipping CPU plan construction for this VM: {e}"
                            ));
                        }
                    }
                } else {
                    // CPUID unavailable (e.g. non-x86 host): log and proceed without CpuPlan.
                    self.rt.push_warning(
                        "cpuplan: host CPU identity could not be detected via CPUID; \
                         skipping CPU plan construction for this VM",
                    );
                }

                // First writey phase: hugepage provisioning.
                self.state = VmState::PrepareHugepages;
            }

            VmState::PrepareHugepages => {
                info!("state=PrepareHugepages");
                self.rt.push_system("state=PrepareHugepages");

                // Phase 12: deterministic hugepage provisioning.
                crate::hugepages::provision_for_vm(&mut self.rt)?;

                self.state = VmState::PreparePci;
            }

            VmState::PreparePci => {
                info!("state=PreparePci");
                self.rt.push_system("state=PreparePci");

                crate::vfio::stage_pci_devices_for_vm(&mut self.rt)?;
                self.rt
                    .push_info("vfio: PCI staging completed for VM passthrough devices");

                self.state = VmState::ReserveCpus;
            }

            VmState::ReserveCpus => {
                info!("state=ReserveCpus");
                self.rt.push_system("state=ReserveCpus");

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
                    self.rt
                        .push_warning("qemu: launch completed but QemuState pid not recorded");
                }

                self.state = VmState::DetectThreads;
            }

            VmState::DetectThreads => {
                info!("state=DetectThreads");
                self.rt.push_system("state=DetectThreads");

                crate::cpu::wait_for_qemu_threads(&self.rt)?;
                self.rt
                    .push_info("affinity: QEMU threads discovered for pinning");

                self.state = VmState::PinVcpus;
            }

            VmState::PinVcpus => {
                info!("state=PinVcpus");
                self.rt.push_system("state=PinVcpus");

                crate::cpu::pin_vcpus(&self.rt)?;
                self.rt.pinned_threads = true;
                self.rt
                    .push_info("affinity: vCPU threads pinned to VM cpuset");

                self.state = VmState::DetectMsi;
            }

            VmState::DetectMsi => {
                info!("state=DetectMsi");
                self.rt.push_system("state=DetectMsi");

                crate::irq::spawn_irq_pin_worker(&self.rt)?;
                self.rt.push_info(
                    "irq: background worker spawned for MSI/MSI-X detection and IRQ pinning",
                );

                self.state = VmState::PinIrqs;
            }

            VmState::PinIrqs => {
                info!("state=PinIrqs");
                self.rt.push_system("state=PinIrqs");

                self.rt.pinned_irqs = true;
                self.rt
                    .push_info("irq: IRQ pinning delegated to background worker");

                self.state = VmState::PeripheralHooks;
            }

            VmState::PeripheralHooks => {
                info!("state=PeripheralHooks");
                self.rt.push_system("state=PeripheralHooks");

                crate::peripherals::apply_vm_up(&mut self.rt)?;
                self.rt
                    .push_info("peripherals: VM up hooks applied successfully");

                self.state = VmState::Steady;
            }

            VmState::Steady => {
                info!("VM already in steady-state");
                self.rt.push_system("state=Steady");
                self.rt
                    .push_info("vm: already in steady-state (all bring-up phases complete)");
            }

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

    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn step_shutdown(&mut self) -> Result<VmState> {
        match self.state {
            VmState::Steady | VmState::PinIrqs | VmState::PeripheralHooks => {
                info!("state=Shutdown");
                self.rt.push_system("state=Shutdown");

                crate::qemu::shutdown(&mut self.rt)?;
                self.rt
                    .push_info("qemu: shutdown requested for VM instance");

                self.state = VmState::Shutdown;
            }

            VmState::Shutdown => {
                info!("state=RestorePci");
                self.rt
                    .push_system("state=RestorePci (restore PCI driver bindings)");
                crate::vfio::restore_pci_devices_for_vm(&self.rt)?;
                self.rt
                    .push_info("vfio: restore of PCI driver bindings requested for VM devices");

                info!("state=Cleanup");
                self.rt.push_system("state=Cleanup");

                crate::cpu::cleanup_cpus(&mut self.rt)?;
                self.rt
                    .push_info("cpuset: VM/host cpuset hierarchy cleaned up");

                crate::hugepages::cleanup_for_vm(&mut self.rt)?;
                self.rt
                    .push_info("hugepages: teardown completed for VM instance");

                crate::peripherals::apply_vm_down(&mut self.rt)?;
                self.rt
                    .push_info("peripherals: VM down hooks applied successfully");

                self.state = VmState::Cleanup;
            }

            VmState::Cleanup => {
                self.state = VmState::Idle;
                self.rt
                    .push_system("state=Idle (VM shutdown sequence completed)");
            }

            VmState::Idle => {
                info!("VM already in Idle; shutdown step is a no-op");
                self.rt
                    .push_system("state=Idle (shutdown already completed)");
            }

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

    #[instrument(skip_all, fields(vm = %self.rt.name))]
    pub fn run_shutdown(&mut self) -> Result<()> {
        loop {
            match self.state {
                VmState::Idle => {
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
