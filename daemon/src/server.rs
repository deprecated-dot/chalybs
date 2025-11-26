use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Error, Result};
use serde_json;

use crate::ipc::{IpcEvent, IpcEventKind, IpcMessage, IpcVmEvents, IpcVmState, IpcVmStatus};

// ---- Chalybs core imports (read-only introspection) ------------------------

use chalybs_core::config::pci::preflight_gpu_policy;
use chalybs_core::config::{IsolationMode, PciDeviceConfig, RootConfig, VmConfig};
use chalybs_core::pci::{PciFunction, PciInventory};
use chalybs_core::util::parse_cpu_list;

// ---------------------------------------------------------------------------

const PRIMARY_SOCKET: &str = "/run/chalybsd.sock";
const FALLBACK_SOCKET: &str = "/tmp/chalybsd.sock";
const DEFAULT_CONFIG_PATH: &str = "/etc/chalybs/chalybs.toml";

fn choose_socket_path() -> PathBuf {
    let run = Path::new("/run");
    if run.is_dir() {
        PathBuf::from(PRIMARY_SOCKET)
    } else {
        PathBuf::from(FALLBACK_SOCKET)
    }
}

/// A connected TUI client.
///
/// Each client has its own stream and receive buffer for decoding
/// newline-delimited JSON messages coming from the TUI (shell commands).
struct Client {
    stream: UnixStream,
    rx_buf: String,
}

impl Client {
    fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Client {
            stream,
            rx_buf: String::new(),
        })
    }
}

/// Outcome of draining commands from a single client.
enum ClientDrainOutcome {
    /// Client is still connected and usable.
    Alive,
    /// Client closed the connection cleanly (EOF or expected disconnect).
    DisconnectedGraceful,
    /// Client encountered an error and should be dropped.
    DisconnectedError(Error),
}

/// Daemon-wide, read-only state for Phase 2C / 3A.
///
/// This is the "introspective" view of the system:
/// - Optional RootConfig
/// - Optional PCI inventory
/// - Startup global + per-VM events that get emitted once (on tick 1)
/// - Intent-only VM state overrides (vm start/stop/restart) projected
///   into IpcVmState for the TUI
struct DaemonState {
    root_cfg: Option<RootConfig>,
    pci_inventory: Option<PciInventory>,
    startup_events_global: Vec<IpcEvent>,
    startup_events_vm: Vec<IpcVmEvents>,

    /// Intent-only VM state overrides, keyed by VM name.
    ///
    /// In Phase 3A this is purely a *view* of requested lifecycle
    /// actions (vm start/stop/restart) and does not actually launch or
    /// stop QEMU. It lets the TUI show structured "pending" state
    /// instead of only log lines.
    vm_states: HashMap<String, IpcVmState>,
}

impl DaemonState {
    /// Build a new DaemonState by:
    /// - Loading configuration (if present)
    /// - Scanning PCI inventory (read-only)
    /// - Computing global + per-VM introspection events
    fn new() -> Self {
        let mut startup_events_global = Vec::new();

        // 1) Load configuration (env override + default path).
        let (root_cfg, cfg_events) = load_root_config();
        startup_events_global.extend(cfg_events);

        // 2) Build PCI inventory (read-only sysfs scan).
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

                // GPU classification summary.
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
                            "GPU {}: vendor=0x{:04x}, device=0x{:04x}, driver={driver}, driver_kind={driver_kind}, safety={safety}",
                            s.bdf, s.vendor_id, s.device_id
                        ),
                    });
                }

                (Some(inv), events)
            }
            Err(e) => {
                let msg = format!("daemon: failed to scan PCI inventory: {e}");
                tracing::warn!("{msg}");
                (
                    None,
                    vec![IpcEvent {
                        kind: IpcEventKind::Error,
                        message: msg,
                    }],
                )
            }
        };

        startup_events_global.extend(pci_events);

        // 3) Per-VM introspection: CPU parsing, isolation, PCI presence,
        //    GPU policy preflight, etc. All read-only.
        let mut startup_events_vm = Vec::new();

        if let Some(ref cfg) = root_cfg {
            match &pci_inventory {
                Some(inv) => {
                    // Full introspection (config + PCI).
                    for (vm_name, vm_cfg) in &cfg.vm {
                        let vm_events = introspect_vm_full(vm_name, vm_cfg, inv);
                        if !vm_events.is_empty() {
                            startup_events_vm.push(IpcVmEvents {
                                vm_name: vm_name.clone(),
                                events: vm_events,
                            });
                        }
                    }
                }
                None => {
                    // PCI inventory unavailable; config-only introspection.
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
        }

        DaemonState {
            root_cfg,
            pci_inventory,
            startup_events_global,
            startup_events_vm,
            vm_states: HashMap::new(),
        }
    }

    /// Get the current intent/view state for a VM, defaulting to Stopped
    /// in Phase 3A when no override is present.
    fn vm_view_state(&self, name: &str) -> IpcVmState {
        self.vm_states
            .get(name)
            .copied()
            .unwrap_or(IpcVmState::Stopped)
    }

    /// Set the intent/view state for a VM based on a lifecycle command.
    ///
    /// This is Phase 3A only: it does *not* launch or stop QEMU, it only
    /// updates what the TUI sees as the VM's state.
    fn set_vm_view_state(&mut self, name: &str, state: IpcVmState) {
        self.vm_states.insert(name.to_string(), state);
    }

    /// Build a read-only snapshot for the TUI.
    ///
    /// - VMs come from RootConfig (if present), otherwise empty.
    /// - VM state is derived from vm_states (intent-only) or Stopped.
    /// - Flags (hugepages, isolation mode, tasmota_on) come from config.
    /// - Startup introspection events are emitted once (tick == 1).
    /// - Heartbeat events are emitted periodically for liveness.
    fn build_snapshot(&self, tick: u64) -> (Vec<IpcVmStatus>, Vec<IpcEvent>, Vec<IpcVmEvents>) {
        // 1) Build VM status list from configuration.
        let mut vms: Vec<IpcVmStatus> = Vec::new();

        if let Some(ref cfg) = self.root_cfg {
            for (name, vm) in &cfg.vm {
                let isolation_mode = match vm.isolation.mode {
                    IsolationMode::Disabled => "disabled",
                    IsolationMode::Audit => "audit",
                    IsolationMode::Enforce => "enforce",
                }
                .to_string();

                let hugepages = vm.qemu.hugepages;

                // In Phase 2C/3A we do not actually talk to Tasmota yet; we
                // simply expose whether a Tasmota block is configured.
                let tasmota_on = vm
                    .peripherals
                    .as_ref()
                    .and_then(|p| p.tasmota.as_ref())
                    .is_some();

                let state = self.vm_view_state(name);

                vms.push(IpcVmStatus {
                    name: name.clone(),
                    state,
                    cpu_pinned: false,
                    irq_pinned: false,
                    tasmota_on,
                    isolation_mode,
                    hugepages,
                });
            }
        }

        // 2) Global events: startup + heartbeat.
        let mut events_global: Vec<IpcEvent> = Vec::new();

        if tick == 1 {
            events_global.extend(self.startup_events_global.clone());
        }

        // Simple deterministic heartbeat to prove liveness.
        if tick % 40 == 0 {
            events_global.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!("daemon heartbeat: tick={tick}"),
            });
        }

        // 3) Per-VM events: only emit startup introspection once.
        let mut events_vm: Vec<IpcVmEvents> = Vec::new();
        if tick == 1 {
            events_vm.extend(self.startup_events_vm.clone());
        }

        (vms, events_global, events_vm)
    }
}

/// Run the chalybsd IPC server in multi-client, push-snapshot mode.
///
/// Behavior:
/// - Binds a Unix-domain socket (preferring /run, falling back to /tmp).
/// - Accepts multiple TUI clients (non-blocking).
/// - Builds snapshots from real Chalybs config + PCI inventory (read-only).
/// - On each tick:
///   - Reads any pending shell commands from all clients.
///   - Applies shell commands to DaemonState as *intent-only* VM view
///     updates (vm start/stop/restart).
///   - Builds a fresh snapshot.
///   - Sends the same Snapshot to all connected clients.
/// - On any I/O error or EOF, drops the offending client and continues.
///
/// This keeps the control flow single-threaded and deterministic while
/// still allowing multiple simultaneous TUIs.
pub fn run_server() -> Result<()> {
    let path = choose_socket_path();

    // Remove any stale socket.
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("failed to remove stale socket at {}", path.display()))?;
    }

    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;

    listener
        .set_nonblocking(true)
        .context("failed to set listener nonblocking")?;

    tracing::info!("chalybsd: listening on {}", path.display());
    tracing::info!("chalybsd: multi-client mode enabled (single-threaded)");

    // Phase 2C/3A: build read-only daemon introspection state.
    let mut daemon_state = DaemonState::new();

    let mut clients: Vec<Client> = Vec::new();
    let mut tick: u64 = 0;

    // Shell-related buffers:
    // - pending_shell_events: echoes + parser errors, etc.
    // - pending_shell_commands: raw commands for semantic handling.
    let mut pending_shell_events: Vec<IpcEvent> = Vec::new();
    let mut pending_shell_commands: Vec<String> = Vec::new();

    loop {
        // 1. Accept new clients (non-blocking).
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    let _ = addr; // Unused for Unix-domain sockets.
                    match Client::new(stream) {
                        Ok(client) => {
                            tracing::info!(
                                "chalybsd: new TUI client connected (total={})",
                                clients.len() + 1
                            );
                            clients.push(client);
                        }
                        Err(err) => {
                            tracing::error!("chalybsd: failed to initialize client: {err}");
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    tracing::error!("chalybsd: accept error: {e}");
                    break;
                }
            }
        }

        // 2. Read commands from all clients.
        //
        // Any ShellCommand results in:
        //   - Echo events added to `pending_shell_events`.
        //   - Raw command strings added to `pending_shell_commands`.
        // These will be broadcast as part of the next snapshot.
        let mut i = 0;
        while i < clients.len() {
            let outcome = drain_client_commands(
                &mut clients[i],
                &mut pending_shell_events,
                &mut pending_shell_commands,
            );
            match outcome {
                ClientDrainOutcome::Alive => {
                    i += 1;
                }
                ClientDrainOutcome::DisconnectedGraceful => {
                    tracing::info!("chalybsd: client disconnected gracefully; removing");
                    clients.remove(i);
                }
                ClientDrainOutcome::DisconnectedError(err) => {
                    tracing::warn!("chalybsd: dropping client due to error: {err}");
                    clients.remove(i);
                }
            }
        }

        // 3. Handle parsed shell commands semantically (intent-only) and
        //    update daemon_state's VM view before building the snapshot.
        let mut shell_semantic_events: Vec<IpcEvent> = Vec::new();
        if !pending_shell_commands.is_empty() {
            for cmd in pending_shell_commands.drain(..) {
                let mut cmd_events = handle_shell_command(&mut daemon_state, cmd.as_str());
                shell_semantic_events.append(&mut cmd_events);
            }
        }

        // 4. Build a real snapshot from daemon_state.
        tick = tick.wrapping_add(1);

        let (mut vms, mut events_global, events_vm) = daemon_state.build_snapshot(tick);

        // First, inject any shell echo / decode events so they appear
        // before semantic command handling.
        if !pending_shell_events.is_empty() {
            events_global.append(&mut pending_shell_events);
        }

        // Next, inject semantic shell events (vm list/status/start/etc).
        if !shell_semantic_events.is_empty() {
            events_global.append(&mut shell_semantic_events);
        }

        // 5. Serialize the snapshot once.
        let msg = IpcMessage::Snapshot {
            vms: std::mem::take(&mut vms),
            events_global,
            events_vm,
        };

        let json = match serde_json::to_string(&msg) {
            Ok(j) => j,
            Err(err) => {
                tracing::error!("chalybsd: failed to serialize Snapshot: {err}");
                // If we can't serialize, skip this tick rather than
                // poisoning all clients.
                thread::sleep(Duration::from_millis(250));
                continue;
            }
        };

        let payload = format!("{json}\n");

        // 6. Broadcast to all clients; drop any that fail on write.
        let mut j = 0;
        while j < clients.len() {
            let write_result = clients[j].stream.write_all(payload.as_bytes());
            match write_result {
                Ok(()) => {
                    // Best-effort flush; ignore errors here.
                    let _ = clients[j].stream.flush();
                    j += 1;
                }
                Err(e)
                    if e.kind() == io::ErrorKind::BrokenPipe
                        || e.kind() == io::ErrorKind::ConnectionReset
                        || e.kind() == io::ErrorKind::ConnectionAborted =>
                {
                    tracing::info!("chalybsd: client disconnected during write: {e}");
                    clients.remove(j);
                }
                Err(e) => {
                    tracing::warn!("chalybsd: write error to client: {e}");
                    clients.remove(j);
                }
            }
        }

        // 7. Push-model heartbeat: fixed interval.
        thread::sleep(Duration::from_millis(250));
    }
}

/// Drain all available input from a single client, decoding any
/// newline-delimited JSON messages and updating the global
/// `pending_shell_events` and `pending_shell_commands` buffers.
///
/// Returns:
/// - Alive on success
/// - DisconnectedGraceful if the client closed the connection (EOF or
///   expected disconnection error kinds)
/// - DisconnectedError(_) if the client should be dropped due to a hard error
fn drain_client_commands(
    client: &mut Client,
    pending_shell_events: &mut Vec<IpcEvent>,
    pending_shell_commands: &mut Vec<String>,
) -> ClientDrainOutcome {
    let mut buf = [0_u8; 4096];

    loop {
        match client.stream.read(&mut buf) {
            Ok(0) => {
                // EOF: client closed connection cleanly.
                tracing::info!("chalybsd: client closed connection (EOF)");
                return ClientDrainOutcome::DisconnectedGraceful;
            }
            Ok(n) => {
                let chunk = match std::str::from_utf8(&buf[..n]) {
                    Ok(c) => c,
                    Err(e) => {
                        let err = Error::new(e).context("invalid UTF-8 from client");
                        return ClientDrainOutcome::DisconnectedError(err);
                    }
                };

                client.rx_buf.push_str(chunk);

                while let Some(idx) = client.rx_buf.find('\n') {
                    // Take an owned copy of the line slice to avoid
                    // borrow conflicts when draining from `rx_buf`.
                    let raw = client.rx_buf[..idx].to_string();
                    client.rx_buf.drain(..=idx);

                    let line = raw.trim();
                    if line.is_empty() {
                        continue;
                    }

                    match serde_json::from_str::<IpcMessage>(line) {
                        Ok(IpcMessage::ShellCommand { command }) => {
                            tracing::info!("chalybsd: shell command from TUI: {command}");

                            // Shell echo event so the TUI always shows the
                            // command the user typed.
                            pending_shell_events.push(IpcEvent {
                                kind: IpcEventKind::Shell,
                                message: format!("chalybs> {command}"),
                            });

                            // Defer semantic handling to the main loop, where
                            // we have access to DaemonState (config/inventory).
                            pending_shell_commands.push(command);
                        }
                        Ok(IpcMessage::Snapshot { .. }) => {
                            // We do not expect Snapshot from TUI; ignore.
                            tracing::warn!("chalybsd: unexpected Snapshot message from client");
                        }
                        Err(err) => {
                            tracing::error!(
                                "chalybsd: failed to decode IPC message from client: {err}"
                            );
                            pending_shell_events.push(IpcEvent {
                                kind: IpcEventKind::Error,
                                message: format!("daemon: invalid IPC message: {err}"),
                            });
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // No more data for now.
                break;
            }
            Err(e)
                if e.kind() == io::ErrorKind::ConnectionReset
                    || e.kind() == io::ErrorKind::ConnectionAborted =>
            {
                // These are expected when the client exits abruptly but
                // without protocol or data corruption. Treat as graceful.
                tracing::info!("chalybsd: client disconnected (read error: {e})");
                return ClientDrainOutcome::DisconnectedGraceful;
            }
            Err(e) => {
                let err = Error::new(e).context("read error from client");
                return ClientDrainOutcome::DisconnectedError(err);
            }
        }
    }

    ClientDrainOutcome::Alive
}

// ---------------------------------------------------------------------------
// Helper: configuration loading
// ---------------------------------------------------------------------------

fn load_root_config() -> (Option<RootConfig>, Vec<IpcEvent>) {
    let mut events = Vec::new();

    // 1) Environment override.
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
                tracing::warn!("{msg}");
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: msg,
                });
            }
        }
    }

    // 2) Default path.
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
                tracing::warn!("{msg}");
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: msg,
                });
                return (None, events);
            }
        }
    }

    // 3) No config at all.
    let msg = format!(
        "daemon: no config found (CHALYBS_CONFIG unset, {} missing)",
        DEFAULT_CONFIG_PATH
    );
    tracing::warn!("{msg}");
    events.push(IpcEvent {
        kind: IpcEventKind::Error,
        message: msg,
    });

    (None, events)
}

// ---------------------------------------------------------------------------
// Helper: VM introspection (config-only + full)
// ---------------------------------------------------------------------------

fn introspect_vm_config_only(vm_name: &str, vm: &VmConfig) -> Vec<IpcEvent> {
    let mut events = Vec::new();

    // CPU layout parsing.
    match parse_cpu_list(vm.cpu.vm_cpus.as_str()) {
        Ok(vcpus) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!(
                    "vm {vm_name}: parsed vm_cpus=\"{}\" ({} vCPUs)",
                    vm.cpu.vm_cpus,
                    vcpus.len()
                ),
            });
        }
        Err(e) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!(
                    "vm {vm_name}: failed to parse vm_cpus=\"{}\": {e}",
                    vm.cpu.vm_cpus
                ),
            });
        }
    }

    match parse_cpu_list(vm.cpu.host_cpus.as_str()) {
        Ok(hcpus) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!(
                    "vm {vm_name}: parsed host_cpus=\"{}\" ({} host CPUs)",
                    vm.cpu.host_cpus,
                    hcpus.len()
                ),
            });
        }
        Err(e) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!(
                    "vm {vm_name}: failed to parse host_cpus=\"{}\": {e}",
                    vm.cpu.host_cpus
                ),
            });
        }
    }

    // Isolation mode summary.
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

    // Device presence / basic classification.
    introspect_vm_devices(vm_name, vm, inv, &mut events);

    // GPU policy preflight (Phase 2/3, read-only).
    match preflight_gpu_policy(vm_name, vm) {
        Ok(()) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: "GPU policy preflight passed".to_string(),
            });
        }
        Err(e) => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!("GPU policy preflight failed: {e}"),
            });
        }
    }

    events
}

fn introspect_vm_devices(
    vm_name: &str,
    vm: &VmConfig,
    inv: &PciInventory,
    events: &mut Vec<IpcEvent>,
) {
    // GPU, NVMe, NIC, USB — basic "present vs missing" + simple type checks.
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
                // Basic sanity checks by kind.
                if let Some(kind) = kind {
                    if !device_kind_matches(func, kind) {
                        events.push(IpcEvent {
                            kind: IpcEventKind::Warning,
                            message: format!(
                                "vm {vm_name}: {kind_label} device {} present in inventory, but PCI class does not match expected {kind:?}",
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
            None => {
                events.push(IpcEvent {
                    kind: IpcEventKind::Warning,
                    message: format!(
                        "vm {vm_name}: optional {kind_label} device {} not found in PCI inventory",
                        dev.pci_address
                    ),
                });
            }
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

// ---------------------------------------------------------------------------
// Helper: shell command semantic handling (Phase 3A — intent-only)
// ---------------------------------------------------------------------------

fn handle_shell_command(state: &mut DaemonState, command: &str) -> Vec<IpcEvent> {
    let mut events = Vec::new();
    let line = command.trim();

    if line.is_empty() {
        return events;
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return events;
    }

    match parts[0] {
        "help" | "h" | "?" => {
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: "available commands: help | vm list | vm status <name> | vm start <name> | vm stop <name> | vm restart <name>".to_string(),
            });
            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: "note: Phase 3A is intent-only; daemon does not yet launch or stop VMs"
                    .to_string(),
            });
        }

        "vm" => handle_vm_command(state, &parts[1..], &mut events),

        other => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!("daemon: unknown command `{other}` (try `help`)"),
            });
        }
    }

    events
}

fn handle_vm_command(state: &mut DaemonState, args: &[&str], events: &mut Vec<IpcEvent>) {
    if args.is_empty() {
        events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: "usage: vm <list|status|start|stop|restart> [name]".to_string(),
        });
        return;
    }

    let Some(ref cfg) = state.root_cfg else {
        events.push(IpcEvent {
            kind: IpcEventKind::Error,
            message: "daemon: no config loaded; vm commands are unavailable".to_string(),
        });
        return;
    };

    match args[0] {
        "list" => {
            if cfg.vm.is_empty() {
                events.push(IpcEvent {
                    kind: IpcEventKind::Info,
                    message: "daemon: no VMs defined in configuration".to_string(),
                });
                return;
            }

            let mut names: Vec<&String> = cfg.vm.keys().collect();
            names.sort();

            let joined = names
                .into_iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");

            events.push(IpcEvent {
                kind: IpcEventKind::Info,
                message: format!("daemon: configured VMs: {joined}"),
            });
        }

        "status" => {
            if args.len() < 2 {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: "usage: vm status <name>".to_string(),
                });
                return;
            }

            let name = args[1];
            match cfg.vm.get(name) {
                Some(vm) => {
                    let isolation_mode = match vm.isolation.mode {
                        IsolationMode::Disabled => "disabled",
                        IsolationMode::Audit => "audit",
                        IsolationMode::Enforce => "enforce",
                    };

                    let gpu_count = vm.devices.gpu.as_ref().map(|v| v.len()).unwrap_or(0);
                    let nvme_count = vm.devices.nvme.as_ref().map(|v| v.len()).unwrap_or(0);
                    let nic_count = vm.devices.nic.as_ref().map(|v| v.len()).unwrap_or(0);
                    let usb_count = vm.devices.usb.as_ref().map(|v| v.len()).unwrap_or(0);

                    let view_state = state.vm_view_state(name);

                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {name}: state={view_state:?} (Phase 3A intent-only), isolation={isolation_mode}, hugepages={}",
                            vm.qemu.hugepages
                        ),
                    });

                    events.push(IpcEvent {
                        kind: IpcEventKind::Info,
                        message: format!(
                            "vm {name}: devices gpu={gpu_count}, nvme={nvme_count}, nic={nic_count}, usb={usb_count}"
                        ),
                    });
                }
                None => {
                    events.push(IpcEvent {
                        kind: IpcEventKind::Error,
                        message: format!("vm {name}: not found in configuration"),
                    });
                }
            }
        }

        "start" | "stop" | "restart" => {
            if args.len() < 2 {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!("usage: vm {} <name>", args[0]),
                });
                return;
            }

            let action = args[0];
            let name = args[1];

            if !cfg.vm.contains_key(name) {
                events.push(IpcEvent {
                    kind: IpcEventKind::Error,
                    message: format!("vm {name}: not found in configuration"),
                });
                return;
            }

            // Phase 3A is intent-only: we acknowledge the command and
            // update daemon-local view state, but do not actually
            // transition any core state or launch/stop QEMU.
            let new_state = match action {
                "start" | "restart" => IpcVmState::Starting,
                "stop" => IpcVmState::ShuttingDown,
                _ => IpcVmState::Stopped, // unreachable for current actions
            };

            state.set_vm_view_state(name, new_state);

            events.push(IpcEvent {
                kind: IpcEventKind::System,
                message: format!(
                    "vm {name}: `{action}` requested (Phase 3A: intent-only; VM state view set to {new_state:?})"
                ),
            });
        }

        other => {
            events.push(IpcEvent {
                kind: IpcEventKind::Error,
                message: format!(
                    "daemon: unknown vm subcommand `{other}` (expected list|status|start|stop|restart)"
                ),
            });
        }
    }
}
