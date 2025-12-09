// daemon/src/snapshot.rs

//! Read-only helpers for daemon snapshots.
//!
//! This module is intentionally **pure** and **side-effect free**.
//!
//! Invariants / contract:
//! - `DaemonState::build_snapshot` remains the **only** authority for
//!   how snapshots are constructed, ordered, and timed.
//! - This module does **not**:
//!     - drive state machines
//!     - touch QEMU processes
//!     - mutate `DaemonState`
//!     - perform sliding-window / heuristic “load” calculations
//! - It only provides small, deterministic helpers for consumers that
//!   already have a snapshot in hand and want to inspect or project it.
//!
//! The intent is to keep this a safe place to grow richer read-only
//! views (NUMA, cpusets, vCPU counters, hugepage info, etc.) without
//! ever risking drift in the core daemon control flow.

#![allow(dead_code)]

use crate::ipc::{
    IpcEvent, IpcVmCpuLayout, IpcVmCpuSample, IpcVmEvents, IpcVmHugepages, IpcVmStatus,
};

/// Lightweight read-only view over a daemon snapshot.
///
/// This wraps the triple that `DaemonState::build_snapshot` produces:
///
/// ```text
/// (Vec<IpcVmStatus>, Vec<IpcEvent>, Vec<IpcVmEvents>)
/// ```
///
/// and exposes helper methods to access VMs and events without forcing
/// any particular ownership model on the caller.
///
/// Typical usage pattern (inside tests, logging, or diagnostics):
///
/// ```ignore
/// let (vms, events_global, events_vm) = daemon_state.build_snapshot(tick);
/// let snapshot = DaemonSnapshot::new(&vms, &events_global, &events_vm);
///
/// if let Some(vm) = snapshot.vm_by_name("win11-gpu") {
///     let layout = vm_cpu_layout(vm);
///     let hp     = vm_hugepages(vm);
///     let sample = vm_cpu_sample(vm);
/// }
/// ```
pub struct DaemonSnapshot<'a> {
    vms: &'a [IpcVmStatus],
    events_global: &'a [IpcEvent],
    events_vm: &'a [IpcVmEvents],
}

impl<'a> DaemonSnapshot<'a> {
    /// Construct a new view from the raw snapshot slices.
    ///
    /// The caller owns the underlying vectors; this type only borrows.
    pub fn new(
        vms: &'a [IpcVmStatus],
        events_global: &'a [IpcEvent],
        events_vm: &'a [IpcVmEvents],
    ) -> Self {
        Self {
            vms,
            events_global,
            events_vm,
        }
    }

    /// All VMs in this snapshot, in the order provided by the daemon.
    ///
    /// Ordering is defined by `DaemonState::build_snapshot` and is
    /// currently a stable, lexicographically-sorted VM name list.
    pub fn vms(&self) -> &'a [IpcVmStatus] {
        self.vms
    }

    /// Iterate over all VMs.
    pub fn iter_vms(&self) -> impl Iterator<Item = &'a IpcVmStatus> {
        self.vms.iter()
    }

    /// Find a VM by its name, if present in this snapshot.
    pub fn vm_by_name(&self, name: &str) -> Option<&'a IpcVmStatus> {
        self.vms.iter().find(|vm| vm.name == name)
    }

    /// Global events attached to this snapshot.
    ///
    /// These already include:
    /// - daemon startup/config/PCI events (on early ticks)
    /// - shell echoes and parser errors
    /// - semantic shell events (`vm list`, `vm status`, etc.)
    /// - periodic heartbeat messages
    pub fn events_global(&self) -> &'a [IpcEvent] {
        self.events_global
    }

    /// Per-VM event batches attached to this snapshot.
    ///
    /// Each entry is a single VM name + its new events for this tick.
    pub fn events_vm(&self) -> &'a [IpcVmEvents] {
        self.events_vm
    }

    /// Look up per-VM events for a specific VM by name.
    ///
    /// Returns a slice view; if the VM has no new events in this
    /// snapshot, returns an empty slice.
    pub fn events_for_vm(&self, name: &str) -> &'a [IpcEvent] {
        self.events_vm
            .iter()
            .find(|batch| batch.vm_name == name)
            .map(|batch| batch.events.as_slice())
            .unwrap_or(&[])
    }
}

/// Accessors for the CPU layout part of the IPC model.
///
/// These are simple pass-through helpers that keep the projection
/// surface explicit and avoid callers needing to know about the
/// internal field layout of `IpcVmStatus`.

/// Get the CPU layout (host + VM CPUs) for a VM.
pub fn vm_cpu_layout(vm: &IpcVmStatus) -> &IpcVmCpuLayout {
    &vm.cpu_layout
}

/// Convenience: host CPU list for this VM.
///
/// This is derived from config and does **not** encode runtime pinning
/// state. It is safe to use as a topology hint only.
pub fn vm_host_cpus(vm: &IpcVmStatus) -> &[u32] {
    &vm.cpu_layout.host
}

/// Convenience: vCPU index list for this VM (config side).
///
/// This is the logical vCPU index space as defined in config, not
/// necessarily the set of vCPUs for which we have runtime usage
/// samples.
pub fn vm_vcpus(vm: &IpcVmStatus) -> &[u32] {
    &vm.cpu_layout.vm
}

/// Accessors for hugepage backing detail.
///
/// Again, these are thin wrappers around fields on `IpcVmStatus` and
/// do not attempt to smuggle in policy or heuristics.

/// Get the hugepage detail structure for a VM.
pub fn vm_hugepages(vm: &IpcVmStatus) -> &IpcVmHugepages {
    &vm.hugepages_detail
}

/// Return `true` if the VM is currently backed by hugepages.
///
/// This reflects the daemon/runtime’s view at snapshot time and is
/// independent of whether the config requested hugepages.
pub fn vm_hugepages_active(vm: &IpcVmStatus) -> bool {
    vm.hugepages_detail.active
}

/// Raw vCPU usage sample helpers.
///
/// These are deliberately minimal and avoid any conversion into
/// percentages or time-windowed “load” metrics. The daemon’s contract
/// is to expose raw jiffies; clients (TUI, external tools) are
/// responsible for differencing samples over time.

/// Get the raw CPU sample for a VM, if present.
///
/// A sample is typically present only when the VM is Running and QEMU
/// vCPU threads have been discovered by the daemon.
pub fn vm_cpu_sample(vm: &IpcVmStatus) -> Option<&IpcVmCpuSample> {
    vm.cpu_sample.as_ref()
}

/// Convenience: return vCPU index + jiffies pairs as a small Vec.
///
/// This is a thin clone of the underlying sample and exists to keep
/// callers from needing to reason about parallel `vcpu_indices` and
/// `vcpu_jiffies` arrays.
///
/// The ordering is the same as provided by the daemon (which is
/// already sorted by vCPU index).
pub fn vm_vcpu_jiffies(vm: &IpcVmStatus) -> Option<Vec<(u32, u64)>> {
    let sample = vm.cpu_sample.as_ref()?;

    // Defensive: enforce the positional pairing contract even if a
    // future change accidentally mis-sizes the arrays.
    let len = sample
        .vcpu_indices
        .len()
        .min(sample.vcpu_jiffies.len());

    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        out.push((sample.vcpu_indices[i], sample.vcpu_jiffies[i]));
    }

    Some(out)
}

/// Convenience: total jiffies across all sampled vCPUs.
///
/// This is still a *raw counter*, not a percentage or a rate. It can be
/// differenced across snapshots by the caller to derive a per-VM load
/// metric if desired.
pub fn vm_total_jiffies(vm: &IpcVmStatus) -> Option<u64> {
    let sample = vm.cpu_sample.as_ref()?;
    let len = sample
        .vcpu_indices
        .len()
        .min(sample.vcpu_jiffies.len());

    let mut total = 0u64;
    for i in 0..len {
        total = total.saturating_add(sample.vcpu_jiffies[i]);
    }
    Some(total)
}
