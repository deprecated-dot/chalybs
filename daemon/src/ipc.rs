#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// IPC types shared between `chalybsd` and frontend clients (e.g. the TUI).
///
/// These types form the serialized protocol over the Unix-domain socket using
/// newline-delimited JSON. The intent is to keep the surface small, stable,
/// and view-oriented rather than mirroring internal runtime types directly.

/// VM state as exposed over IPC.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum IpcVmState {
    Stopped,
    Starting,
    Running,
    ShuttingDown,
}

/// VM status as exposed over IPC.
///
/// This is intentionally a reduced, view-oriented representation of a VM.
/// It is *not* the same as `VmRuntime` and should remain decoupled from
/// internal process handles, cgroup paths, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmStatus {
    pub name: String,
    pub state: IpcVmState,
    pub cpu_pinned: bool,
    pub irq_pinned: bool,
    pub tasmota_on: bool,
    pub isolation_mode: String,
    pub hugepages: bool,
}

/// Event severity / kind emitted by the daemon.
///
/// This mirrors the semantics used by the TUI's `AppEventKind`, but is kept
/// local to the IPC boundary so it can evolve independently if needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcEventKind {
    Info,
    Warning,
    Error,
    Shell,
    System,
}

/// A single line in the daemon's event stream as exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEvent {
    pub kind: IpcEventKind,
    pub message: String,
}

/// Per-VM events as exposed over IPC.
///
/// This associates a VM name with a list of events specific to that VM.
/// The VM name must match one of the `IpcVmStatus.name` entries in the
/// same snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmEvents {
    pub vm_name: String,
    pub events: Vec<IpcEvent>,
}

/// Top-level IPC message envelope.
///
/// This is the unit that is serialized on the wire between `chalybsd` and
/// clients. The framing is newline-delimited JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcMessage {
    /// TUI → Daemon: a shell command issued by the user.
    ShellCommand { command: String },

    /// Daemon → TUI: a snapshot of current VM statuses and associated events.
    ///
    /// `events_global`:
    ///   - Daemon lifecycle
    ///   - Global system/IPC events
    ///   - Shell command echoes and responses
    ///
    /// `events_vm`:
    ///   - Events scoped to individual VMs (e.g., isolation findings,
    ///     pinning status, PCI/VFIO changes).
    Snapshot {
        vms: Vec<IpcVmStatus>,
        events_global: Vec<IpcEvent>,
        events_vm: Vec<IpcVmEvents>,
    },
}
