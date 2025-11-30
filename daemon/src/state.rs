// daemon/src/state.rs

use std::collections::HashMap;

use crate::ipc::{IpcEvent, IpcEventKind, IpcVmEvents, IpcVmState, IpcVmStatus};

// ---- Real Daemon State + Core Integration (Phase 11) ----------------------
//
// This module owns the daemon-side view of Chalybs VM state:
//
//   - RootConfig and a read-only PCI inventory snapshot for introspection
//   - Startup events (config + PCI introspection) for global + per-VM
//   - A registry of DaemonVm entries, each wrapping a core VmStateMachine
//     plus a "desired" IPC state and an event cursor.
//
// The synthetic backend used in early TUI development has been removed;
// all snapshots are now derived from real config + real state machines,
// with VM lifecycle driven deterministically from daemon ticks.

use chalybs_core::config::pci::preflight_gpu_policy;
use chalybs_core::config::{IsolationMode, PciDeviceConfig, RootConfig, VmConfig};
use chalybs_core::model::{CoreEventKind, CpuSet, VmCpuLayout, VmRuntime};
use chalybs_core::pci::{PciFunction, PciInventory};
use chalybs_core::state::{VmState, VmStateMachine};
use chalybs_core::util::parse_cpu_list;

/// A single VM managed by the daemon.
///
/// This wraps the core VmStateMachine plus daemon-local metadata:
///   - `desired`: the VM's desired high-level IPC state (Running/Stopped)
///   - `last_sent_event_index`: cursor into VmRuntime.events used to
///     emit incremental VmEvents to the TUI.
pub struct DaemonVm {
    pub sm: VmStateMachine,
    pub desired: IpcVmState,
    pub last_sent_event_index: usize,
}

impl DaemonVm {
    /// Construct a new DaemonVm from a config entry.
    ///
    /// This parses the VM/host CPU lists into a VmCpuLayout, builds a
    /// VmRuntime, and wraps it in a VmStateMachine. On error, a human-
    /// readable string is returned so the caller can surface it as an
    /// IPC error event.
    pub fn new(vm_name: &str, vm_cfg: &VmConfig) -> Result<Self, String> {
        let host_cpus = parse_cpu_list(vm_cfg.cpu.host_cpus.as_str()).map_err(|e| {
            format!(
                "vm {vm_name}: failed to parse host_cpus=\"{}\" for runtime construction: {e}",
                vm_cfg.cpu.host_cpus
            )
        })?;

        let vm_cpus = parse_cpu_list(vm_cfg.cpu.vm_cpus.as_str()).map_err(|e| {
            format!(
                "vm {vm_name}: failed to parse vm_cpus=\"{}\" for runtime construction: {e}",
                vm_cfg.cpu.vm_cpus
            )
        })?;

        let layout = VmCpuLayout {
            host: CpuSet { cpus: host_cpus },
            vm: CpuSet { cpus: vm_cpus },
        };

        let rt = VmRuntime::new(vm_name.to_string(), vm_cfg.clone(), layout);
        let sm = VmStateMachine::new(rt);

        Ok(Self {
            sm,
            desired: IpcVmState::Stopped,
            last_sent_event_index: 0,
        })
    }
}

/// Daemon-wide state.
///
/// This is constructed once at chalybsd startup and then mutated in
/// the main server loop. All VM lifecycle activity flows through the
/// `vms` map, which holds DaemonVm entries keyed by VM name.
pub struct DaemonState {
    /// Loaded root config, if any.
    pub root_cfg: Option<RootConfig>,
    /// PCI inventory snapshot captured at daemon startup.
    pub pci_inventory: Option<PciInventory>,
    /// Global startup events (config + PCI introspection).
    pub startup_events_global: Vec<IpcEvent>,
    /// Per-VM startup events (config + PCI / GPU introspection).
    pub startup_events_vm: Vec<IpcVmEvents>,
    /// Active VM state machines keyed by VM name.
    pub vms: HashMap<String, DaemonVm>,
}

impl DaemonState {
    /// Construct daemon state from config + PCI inventory.
    ///
    /// This preserves the original Phase 3A behavior for config/PCI
    /// introspection (startup events), while adding a new VM registry
    /// for real core state machines. VM runtimes are created lazily
    /// when `vm start` is requested, not eagerly at startup.
    pub fn new() -> Self {
        // Load config (unchanged semantics).
        let (root_cfg, mut startup_events_global) = load_root_config();

        // PCI inventory introspection (unchanged semantics).
        let (pci_inventory, pci_events) = match PciInventory::scan() {
            Ok(inv) => {
                let mut events = Vec::new();
                events.push(IpcEvent {
                    kind: IpcEventKind::System,
                    message: format!(
                        "daemon: PCI inventory built ({} functions, {} display controllers)",
                        inv.functions.len(),
                        inv.count_display_controllers()
                    ),
                });

                for s in inv.gpu_summaries() {
                    let driver = s.driver.as_deref().unwrap_or("<none>");
                    let driver_kind = s
                        .driver_kind
                        .as_ref()
                        .map(|k| format!("{k:?}"))
                        .unwrap_or_else(|| "<none>".to_string());
                    let safety = s
                        .safety
                        .as_ref()
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|| "<none>".to_string());

                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "GPU {}: vendor=0x{:04x}, device=0x{:04x}, driver={driver}, \
                             driver_kind={driver_kind}, safety={safety}",
                            s.bdf, s.vendor_id, s.device_id
                        ),
                    });
                }

                (Some(inv), events)
            }
            Err(e) => (
                None,
                vec![IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!("daemon: failed to scan PCI inventory: {e}"),
                }],
            ),
        };

        startup_events_global.extend(pci_events);

        // Per-VM startup introspection (unchanged semantics).
        let mut startup_events_vm = Vec::new();
        if let Some(ref cfg) = root_cfg {
            if let Some(inv) = &pci_inventory {
                for (vm_name, vm_cfg) in &cfg.vm {
                    let vm_events = introspect_vm_full(vm_name, vm_cfg, inv);
                    if !vm_events.is_empty() {
                        startup_events_vm.push(IpcVmEvents {
                            vm_name: vm_name.clone(),
                            events: vm_events,
                        });
                    }
                }
            } else {
                for (vm_name, vm_cfg) in &cfg.vm {
                    let vm_events = introspect_vm_config_only(vm_name, vm_cfg);
                    if !vm_events.is_empty() {
                        startup_events_vm.push(IpcVmEvents {
                            vm_name: vm_name.clone(),
                            events: vm_events,
                        });
                    }
                }
            }
        }

        Self {
            root_cfg,
            pci_inventory,
            startup_events_global,
            startup_events_vm,
            vms: HashMap::new(),
        }
    }

    /// Advance all managed VMs one deterministic tick towards their
    /// desired state and build a snapshot for the TUI.
    ///
    /// This replaces the old synthetic backend. Snapshots now reflect:
    ///   - real VmStateMachine states (Init → Steady → Idle)
    ///   - real VM-scoped events from VmRuntime.events
    ///   - real cpuset / IRQ pinning flags from VmRuntime
    pub fn build_snapshot(
        &mut self,
        tick: u64,
    ) -> (Vec<IpcVmStatus>, Vec<IpcEvent>, Vec<IpcVmEvents>) {
        let mut vms_status = Vec::new();
        let mut events_global = Vec::new();
        let mut events_vm = Vec::new();

        // Inject startup events on tick == 1 (unchanged semantics).
        if tick == 1 {
            events_global.extend(self.startup_events_global.clone());
            events_vm.extend(self.startup_events_vm.clone());
        }

        // Build VM status for all VMs defined in config (sorted by name
        // for deterministic ordering), regardless of whether they have
        // an active state machine yet.
        if let Some(ref cfg) = self.root_cfg {
            let mut names: Vec<&String> = cfg.vm.keys().collect();
            names.sort();

            for name in names {
                let name_str = name.as_str();
                let vm_cfg = &cfg.vm[name_str];

                // If we have an active DaemonVm, drive its state machine
                // one step towards the desired state.
                let (view_state, cpu_pinned, irq_pinned, tasmota_on) =
                    if let Some(dvm) = self.vms.get_mut(name_str) {
                        drive_vm_towards_desired(dvm);

                        let view_state = map_vm_state_to_ipc(dvm.sm.state);
                        let cpu_pinned = dvm.sm.rt.pinned_threads;
                        let irq_pinned = dvm.sm.rt.pinned_irqs;

                        let tasmota_on = dvm
                            .sm
                            .rt
                            .cfg
                            .peripherals
                            .as_ref()
                            .and_then(|p| p.tasmota.as_ref())
                            .is_some();

                        (view_state, cpu_pinned, irq_pinned, tasmota_on)
                    } else {
                        // No active runtime yet; treat as stopped and
                        // rely purely on config for static fields.
                        let tasmota_on = vm_cfg
                            .peripherals
                            .as_ref()
                            .and_then(|p| p.tasmota.as_ref())
                            .is_some();

                        (IpcVmState::Stopped, false, false, tasmota_on)
                    };

                let isolation_mode = match vm_cfg.isolation.mode {
                    IsolationMode::Disabled => "disabled",
                    IsolationMode::Audit => "audit",
                    IsolationMode::Enforce => "enforce",
                }
                .to_string();

                let hugepages = vm_cfg.qemu.hugepages;

                vms_status.push(IpcVmStatus {
                    name: name_str.to_string(),
                    state: view_state,
                    cpu_pinned,
                    irq_pinned,
                    tasmota_on,
                    isolation_mode,
                    hugepages,
                });
            }
        }

        // Daemon heartbeat at fixed interval (unchanged).
        if tick % 40 == 0 {
            events_global.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!("daemon heartbeat: tick={tick}"),
            });
        }

        // Append VM-scoped core events incrementally for each active VM.
        for (name, dvm) in self.vms.iter_mut() {
            if dvm.sm.rt.events.len() <= dvm.last_sent_event_index {
                continue;
            }

            let new_events = &dvm.sm.rt.events[dvm.last_sent_event_index..];
            let mut ipc_events = Vec::new();

            for ev in new_events {
                let kind = match ev.kind {
                    CoreEventKind::Info => IpcEventKind::Info,
                    CoreEventKind::Warning => IpcEventKind::Warning,
                    CoreEventKind::Error => IpcEventKind::Error,
                    CoreEventKind::System => IpcEventKind::System,
                };

                ipc_events.push(IpcEvent {
                    kind,
                    message: ev.message.clone(),
                });
            }

            if !ipc_events.is_empty() {
                events_vm.push(IpcVmEvents {
                    vm_name: name.clone(),
                    events: ipc_events,
                });
            }

            dvm.last_sent_event_index = dvm.sm.rt.events.len();
        }

        (vms_status, events_global, events_vm)
    }
}

/// Drive a VM one step towards its desired high-level state.
///
/// This is intentionally conservative:
///   - Starting/Running ⇒ bring-up via step() from valid bring-up
///     states (Init → Steady).
///   - Stopped/ShuttingDown ⇒ shutdown via step_shutdown() until Idle.
///   - Any state machine error is recorded as a VM-scoped error event
///     and the desired state is forced to Stopped to avoid tight loops.
fn drive_vm_towards_desired(vm: &mut DaemonVm) {
    match vm.desired {
        IpcVmState::Running | IpcVmState::Starting => match vm.sm.state {
            VmState::Init
            | VmState::Validate
            | VmState::PreparePci
            | VmState::ReserveCpus
            | VmState::LaunchQemu
            | VmState::DetectThreads
            | VmState::PinVcpus
            | VmState::DetectMsi
            | VmState::PinIrqs
            | VmState::PeripheralHooks => {
                if let Err(e) = vm.sm.step() {
                    vm.sm
                        .rt
                        .push_error(format!("vm state machine bring-up error: {e}"));
                    vm.desired = IpcVmState::Stopped;
                }
            }
            VmState::Steady => {
                // Reached steady-state; normalize desired to Running.
                vm.desired = IpcVmState::Running;
            }
            VmState::Shutdown | VmState::Cleanup | VmState::Idle => {
                // Currently shutting down or fully Idle; do not attempt
                // to re-enter bring-up implicitly. The operator should
                // issue another `vm start` to reinitialize.
            }
        },

        IpcVmState::Stopped | IpcVmState::ShuttingDown => match vm.sm.state {
            VmState::Idle => {
                // Fully shut down; nothing more to do.
            }
            _ => {
                if let Err(e) = vm.sm.step_shutdown() {
                    vm.sm
                        .rt
                        .push_error(format!("vm state machine shutdown error: {e}"));
                }
            }
        },
    }
}

/// Map core VmState into a coarse IPC-visible VmState.
pub fn map_vm_state_to_ipc(state: VmState) -> IpcVmState {
    match state {
        VmState::Init
        | VmState::Validate
        | VmState::PreparePci
        | VmState::ReserveCpus
        | VmState::LaunchQemu
        | VmState::DetectThreads
        | VmState::PinVcpus
        | VmState::DetectMsi
        | VmState::PinIrqs
        | VmState::PeripheralHooks => IpcVmState::Starting,
        VmState::Steady => IpcVmState::Running,
        VmState::Shutdown | VmState::Cleanup => IpcVmState::ShuttingDown,
        VmState::Idle => IpcVmState::Stopped,
    }
}

// ----- Helpers from original server.rs (config + PCI introspection) --------

fn load_root_config() -> (Option<RootConfig>, Vec<IpcEvent>) {
    // COPIED EXACTLY from original server.rs
    use std::path::Path;
    const DEFAULT_CONFIG_PATH: &str = "/etc/chalybs/chalybs.toml";

    let mut events = Vec::new();

    if let Ok(path_str) = std::env::var("CHALYBS_CONFIG") {
        let path = Path::new(&path_str);
        match RootConfig::from_path(path) {
            Ok(cfg) => {
                events.push(IpcEvent {
                    kind: IpcEventKind::System,
                    message: format!(
                        "daemon: loaded config from CHALYBS_CONFIG={}",
                        path.display()
                    ),
                });
                return (Some(cfg), events);
            }
            Err(e) => {
                let msg = format!(
                    "daemon: failed to load config from CHALYBS_CONFIG={}: {e}",
                    path.display()
                );
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: msg,
                });
            }
        }
    }

    let default_path = Path::new(DEFAULT_CONFIG_PATH);
    if default_path.exists() {
        match RootConfig::from_path(default_path) {
            Ok(cfg) => {
                events.push(IpcEvent {
                    kind: IpcEventKind::System,
                    message: format!(
                        "daemon: loaded config from default path {}",
                        default_path.display()
                    ),
                });
                return (Some(cfg), events);
            }
            Err(e) => {
                let msg = format!(
                    "daemon: failed to load config from {}: {e}",
                    default_path.display()
                );
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: msg,
                });
                return (None, events);
            }
        }
    }

    events.push(IpcEvent {
        kind: IpcEventKind::Error,
        message: format!(
            "daemon: no config found (CHALYBS_CONFIG unset, {} missing)",
            DEFAULT_CONFIG_PATH
        ),
    });

    (None, events)
}

fn introspect_vm_config_only(vm_name: &str, vm: &VmConfig) -> Vec<IpcEvent> {
    // COPIED EXACTLY from original server.rs
    let mut events = Vec::new();

    match parse_cpu_list(vm.cpu.vm_cpus.as_str()) {
        Ok(vcpus) => events.push(IpcEvent {
            kind: IpcEventKind::Info,
            message: format!(
                "vm {vm_name}: parsed vm_cpus=\"{}\" ({} vCPUs)",
                vm.cpu.vm_cpus,
                vcpus.len()
            ),
        }),
        Err(e) => events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: format!(
                "vm {vm_name}: failed to parse vm_cpus=\"{}\": {e}",
                vm.cpu.vm_cpus
            ),
        }),
    }

    match parse_cpu_list(vm.cpu.host_cpus.as_str()) {
        Ok(hcpus) => events.push(IpcEvent {
            kind: IpcEventKind::Info,
            message: format!(
                "vm {vm_name}: parsed host_cpus=\"{}\" ({} host CPUs)",
                vm.cpu.host_cpus,
                hcpus.len()
            ),
        }),
        Err(e) => events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: format!(
                "vm {vm_name}: failed to parse host_cpus=\"{}\": {e}",
                vm.cpu.host_cpus
            ),
        }),
    }

    let iso_mode = match vm.isolation.mode {
        IsolationMode::Disabled => "disabled",
        IsolationMode::Audit => "audit",
        IsolationMode::Enforce => "enforce",
    };

    events.push(IpcEvent {
        kind: IpcEventKind::Info,
        message: format!(
            "vm {vm_name}: isolation mode={iso_mode}, default_level={:?}",
            vm.isolation.default_level
        ),
    });

    events
}

fn introspect_vm_full(vm_name: &str, vm: &VmConfig, inv: &PciInventory) -> Vec<IpcEvent> {
    let mut events = introspect_vm_config_only(vm_name, vm);

    introspect_vm_devices(vm_name, vm, inv, &mut events);

    match preflight_gpu_policy(vm_name, vm) {
        Ok(()) => events.push(IpcEvent {
            kind: IpcEventKind::Info,
            message: "GPU policy preflight passed".to_string(),
        }),
        Err(e) => events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: format!("GPU policy preflight failed: {e}"),
        }),
    }

    events
}

fn introspect_vm_devices(
    vm_name: &str,
    vm: &VmConfig,
    inv: &PciInventory,
    events: &mut Vec<IpcEvent>,
) {
    inspect_device_list(
        vm_name,
        "GPU",
        vm.devices.gpu.as_ref(),
        inv,
        Some(DeviceKind::Gpu),
        events,
    );
    inspect_device_list(
        vm_name,
        "NVMe",
        vm.devices.nvme.as_ref(),
        inv,
        Some(DeviceKind::Nvme),
        events,
    );
    inspect_device_list(
        vm_name,
        "NIC",
        vm.devices.nic.as_ref(),
        inv,
        Some(DeviceKind::Nic),
        events,
    );
    inspect_device_list(
        vm_name,
        "USB",
        vm.devices.usb.as_ref(),
        inv,
        Some(DeviceKind::Usb),
        events,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceKind {
    Gpu,
    Nvme,
    Nic,
    Usb,
}

fn inspect_device_list(
    vm_name: &str,
    kind_label: &str,
    cfgs_opt: Option<&Vec<PciDeviceConfig>>,
    inv: &PciInventory,
    kind: Option<DeviceKind>,
    events: &mut Vec<IpcEvent>,
) {
    let cfgs = match cfgs_opt {
        Some(c) if !c.is_empty() => c,
        _ => return,
    };

    for dev in cfgs {
        match inv.find_by_bdf(dev.pci_address.as_str()) {
            Some(func) => {
                if let Some(kind) = kind {
                    if !device_kind_matches(func, kind) {
                        events.push(IpcEvent {
                            kind: IpcEventKind::Warning,
                            message: format!(
                                "vm {vm_name}: {kind_label} device {} present, wrong PCI class (expected {kind:?})",
                                dev.pci_address
                            ),
                        });
                    } else {
                        events.push(IpcEvent {
                            kind: IpcEventKind::Info,
                            message: format!(
                                "vm {vm_name}: {kind_label} device {} present in inventory",
                                dev.pci_address
                            ),
                        });
                    }
                } else {
                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {vm_name}: device {} present in inventory",
                            dev.pci_address
                        ),
                    });
                }
            }
            None if dev.required => {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!(
                        "vm {vm_name}: required {kind_label} device {} is missing from PCI inventory",
                        dev.pci_address
                    ),
                });
            }
            None => events.push(IpcEvent {
                kind: IpcEventKind::Warning,
                message: format!(
                    "vm {vm_name}: optional {kind_label} device {} not found in PCI inventory",
                    dev.pci_address
                ),
            }),
        }
    }
}

fn device_kind_matches(func: &PciFunction, kind: DeviceKind) -> bool {
    match kind {
        DeviceKind::Gpu => func.is_display_controller(),
        DeviceKind::Nvme => func.is_nvme(),
        DeviceKind::Nic => func.is_network_controller(),
        DeviceKind::Usb => func.is_usb_controller(),
    }
}
