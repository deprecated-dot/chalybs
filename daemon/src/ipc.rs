// daemon/src/ipc.rs

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

/// Host/VM CPU layout as exposed over IPC.
///
/// This is derived from config (vm.cpu.host_cpus / vm.cpu.vm_cpus) and is
/// independent of runtime pinning state. The TUI can use this for topology
/// views without needing to understand internal CpuPlan structures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmCpuLayout {
    pub host: Vec<u32>,
    pub vm: Vec<u32>,
}

/// Detailed hugepage backing information for a VM.
///
/// This is the view-projection of the core `VmRuntime` hugepage fields and
/// is intended for display only (no policy baked in).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmHugepages {
    /// Whether hugepages are *currently* active for this VM.
    pub active: bool,
    /// NUMA node chosen by the hugepage provisioning phase, if any.
    pub node: Option<u16>,
    /// Number of 2MiB hugepages provisioned.
    pub pages: u64,
    /// Total bytes of hugepage-backed memory.
    pub bytes: u64,
}

/// Raw vCPU usage sample for a VM.
///
/// Semantics:
///   - Each Snapshot may carry at most one sample for the VM.
///   - `vcpu_indices[i]` corresponds to `vcpu_jiffies[i]`.
///   - `vcpu_jiffies` is the sum of utime+stime (jiffies) for that vCPU
///     thread at the time the snapshot was built.
///   - The TUI is responsible for computing deltas between snapshots and
///     turning them into per-vCPU or aggregate "load" percentages.
///
/// This keeps the daemon side deterministic and transparent: it only
/// reports what the vCPU threads have actually done, not inferred load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmCpuSample {
    pub vcpu_indices: Vec<u32>,
    pub vcpu_jiffies: Vec<u64>,
}

/// VM status as exposed over IPC.
///
/// This is intentionally a reduced, view-oriented representation of a VM.
/// It is *not* the same as `VmRuntime` and should remain decoupled from
/// internal process handles, cgroup paths, etc.
///
/// In addition to coarse state and flags, this now serves as the unified
/// "projection model" for the TUI: both the VM list and any detail/expanded
/// views should be derived from this structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcVmStatus {
    pub name: String,
    pub state: IpcVmState,
    pub cpu_pinned: bool,
    pub irq_pinned: bool,
    pub tasmota_on: bool,
    pub isolation_mode: String,

    /// Legacy boolean "hugepages enabled" from config.
    /// Kept for backwards compatibility with simpler UIs.
    pub hugepages: bool,

    /// Config/runtime-derived CPU layout (host and VM CPU lists).
    pub cpu_layout: IpcVmCpuLayout,

    /// Detailed hugepage backing information for this VM.
    pub hugepages_detail: IpcVmHugepages,

    /// Optional raw vCPU usage snapshot for this VM.
    ///
    /// Present when the VM is Running and QEMU vCPU threads can be
    /// discovered; absent otherwise.
    pub cpu_sample: Option<IpcVmCpuSample>,
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
