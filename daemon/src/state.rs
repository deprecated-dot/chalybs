use crate::ipc::{IpcEvent, IpcEventKind, IpcVmEvents, IpcVmState, IpcVmStatus};

/// Daemon-side state and snapshot builder.
///
/// Right now this is a synthetic backend that produces a small set of
/// deterministic VMs and events on every tick. The public API is
/// intentionally small so we can swap the internals out for real
/// Chalybs core integration later without touching the server loop.
#[derive(Debug)]
pub struct DaemonState {
    tick: u64,
}

impl DaemonState {
    /// Create a fresh daemon state.
    pub fn new() -> Self {
        Self { tick: 0 }
    }

    /// Advance the daemon state by one tick and build a snapshot view
    /// suitable for sending over IPC to the TUI.
    ///
    /// Returns:
    /// - `vms`:          list of VM status rows
    /// - `events_global`:global daemon/system events
    /// - `events_vm`:    per-VM scoped events
    pub fn next_snapshot(
        &mut self,
    ) -> (Vec<IpcVmStatus>, Vec<IpcEvent>, Vec<IpcVmEvents>) {
        self.tick = self.tick.wrapping_add(1);

        let vms = synthetic_vms(self.tick);
        let events_global = synthetic_events_global(self.tick);
        let events_vm = synthetic_events_vm(self.tick);

        (vms, events_global, events_vm)
    }
}

// ---------------------------------------------------------------------
// Synthetic backend helpers (Phase 0).
//
// These are a straight move of the previous synthetic_* functions out
// of server.rs so the server loop talks to a single state object.
// ---------------------------------------------------------------------

fn synthetic_vms(tick: u64) -> Vec<IpcVmStatus> {
    let mut vms = vec![
        IpcVmStatus {
            name: "win11-gpu".to_string(),
            state: IpcVmState::Running,
            cpu_pinned: true,
            irq_pinned: true,
            tasmota_on: true,
            isolation_mode: "enforce".to_string(),
            hugepages: true,
        },
        IpcVmStatus {
            name: "linux-lab".to_string(),
            state: IpcVmState::Stopped,
            cpu_pinned: false,
            irq_pinned: false,
            tasmota_on: false,
            isolation_mode: "audit".to_string(),
            hugepages: false,
        },
        IpcVmStatus {
            name: "baremetal-sim".to_string(),
            state: IpcVmState::Running,
            cpu_pinned: true,
            irq_pinned: false,
            tasmota_on: false,
            isolation_mode: "disabled".to_string(),
            hugepages: true,
        },
    ];

    // Simple deterministic toggles to keep UI "alive".
    if tick % 32 == 0 {
        vms[0].tasmota_on = !vms[0].tasmota_on;
    }

    if tick % 64 == 0 {
        vms[2].irq_pinned = !vms[2].irq_pinned;
    }

    vms
}

/// Global (non-VM-specific) events.
fn synthetic_events_global(tick: u64) -> Vec<IpcEvent> {
    let mut events = Vec::new();

    if tick == 1 {
        events.push(IpcEvent {
            kind: IpcEventKind::System,
            message: "chalybsd: attached to TUI client(s) (synthetic backend)".to_string(),
        });
    }

    if tick % 40 == 0 {
        events.push(IpcEvent {
            kind: IpcEventKind::Info,
            message: format!("daemon heartbeat: tick={tick}"),
        });
    }

    events
}

/// Per-VM events in the synthetic backend.
///
/// In the future these will be driven directly from the real Chalybs
/// core state machine and PCI/VFIO lifecycle.
fn synthetic_events_vm(tick: u64) -> Vec<IpcVmEvents> {
    let mut out = Vec::new();

    if tick % 90 == 0 {
        out.push(IpcVmEvents {
            vm_name: "win11-gpu".to_string(),
            events: vec![IpcEvent {
                kind: IpcEventKind::Warning,
                message: "GPU isolation in audit-only mode (synthetic)".to_string(),
            }],
        });
    }

    out
}
