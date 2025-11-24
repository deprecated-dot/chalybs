use std::time::Instant;

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
/// Right now we only implement a mock backend, but the trait
/// is the stable surface the TUI uses to:
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
        }
    }

    pub fn on_tick<B: ChalybsBackend>(&mut self, backend: &mut B) {
        self.tick_count = self.tick_count.wrapping_add(1);
        self.last_tick = Instant::now();
        backend.refresh_status(&mut self.vms);
        let new_events = backend.poll_events();
        self.push_events(new_events);
    }

    pub fn push_events(&mut self, new_events: Vec<AppEvent>) {
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

    pub fn submit_shell_command<B: ChalybsBackend>(&mut self, backend: &mut B) {
        let cmd = self.shell_input.trim();
        if cmd.is_empty() {
            return;
        }

        self.shell_history.push(cmd.to_string());
        let events = backend.handle_shell_command(cmd);
        self.push_events(events);
        self.shell_input.clear();
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

/// Helper to construct the App + MockBackend pair used by `main`.
pub fn create_mock_app() -> (App, MockBackend) {
    let backend = MockBackend::new();
    let initial_vms = backend.initial_vms();
    let app = App::new(initial_vms);
    (app, backend)
}
