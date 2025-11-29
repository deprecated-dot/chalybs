use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chalybsd::ipc::{IpcEvent, IpcEventKind, IpcMessage, IpcVmState, IpcVmStatus};
use serde_json;

use crate::config::{LogoHaloProfile, TuiConfig};

/// Overall health state of the daemon from the TUI's perspective.
///
/// This is intentionally coarse and human-facing:
/// - Healthy: daemon reachable, no recent serious complaints
/// - Degraded: daemon reachable but we've seen recent errors/warnings
/// - Disconnected: daemon not reachable / IPC closed
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonHealth {
    Disconnected,
    Degraded,
    Healthy,
}

/// High-level application state used by the TUI.
///
/// This is intentionally backend-agnostic; it only stores
/// data required for rendering and simple interactions.
pub struct App {
    pub running: bool,

    pub vms: Vec<VmStatus>,
    pub selected_vm: usize,

    pub events: Vec<AppEvent>,

    pub shell_input: String,
    pub shell_history: Vec<String>,

    pub tick_count: u64,
    pub last_tick: Instant,

    /// When true, the events panel stops auto-following
    /// the newest entries. The user can scroll manually.
    pub events_scroll_locked: bool,
    /// Offset from the newest event when scroll-locked.
    /// 0 = follow newest, N = show older events.
    pub events_scroll_offset: usize,

    /// When true, show a VM detail modal overlay.
    pub vm_detail_open: bool,

    /// Visual effect toggles for the TUI. These default to ON and can be
    /// adjusted at runtime via `effects` shell commands. Persistence is
    /// handled externally (e.g. via global config / env); this type is
    /// purely an in-memory description.
    pub effects: VisualEffects,

    /// Current notion of daemon health for UI purposes.
    pub daemon_health: DaemonHealth,
    /// Number of consecutive "clean" event batches used for auto-heal.
    daemon_health_clean_runs: u8,
}

/// Simple VM status snapshot.
///
/// In the future this will be derived from the real chalybsd
/// control-plane, but for now it's populated by `ChalybsBackend`.
#[derive(Clone, Debug)]
pub struct VmStatus {
    pub name: String,
    pub state: VmState,
    pub cpu_pinned: bool,
    pub irq_pinned: bool,
    pub tasmota_on: bool,
    pub isolation_mode: String,
    pub hugepages: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmState {
    Stopped,
    Starting,
    Running,
    ShuttingDown,
}

/// Visual effect configuration for the TUI.
///
/// All effects default to ON. Users can toggle them at runtime
/// via the `effects` shell command. Persistence (if any) is
/// handled *outside* this type; we no longer touch XDG / $HOME
/// or write any TUI-local config files.
///
/// The halo profile here is the **canonical Set C** enum shared
/// with `tui::config` and `tui::logo_png`.
#[derive(Clone, Debug)]
pub struct VisualEffects {
    pub pulse: bool,
    pub scanlines: bool,
    pub matrix: bool,
    pub border_noise: bool,
    pub badges: bool,
    pub logo_reactive: bool,
    pub load_index: bool,

    /// Chalybs logo halo profile used by the PNG renderer.
    pub logo_halo: LogoHaloProfile,
}

impl VisualEffects {
    /// Default effect set: everything enabled + halo profile = C3.
    ///
    /// This is the TUI's built-in default when no config/env is present.
    pub fn default_enabled() -> Self {
        Self {
            pulse: true,
            scanlines: true,
            matrix: true,
            border_noise: true,
            badges: true,
            logo_reactive: true,
            load_index: true,
            logo_halo: LogoHaloProfile::Off,
        }
    }

    /// Enable / disable all flags at once (halo profile is left unchanged).
    pub fn set_all(&mut self, value: bool) {
        self.pulse = value;
        self.scanlines = value;
        self.matrix = value;
        self.border_noise = value;
        self.badges = value;
        self.logo_reactive = value;
        self.load_index = value;
    }

    /// Set an individual flag by textual name (boolean toggles only).
    ///
    /// Accepted names:
    ///   pulse, scanlines, matrix, border, border_noise,
    ///   badges, logo, logo_reactive, load, load_index
    ///
    /// Halo profile is handled separately via:
    ///   `effects halo <...>`
    pub fn set_flag(&mut self, name: &str, value: bool) {
        match name {
            "pulse" => self.pulse = value,
            "scanlines" => self.scanlines = value,
            "matrix" => self.matrix = value,
            "border" | "border_noise" => self.border_noise = value,
            "badges" => self.badges = value,
            "logo" | "logo_reactive" => self.logo_reactive = value,
            "load" | "load_index" => self.load_index = value,
            _ => {}
        }
    }
}

/// Event severity / kind for the middle "events" column.
#[derive(Clone, Debug)]
pub enum AppEventKind {
    Info,
    Warning,
    Error,
    Shell,
    System,
}

/// A single line in the events stream.
#[derive(Clone, Debug)]
pub struct AppEvent {
    pub kind: AppEventKind,
    pub message: String,
}

/// Abstraction for "chalybsd" interaction.
///
/// We implement both a mock backend and a daemon backend.
/// The trait is the stable surface the TUI uses to:
/// - refresh VM status
/// - poll events
/// - handle shell commands
pub trait ChalybsBackend {
    fn initial_vms(&self) -> Vec<VmStatus>;

    /// Called periodically from `App::on_tick`.
    fn refresh_status(&mut self, vms: &mut [VmStatus]);

    /// Return any newly available events for the event stream.
    fn poll_events(&mut self) -> Vec<AppEvent>;

    /// Submit a shell command and return any resulting events.
    fn handle_shell_command(&mut self, command: &str) -> Vec<AppEvent>;
}

/// Allow boxed trait objects to be used wherever a `ChalybsBackend` is
/// expected. This enables patterns like `Box<dyn ChalybsBackend>` to work
/// seamlessly with the generic `App` methods.
impl<T: ChalybsBackend + ?Sized> ChalybsBackend for Box<T> {
    fn initial_vms(&self) -> Vec<VmStatus> {
        (**self).initial_vms()
    }

    fn refresh_status(&mut self, vms: &mut [VmStatus]) {
        (**self).refresh_status(vms)
    }

    fn poll_events(&mut self) -> Vec<AppEvent> {
        (**self).poll_events()
    }

    fn handle_shell_command(&mut self, command: &str) -> Vec<AppEvent> {
        (**self).handle_shell_command(command)
    }
}

/// A simple mock backend used while the real control-plane
/// integration is being developed.
pub struct MockBackend {
    tick: u64,
}

impl MockBackend {
    pub fn new() -> Self {
        Self { tick: 0 }
    }
}

impl ChalybsBackend for MockBackend {
    fn initial_vms(&self) -> Vec<VmStatus> {
        vec![
            VmStatus {
                name: "win11-gpu".to_string(),
                state: VmState::Running,
                cpu_pinned: true,
                irq_pinned: true,
                tasmota_on: true,
                isolation_mode: "enforce".to_string(),
                hugepages: true,
            },
            VmStatus {
                name: "linux-lab".to_string(),
                state: VmState::Stopped,
                cpu_pinned: false,
                irq_pinned: false,
                tasmota_on: false,
                isolation_mode: "audit".to_string(),
                hugepages: false,
            },
            VmStatus {
                name: "baremetal-sim".to_string(),
                state: VmState::Running,
                cpu_pinned: true,
                irq_pinned: false,
                tasmota_on: false,
                isolation_mode: "disabled".to_string(),
                hugepages: true,
            },
        ]
    }

    fn refresh_status(&mut self, vms: &mut [VmStatus]) {
        // Deterministic, very simple "heartbeat" just to make
        // the UI feel alive without being noisy.
        self.tick = self.tick.wrapping_add(1);

        if self.tick % 32 == 0 {
            // Flip the Tasmota state for the first VM as a demo.
            if let Some(vm) = vms.get_mut(0) {
                vm.tasmota_on = !vm.tasmota_on;
            }
        }

        if self.tick % 64 == 0 {
            // Toggle irq_pinned for the third VM.
            if let Some(vm) = vms.get_mut(2) {
                vm.irq_pinned = !vm.irq_pinned;
            }
        }
    }

    fn poll_events(&mut self) -> Vec<AppEvent> {
        let mut out = Vec::new();

        if self.tick == 1 {
            out.push(AppEvent {
                kind: AppEventKind::System,
                message: "chalybs-tui: attached to mock backend".to_string(),
            });
        }

        if self.tick % 40 == 0 {
            out.push(AppEvent {
                kind: AppEventKind::Info,
                message: format!("heartbeat: tick={}", self.tick),
            });
        }

        if self.tick % 90 == 0 {
            out.push(AppEvent {
                kind: AppEventKind::Warning,
                message: "win11-gpu: GPU isolation in audit-only mode (mock)".to_string(),
            });
        }

        out
    }

    fn handle_shell_command(&mut self, command: &str) -> Vec<AppEvent> {
        let command = command.trim();
        if command.is_empty() {
            return Vec::new();
        }

        let mut events = Vec::new();

        events.push(AppEvent {
            kind: AppEventKind::Shell,
            message: format!("chalybs> {command}"),
        });

        match command {
            "help" | "?" => {
                events.push(AppEvent {
                    kind: AppEventKind::Info,
                    message: "mock backend: available commands: help, list, status, clear"
                        .to_string(),
                });
            }
            "list" => {
                events.push(AppEvent {
                    kind: AppEventKind::Info,
                    message: "mock backend: VMs = win11-gpu, linux-lab, baremetal-sim".to_string(),
                });
            }
            "status" => {
                events.push(AppEvent {
                    kind: AppEventKind::Info,
                    message: "mock backend: PCI phases 1–8 nominal (simulated)".to_string(),
                });
            }
            "clear" => {
                events.push(AppEvent {
                    kind: AppEventKind::System,
                    message: "mock backend: UI event buffer clear requested (not yet wired)"
                        .to_string(),
                });
            }
            other => {
                events.push(AppEvent {
                    kind: AppEventKind::Error,
                    message: format!(
                        "mock backend: `{other}` is not a recognized command (try `help`)"
                    ),
                });
            }
        }

        events
    }
}

/// Daemon-backed Chalybs backend using Unix-domain sockets.
///
/// This implementation uses newline-delimited JSON messages following the
/// IPC types defined in `chalybsd::ipc`. It operates in a push-model: the
/// daemon periodically sends `Snapshot` messages, which we consume during
/// `refresh_status`.
pub struct DaemonBackend {
    stream: UnixStream,
    read_buffer: String,
    pending_events: Vec<AppEvent>,
    last_snapshot_vms: Vec<VmStatus>,
    connected: bool,
}

impl DaemonBackend {
    fn primary_socket() -> &'static str {
        "/run/chalybsd.sock"
    }

    fn fallback_socket() -> &'static str {
        "/tmp/chalybsd.sock"
    }

    fn choose_socket_path() -> PathBuf {
        let run = Path::new("/run");
        if run.is_dir() {
            PathBuf::from(Self::primary_socket())
        } else {
            PathBuf::from(Self::fallback_socket())
        }
    }

    /// Attempt to connect to the default chalybsd IPC endpoint and wait
    /// for the initial Snapshot message.
    pub fn connect_default() -> Result<Self, String> {
        let path = Self::choose_socket_path();

        let stream = UnixStream::connect(&path)
            .map_err(|e| format!("failed to connect to {}: {e}", path.display()))?;

        stream
            .set_nonblocking(true)
            .map_err(|e| format!("failed to set nonblocking on {}: {e}", path.display()))?;

        let mut backend = DaemonBackend {
            stream,
            read_buffer: String::new(),
            pending_events: Vec::new(),
            last_snapshot_vms: Vec::new(),
            connected: true,
        };

        // Wait deterministically for the first snapshot, with a bounded timeout.
        let deadline = Instant::now() + Duration::from_secs(2);

        loop {
            if Instant::now() >= deadline {
                return Err("timed out waiting for initial daemon snapshot".to_string());
            }

            if let Err(err) = backend.drain_socket() {
                return Err(format!("error while waiting for initial snapshot: {err}"));
            }

            if !backend.last_snapshot_vms.is_empty() {
                break;
            }

            std::thread::sleep(Duration::from_millis(50));
        }

        Ok(backend)
    }

    /// Internal helper: read any available data from the socket, parse
    /// complete JSON lines, and update `last_snapshot_vms` + `pending_events`.
    fn drain_socket(&mut self) -> Result<(), String> {
        if !self.connected {
            return Ok(());
        }

        let mut buf = [0_u8; 4096];

        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => {
                    self.connected = false;
                    self.pending_events.push(AppEvent {
                        kind: AppEventKind::System,
                        message: "chalybs-tui: daemon closed IPC connection".to_string(),
                    });
                    break;
                }
                Ok(n) => {
                    let chunk = std::str::from_utf8(&buf[..n])
                        .map_err(|e| format!("invalid UTF-8 from daemon: {e}"))?;
                    self.read_buffer.push_str(chunk);

                    while let Some(idx) = self.read_buffer.find('\n') {
                        let line = self.read_buffer[..idx].trim().to_string();
                        self.read_buffer.drain(..=idx);

                        if line.is_empty() {
                            continue;
                        }

                        match serde_json::from_str::<IpcMessage>(&line) {
                            Ok(IpcMessage::Snapshot {
                                vms,
                                events_global,
                                events_vm,
                            }) => {
                                self.last_snapshot_vms =
                                    vms.into_iter().map(Self::map_vm_status).collect();

                                // Flatten global + per-VM events into a single stream.
                                let mut mapped_events: Vec<AppEvent> = Vec::new();

                                // Global events.
                                mapped_events
                                    .extend(events_global.into_iter().map(Self::map_event_global));

                                // Per-VM events: prefix message with VM name.
                                for vm_ev in events_vm.into_iter() {
                                    let vm_name = vm_ev.vm_name;
                                    for ev in vm_ev.events.into_iter() {
                                        mapped_events.push(Self::map_event_for_vm(&vm_name, ev));
                                    }
                                }

                                self.pending_events.extend(mapped_events);
                            }
                            Ok(IpcMessage::ShellCommand { .. }) => {
                                // Daemon should not send ShellCommand to TUI;
                                // treat as a soft protocol violation.
                                self.pending_events.push(AppEvent {
                                    kind: AppEventKind::Warning,
                                    message: "chalybs-tui: unexpected ShellCommand from daemon"
                                        .into(),
                                });
                            }
                            Err(err) => {
                                self.pending_events.push(AppEvent {
                                    kind: AppEventKind::Error,
                                    message: format!(
                                        "chalybs-tui: failed to decode IPC message: {err}"
                                    ),
                                });
                            }
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    break;
                }
                Err(e) => {
                    self.connected = false;
                    self.pending_events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message: format!("chalybs-tui: daemon IPC error: {e}"),
                    });
                    break;
                }
            }
        }

        Ok(())
    }

    fn map_vm_status(src: IpcVmStatus) -> VmStatus {
        VmStatus {
            name: src.name,
            state: match src.state {
                IpcVmState::Stopped => VmState::Stopped,
                IpcVmState::Starting => VmState::Starting,
                IpcVmState::Running => VmState::Running,
                IpcVmState::ShuttingDown => VmState::ShuttingDown,
            },
            cpu_pinned: src.cpu_pinned,
            irq_pinned: src.irq_pinned,
            tasmota_on: src.tasmota_on,
            isolation_mode: src.isolation_mode,
            hugepages: src.hugepages,
        }
    }

    fn map_event_global(src: IpcEvent) -> AppEvent {
        AppEvent {
            kind: match src.kind {
                IpcEventKind::Info => AppEventKind::Info,
                IpcEventKind::Warning => AppEventKind::Warning,
                IpcEventKind::Error => AppEventKind::Error,
                IpcEventKind::Shell => AppEventKind::Shell,
                IpcEventKind::System => AppEventKind::System,
            },
            message: src.message,
        }
    }

    fn map_event_for_vm(vm_name: &str, src: IpcEvent) -> AppEvent {
        let base = Self::map_event_global(src);
        AppEvent {
            kind: base.kind,
            message: format!("{vm_name}: {}", base.message),
        }
    }

    #[allow(dead_code)]
    fn map_event(src: IpcEvent) -> AppEvent {
        // Kept for potential reuse; currently unused.
        Self::map_event_global(src)
    }
}

impl ChalybsBackend for DaemonBackend {
    fn initial_vms(&self) -> Vec<VmStatus> {
        self.last_snapshot_vms.clone()
    }

    fn refresh_status(&mut self, vms: &mut [VmStatus]) {
        if let Err(err) = self.drain_socket() {
            self.pending_events.push(AppEvent {
                kind: AppEventKind::Error,
                message: format!("chalybs-tui: IPC drain error: {err}"),
            });
        }

        if !self.last_snapshot_vms.is_empty() {
            if vms.len() == self.last_snapshot_vms.len() {
                for (dst, src) in vms.iter_mut().zip(self.last_snapshot_vms.iter()) {
                    *dst = src.clone();
                }
            } else {
                // Length mismatch: this violates the fixed-VM-count assumption.
                // We do NOT try to resize the slice; instead, surface a warning
                // and leave the current view unchanged.
                self.pending_events.push(AppEvent {
                    kind: AppEventKind::Warning,
                    message: format!(
                        "chalybs-tui: daemon snapshot VM count {} != local VM count {} (ignoring)",
                        self.last_snapshot_vms.len(),
                        vms.len()
                    ),
                });
            }
        }
    }

    fn poll_events(&mut self) -> Vec<AppEvent> {
        if self.pending_events.is_empty() {
            Vec::new()
        } else {
            let mut out = Vec::new();
            std::mem::swap(&mut out, &mut self.pending_events);
            out
        }
    }

    fn handle_shell_command(&mut self, command: &str) -> Vec<AppEvent> {
        let command = command.trim();
        if command.is_empty() || !self.connected {
            return Vec::new();
        }

        let msg = IpcMessage::ShellCommand {
            command: command.to_string(),
        };

        match serde_json::to_string(&msg) {
            Ok(json) => {
                if let Err(e) = self.stream.write_all(json.as_bytes()) {
                    self.pending_events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message: format!("chalybs-tui: failed to send shell command: {e}"),
                    });
                } else if let Err(e) = self.stream.write_all(b"\n") {
                    self.pending_events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message: format!("chalybs-tui: failed to send newline: {e}"),
                    });
                }
            }
            Err(e) => {
                self.pending_events.push(AppEvent {
                    kind: AppEventKind::Error,
                    message: format!("chalybs-tui: failed to encode shell command: {e}"),
                });
            }
        }

        Vec::new()
    }
}

impl App {
    pub fn new(initial_vms: Vec<VmStatus>) -> Self {
        Self {
            running: true,
            vms: initial_vms,
            selected_vm: 0,
            events: Vec::new(),
            shell_input: String::new(),
            shell_history: Vec::new(),
            tick_count: 0,
            last_tick: Instant::now(),
            events_scroll_locked: false,
            events_scroll_offset: 0,
            vm_detail_open: false,
            // Start with in-memory defaults; config/env can adjust
            // halo and other effects at runtime via `effects` command.
            effects: VisualEffects::default_enabled(),
            daemon_health: DaemonHealth::Disconnected,
            daemon_health_clean_runs: 0,
        }
    }

    pub fn on_tick<B: ChalybsBackend>(&mut self, backend: &mut B) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.last_tick = Instant::now();
        backend.refresh_status(&mut self.vms);
        let new_events = backend.poll_events();
        self.push_events(new_events);
    }

    /// Update daemon_health based on new events (Option A-ish heuristic).
    fn update_daemon_health(&mut self, new_events: &[AppEvent]) {
        if new_events.is_empty() {
            // No new information; slowly let clean_runs accumulate
            // only when we're not disconnected.
            if !matches!(self.daemon_health, DaemonHealth::Disconnected) {
                self.daemon_health_clean_runs = self.daemon_health_clean_runs.saturating_add(1);
                if self.daemon_health == DaemonHealth::Degraded
                    && self.daemon_health_clean_runs >= 6
                {
                    self.daemon_health = DaemonHealth::Healthy;
                }
            }
            return;
        }

        let mut saw_daemon_error_or_warn = false;
        let mut saw_disconnect = false;

        for evt in new_events {
            let msg = evt.message.as_str();

            // Hard disconnect signals.
            if msg.contains("daemon unavailable")
                || msg.contains("daemon closed IPC connection")
                || msg.contains("failed to connect to")
            {
                saw_disconnect = true;
                break;
            }

            if matches!(evt.kind, AppEventKind::Error | AppEventKind::Warning)
                && (msg.contains("chalybsd") || msg.contains("daemon") || msg.contains("IPC"))
            {
                saw_daemon_error_or_warn = true;
            }
        }

        if saw_disconnect {
            self.daemon_health = DaemonHealth::Disconnected;
            self.daemon_health_clean_runs = 0;
            return;
        }

        if saw_daemon_error_or_warn {
            if self.daemon_health == DaemonHealth::Healthy {
                self.daemon_health = DaemonHealth::Degraded;
            }
            self.daemon_health_clean_runs = 0;
        } else {
            if !matches!(self.daemon_health, DaemonHealth::Disconnected) {
                self.daemon_health_clean_runs = self.daemon_health_clean_runs.saturating_add(1);
                if self.daemon_health == DaemonHealth::Degraded
                    && self.daemon_health_clean_runs >= 6
                {
                    self.daemon_health = DaemonHealth::Healthy;
                }
            }
        }
    }

    pub fn push_events(&mut self, new_events: Vec<AppEvent>) {
        // Update health first, while we still have a slice.
        self.update_daemon_health(&new_events);

        if new_events.is_empty() {
            return;
        }

        self.events.extend(new_events);

        const MAX_EVENTS: usize = 256;
        if self.events.len() > MAX_EVENTS {
            let overflow = self.events.len() - MAX_EVENTS;
            self.events.drain(0..overflow);
        }

        // When not scroll-locked, always auto-follow newest.
        if !self.events_scroll_locked {
            self.events_scroll_offset = 0;
        }
    }

    pub fn push_shell_char(&mut self, c: char) {
        self.shell_input.push(c);
    }

    pub fn pop_shell_char(&mut self) {
        self.shell_input.pop();
    }

    pub fn clear_shell_input(&mut self) {
        self.shell_input.clear();
    }

    /// Handle submission of a shell command from the prompt.
    ///
    /// The flow is:
    ///   1. If the command is a TUI-local directive (e.g. `effects ...`),
    ///      handle it locally and DO NOT forward it to the backend.
    ///   2. Otherwise, forward to the backend and append any events
    ///      it returns.
    pub fn submit_shell_command<B: ChalybsBackend>(&mut self, backend: &mut B) {
        // Take ownership of the current input to avoid aliasing `self.shell_input`.
        let raw = std::mem::take(&mut self.shell_input);
        let cmd = raw.trim().to_string();
        if cmd.is_empty() {
            return;
        }

        // First, check for local commands.
        if let Some(events) = self.handle_local_command(&cmd) {
            self.shell_history.push(cmd);
            self.push_events(events);
            return;
        }

        // Forward to backend.
        self.shell_history.push(cmd.clone());
        let events = backend.handle_shell_command(&cmd);
        self.push_events(events);
    }

    /// TUI-local command handling.
    ///
    /// Right now this covers `effects` and related subcommands.
    ///
    /// Returns Some(events) if the command was recognized and handled
    /// locally, or None if it should be forwarded to the backend.
    fn handle_local_command(&mut self, cmd: &str) -> Option<Vec<AppEvent>> {
        let mut parts = cmd.split_whitespace();
        let first = parts.next()?;

        if first != "effects" {
            return None;
        }

        let mut events = Vec::new();

        let sub = parts.next();

        match sub.unwrap_or("status") {
            "status" => {
                let halo = self.effects.logo_halo.as_str();
                events.push(AppEvent {
                    kind: AppEventKind::Info,
                    message: format!(
                        "effects: pulse={}, scanlines={}, matrix={}, border_noise={}, badges={}, logo_reactive={}, load_index={}, halo={}",
                        self.effects.pulse,
                        self.effects.scanlines,
                        self.effects.matrix,
                        self.effects.border_noise,
                        self.effects.badges,
                        self.effects.logo_reactive,
                        self.effects.load_index,
                        halo,
                    ),
                });
            }
            "on" | "off" => {
                let enable = sub == Some("on");
                self.effects.set_all(enable);
                events.push(AppEvent {
                    kind: AppEventKind::System,
                    message: format!("effects: all={}", if enable { "on" } else { "off" }),
                });
            }
            "halo" => {
                let value_word = parts.next();
                let Some(value_word) = value_word else {
                    events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message:
                            "effects: usage: effects halo <off|c3|c3narrow|c3wide|c3extrawide>"
                                .to_string(),
                    });
                    return Some(events);
                };

                match LogoHaloProfile::from_str_kind(value_word) {
                    Some(profile) => {
                        self.effects.logo_halo = profile;
                        events.push(AppEvent {
                            kind: AppEventKind::System,
                            message: format!("effects: halo={}", profile.as_str()),
                        });
                    }
                    None => {
                        events.push(AppEvent {
                            kind: AppEventKind::Error,
                            message: format!(
                                "effects: unknown halo profile `{}` (expected: off|c3|c3narrow|c3wide|c3extrawide)",
                                value_word
                            ),
                        });
                    }
                }
            }
            field => {
                let value_word = parts.next();
                let Some(value_word) = value_word else {
                    events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message: "effects: usage: effects <pulse|scanlines|matrix|border|badges|logo|load> <on|off> or effects halo <off|c3|c3narrow|c3wide|c3extrawide>".to_string(),
                    });
                    return Some(events);
                };

                let value = match value_word {
                    "on" => Some(true),
                    "off" => Some(false),
                    _ => None,
                };

                let Some(value) = value else {
                    events.push(AppEvent {
                        kind: AppEventKind::Error,
                        message: "effects: expected `on` or `off`".to_string(),
                    });
                    return Some(events);
                };

                // Apply flag.
                self.effects.set_flag(field, value);
                events.push(AppEvent {
                    kind: AppEventKind::System,
                    message: format!("effects: {field}={}", if value { "on" } else { "off" }),
                });
            }
        }

        Some(events)
    }

    pub fn select_next_vm(&mut self) {
        if self.vms.is_empty() {
            return;
        }
        self.selected_vm = (self.selected_vm + 1).min(self.vms.len() - 1);
    }

    pub fn select_previous_vm(&mut self) {
        if self.vms.is_empty() {
            return;
        }
        if self.selected_vm > 0 {
            self.selected_vm -= 1;
        }
    }

    /// Lock the events panel so it stops auto-following.
    pub fn lock_events_scroll(&mut self) {
        self.events_scroll_locked = true;
    }

    /// Unlock the events panel and jump back to newest.
    pub fn unlock_events_scroll(&mut self) {
        self.events_scroll_locked = false;
        self.events_scroll_offset = 0;
    }

    /// Scroll the events up (toward older entries) by `lines`.
    pub fn scroll_events_up(&mut self, lines: usize) {
        if self.events.is_empty() {
            return;
        }
        // Offset counts how far back from the newest entry we are.
        self.events_scroll_offset = self
            .events_scroll_offset
            .saturating_add(lines)
            .min(self.events.len().saturating_sub(1));
    }

    /// Scroll the events down (toward newer entries) by `lines`.
    pub fn scroll_events_down(&mut self, lines: usize) {
        if self.events.is_empty() {
            return;
        }
        self.events_scroll_offset = self.events_scroll_offset.saturating_sub(lines);
    }

    /// Toggle the VM detail modal.
    pub fn toggle_vm_detail(&mut self) {
        self.vm_detail_open = !self.vm_detail_open;
    }

    /// Access the currently selected VM, if any.
    pub fn selected_vm(&self) -> Option<&VmStatus> {
        self.vms.get(self.selected_vm)
    }
}

/// Helper to construct the App + MockBackend pair.
///
/// This is preserved as-is so tests or other callers can force the
/// mock backend explicitly if desired.
pub fn create_mock_app() -> (App, MockBackend) {
    let backend = MockBackend::new();
    let initial_vms = backend.initial_vms();
    let mut app = App::new(initial_vms);

    // If a global TUI config exists, apply halo profile at startup
    // while leaving all other effects at their defaults.
    if let Some(cfg) = TuiConfig::load() {
        app.effects.logo_halo = cfg.logo_halo;
    }

    (app, backend)
}

/// Helper to construct the App + backend pair used by `main`,
/// automatically selecting the backend.
///
/// This version matches the signature used in `main.rs`:
///
///     let tui_config = TuiConfig::load();
///     let (mut app, mut backend) = create_app_autodetect(tui_config);
///
/// The config is accepted and used only to seed the initial halo
/// profile; behavior is otherwise identical.
pub fn create_app_autodetect(tui_config: Option<TuiConfig>) -> (App, Box<dyn ChalybsBackend>) {
    match DaemonBackend::connect_default() {
        Ok(daemon) => {
            let mut app = App::new(daemon.initial_vms());

            if let Some(cfg) = tui_config {
                app.effects.logo_halo = cfg.logo_halo;
            }

            app.daemon_health = DaemonHealth::Healthy;
            app.daemon_health_clean_runs = 0;
            app.push_events(vec![AppEvent {
                kind: AppEventKind::System,
                message: "chalybs-tui: connected to daemon backend".to_string(),
            }]);
            (app, Box::new(daemon))
        }
        Err(err) => {
            let (mut app, mock) = create_mock_app();

            if let Some(cfg) = tui_config {
                app.effects.logo_halo = cfg.logo_halo;
            }

            app.daemon_health = DaemonHealth::Disconnected;
            app.daemon_health_clean_runs = 0;
            app.push_events(vec![AppEvent {
                kind: AppEventKind::System,
                message: format!("chalybs-tui: daemon unavailable ({err}); using mock backend"),
            }]);
            (app, Box::new(mock))
        }
    }
}
