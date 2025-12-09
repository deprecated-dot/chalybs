// daemon/src/state.rs

use std::collections::HashMap;

use crate::ipc::{
    IpcEvent, IpcEventKind, IpcVmCpuLayout, IpcVmCpuSample, IpcVmEvents, IpcVmHugepages,
    IpcVmState, IpcVmStatus,
};

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
use tracing::{info, warn};

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

        // Per-VM startup introspection.
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

    /// Advance all managed VMs one deterministic tick.
    pub fn build_snapshot(
        &mut self,
        tick: u64,
    ) -> (Vec<IpcVmStatus>, Vec<IpcEvent>, Vec<IpcVmEvents>) {
        let mut vms_status = Vec::new();
        let mut events_global = Vec::new();
        let mut events_vm = Vec::new();

        if tick == 1 {
            events_global.extend(self.startup_events_global.clone());
            events_vm.extend(self.startup_events_vm.clone());
        }

        if let Some(ref cfg) = self.root_cfg {
            let mut names: Vec<&String> = cfg.vm.keys().collect();
            names.sort();

            for name in names {
                let name_str = name.as_str();
                let vm_cfg = &cfg.vm[name_str];

                // Projection model fields that can be derived from config alone.
                let cpu_layout = IpcVmCpuLayout {
                    host: parse_cpu_list(vm_cfg.cpu.host_cpus.as_str()).unwrap_or_default(),
                    vm: parse_cpu_list(vm_cfg.cpu.vm_cpus.as_str()).unwrap_or_default(),
                };

                let hugepages = vm_cfg.qemu.hugepages;

                // Runtime-derived detail; default for "no active VM".
                let mut hugepages_detail = IpcVmHugepages {
                    active: false,
                    node: None,
                    pages: 0,
                    bytes: 0,
                };

                let mut cpu_sample: Option<IpcVmCpuSample> = None;

                let (view_state, cpu_pinned, irq_pinned, tasmota_on) = if let Some(dvm) =
                    self.vms.get_mut(name_str)
                {
                    // Deterministic guest-initiated shutdown detection:
                    //
                    // If QEMU has exited (e.g. Windows Server shutdown from inside
                    // the guest), we observe the child status here and:
                    //
                    //   - Drop the QEMU handle from the runtime so the core
                    //     shutdown path does *not* attempt another SIGTERM.
                    //   - Set `desired` to Stopped so the daemon/core state
                    //     machine walks the normal Shutdown → Cleanup → Idle path.
                    //
                    // This keeps the TUI/daemon view in sync with reality and
                    // still routes cleanup through the core state machine.
                    if dvm.sm.state == VmState::Steady {
                        if let Some(ref mut qstate) = dvm.sm.rt.qemu {
                            match qstate.child.try_wait() {
                                Ok(Some(status)) => {
                                    info!(
                                        vm = name_str,
                                        pid = qstate.pid,
                                        ?status,
                                        "daemon: detected QEMU process exit (guest-initiated shutdown)"
                                    );
                                    // Drop the QEMU handle so qemu::shutdown() is a no-op.
                                    dvm.sm.rt.qemu = None;
                                    // Ask the core to walk the shutdown path on the next tick.
                                    dvm.desired = IpcVmState::Stopped;
                                }
                                Ok(None) => {
                                    // Still running; nothing to do.
                                }
                                Err(e) => {
                                    warn!(
                                        vm = name_str,
                                        pid = qstate.pid,
                                        error = %e,
                                        "daemon: failed to poll QEMU child process status"
                                    );
                                }
                            }
                        }
                    }

                    // Drive state machine toward desired state (bring-up or shutdown).
                    drive_vm_towards_desired(dvm);

                    let view_state = map_vm_state_to_ipc(dvm.sm.state);
                    let mut cpu_pinned = dvm.sm.rt.pinned_threads;
                    let mut irq_pinned = dvm.sm.rt.pinned_irqs;

                    let tasmota_configured = dvm
                        .sm
                        .rt
                        .cfg
                        .peripherals
                        .as_ref()
                        .and_then(|p| p.tasmota.as_ref())
                        .is_some();

                    // NOTE: tasmota_powered is a Cell<bool>; read with .get()
                    let mut tasmota_on = tasmota_configured && dvm.sm.rt.tasmota_powered.get();

                    // When the VM is stopped, we explicitly drop pinned + tasmota flags
                    // in the projection model so the TUI does not carry stale state.
                    if matches!(view_state, IpcVmState::Stopped) {
                        cpu_pinned = false;
                        irq_pinned = false;
                        tasmota_on = false;
                    }

                    // Populate hugepage detail from runtime.
                    hugepages_detail = IpcVmHugepages {
                        active: dvm.sm.rt.hugepages_active,
                        node: dvm.sm.rt.hugepages_node,
                        pages: dvm.sm.rt.hugepages_pages,
                        bytes: dvm.sm.rt.hugepages_bytes,
                    };

                    // Snapshot raw vCPU usage when the VM is running.
                    if matches!(view_state, IpcVmState::Running) {
                        cpu_sample = snapshot_vm_cpu_sample(&dvm.sm.rt);
                    }

                    (view_state, cpu_pinned, irq_pinned, tasmota_on)
                } else {
                    // VM is configured but has no active daemon-side state machine.
                    (IpcVmState::Stopped, false, false, false)
                };

                let isolation_mode = match vm_cfg.isolation.mode {
                    IsolationMode::Disabled => "disabled",
                    IsolationMode::Audit => "audit",
                    IsolationMode::Enforce => "enforce",
                }
                .to_string();

                vms_status.push(IpcVmStatus {
                    name: name_str.to_string(),
                    state: view_state,
                    cpu_pinned,
                    irq_pinned,
                    tasmota_on,
                    isolation_mode,
                    hugepages,
                    cpu_layout,
                    hugepages_detail,
                    cpu_sample,
                });
            }
        }

        if tick % 40 == 0 {
            events_global.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!("daemon heartbeat: tick={tick}"),
            });
        }

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

/// Drive a VM one step toward its desired state.
fn drive_vm_towards_desired(vm: &mut DaemonVm) {
    match vm.desired {
        IpcVmState::Running | IpcVmState::Starting => match vm.sm.state {
            VmState::Init
            | VmState::Validate
            | VmState::PrepareHugepages
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
                vm.desired = IpcVmState::Running;
            }
            VmState::Shutdown | VmState::Cleanup => {
                // Already on the shutdown path; nothing to do here for a
                // "keep running" desire until the state machine returns to Idle.
            }
            VmState::Idle => {
                // Idle + desire=Running will be handled on the next tick as
                // the caller transitions desired to Running and step() begins
                // a fresh bring-up sequence.
            }
        },

        IpcVmState::Stopped | IpcVmState::ShuttingDown => match vm.sm.state {
            VmState::Idle => {}
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

/// Map core VmState into coarse IPC state.
pub fn map_vm_state_to_ipc(state: VmState) -> IpcVmState {
    match state {
        VmState::Init
        | VmState::Validate
        | VmState::PrepareHugepages
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

// ----- Helpers from original server.rs -------------------------------------

fn load_root_config() -> (Option<RootConfig>, Vec<IpcEvent>) {
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

/// Local copy of the vCPU thread-name parser used in core affinity.
///
/// Historically QEMU vCPU threads show up as "CPU N/KVM". To make this
/// robust against minor QEMU variations, we accept:
///
///   - "CPU N"
///   - "CPU N/KVM"
///   - "CPU N/kvm"
///   - "CPU N/qemu"
///   - "CPU N/<anything>"
///
/// and simply parse the first integer after "CPU ".
fn parse_vcpu_name(name: &str) -> Option<u32> {
    if !name.starts_with("CPU ") {
        return None;
    }

    let rest = &name[4..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();

    if digits.is_empty() {
        return None;
    }

    digits.parse::<u32>().ok()
}

/// Snapshot raw vCPU usage for a VM into the IPC projection model.
///
/// This is intentionally minimal and deterministic:
///   - If QEMU is not present, returns None.
///   - If /proc lookups fail, returns None.
///   - On success, returns a single IpcVmCpuSample with per-vCPU jiffies.
///
/// No heuristics, no retries, no time windows; the TUI is responsible
/// for turning these counters into "load" over time.
fn snapshot_vm_cpu_sample(rt: &VmRuntime) -> Option<IpcVmCpuSample> {
    use procfs::process::Process;

    let q = rt.qemu.as_ref()?;
    let pid = q.pid;

    let proc = Process::new(pid).ok()?;
    let tasks = proc.tasks().ok()?;

    let mut pairs: Vec<(u32, u64)> = Vec::new();

    for task_res in tasks {
        let task = match task_res {
            Ok(t) => t,
            Err(_) => continue,
        };

        let stat = match task.stat() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let idx = match parse_vcpu_name(&stat.comm) {
            Some(i) => i,
            None => continue,
        };

        // utime + stime: userspace + kernel jiffies.
        let total = stat.utime.saturating_add(stat.stime);
        pairs.push((idx, total));
    }

    if pairs.is_empty() {
        return None;
    }

    pairs.sort_by_key(|(idx, _)| *idx);

    let mut vcpu_indices = Vec::with_capacity(pairs.len());
    let mut vcpu_jiffies = Vec::with_capacity(pairs.len());

    for (idx, ticks) in pairs {
        vcpu_indices.push(idx);
        vcpu_jiffies.push(ticks);
    }

    Some(IpcVmCpuSample {
        vcpu_indices,
        vcpu_jiffies,
    })
}
