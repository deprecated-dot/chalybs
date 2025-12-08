use std::cell::Cell;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use crate::config::VmConfig;
use crate::cpuplan::CpuPlan;

/// Core-level event kind for VM-specific lifecycle activity.
///
/// This is intentionally small and decoupled from any particular logging
/// backend. It can be projected into daemon/TUI IPC events or used for
/// internal diagnostics.
#[derive(Debug, Clone, Serialize)]
pub enum CoreEventKind {
    Info,
    Warning,
    Error,
    System,
}

/// A single VM-scoped event emitted during the VM lifecycle.
///
/// These events are attached to `VmRuntime` and are intended to reflect
/// major lifecycle milestones (PCI/VFIO transitions, cpuset changes,
/// QEMU launch, pinning, isolation findings, shutdown, etc.).
#[derive(Debug, Clone, Serialize)]
pub struct CoreEvent {
    pub kind: CoreEventKind,
    pub message: String,
}

/// Simple representation of a CPU list: Vec<u32>
#[derive(Debug, Clone, Serialize)]
pub struct CpuSet {
    pub cpus: Vec<u32>,
}

/// Aggregate VM/Host CPU layout
#[derive(Debug, Clone, Serialize)]
pub struct VmCpuLayout {
    pub host: CpuSet,
    pub vm: CpuSet,
}

/// Cgroup directory paths for cpuset
#[derive(Debug, Clone)]
pub struct CgroupPaths {
    pub root: std::path::PathBuf,
    pub vm: std::path::PathBuf,
    pub host: std::path::PathBuf,
}

/// QEMU runtime state (pid + child handle)
#[derive(Debug)]
pub struct QemuState {
    pub pid: i32,
    pub child: Child,
}

/// VFIO transition record for a single PCI device.
///
/// This captures the *original* driver binding before Chalybs staged
/// the device for passthrough (i.e., before binding it to vfio-pci).
#[derive(Debug, Clone)]
pub struct VfioTransition {
    /// PCI BDF, e.g. "0000:0b:00.0".
    pub bdf: String,
    /// Original bound driver name, if any (e.g. "amdgpu", "nvidia").
    /// None means the device was originally unbound.
    pub from_driver: Option<String>,
    /// IOMMU group id at the time of staging, if known. This is used
    /// only for diagnostics and future ACS / domain-aware behavior.
    pub iommu_group: Option<u32>,
}

/// Unified runtime object passed through state machine
#[derive(Debug)]
pub struct VmRuntime {
    pub name: String,
    pub cfg: VmConfig,     // Entire VM config
    pub cpus: VmCpuLayout, // Parsed CPU layout
    pub cgroups: Option<CgroupPaths>,
    pub qemu: Option<QemuState>,

    pub pinned_threads: bool,
    pub pinned_irqs: bool,

    /// Completion flag for asynchronous IRQ pinning.
    ///
    /// This is set to `true` exactly once by the IRQ worker when it has
    /// successfully discovered and pinned IRQs for all eligible devices.
    /// It is never reset by core; consumers treat it as a latch.
    pub irq_pinning_complete: Arc<AtomicBool>,

    /// VFIO driver transitions performed while staging PCI devices for
    /// this VM. Used during shutdown to restore original bindings.
    pub vfio_transitions: Vec<VfioTransition>,

    /// VM-scoped lifecycle events accumulated during execution.
    ///
    /// This is the per-VM half of the hybrid event model; the daemon
    /// maintains its own global event bus for system-level activity.
    /// These events are intended to be deterministic and ordered with
    /// respect to the VM state machine.
    pub events: Vec<CoreEvent>,

    /// Deterministic hugepage backing record.
    ///
    /// When `qemu.hugepages = true`, the hugepage provisioning phase
    /// will:
    ///   - pick a NUMA node (config / topology)
    ///   - provision the required number of 2MiB hugepages
    ///   - record the outcome here for later inspection / TUI display.
    pub hugepages_node: Option<u16>,
    pub hugepages_pages: u64,
    pub hugepages_bytes: u64,
    pub hugepages_active: bool,

    /// Immutable, validated CPU plan for this VM, if available.
    ///
    /// This is constructed during the Validate phase from:
    ///   - host CPUID identity
    ///   - host NUMA topology
    ///   - VM CPU layout (host/vm sets)
    ///
    /// and is used by current and future validation layers to enforce
    /// NUMA/CPU invariants deterministically.
    pub cpu_plan: Option<CpuPlan>,

    /// Best-known Tasmota relay state for this VM.
    ///
    /// This is updated by the Tasmota peripheral hook based on the
    /// outcome of MQTT POWER publishes and is consumed by the daemon
    /// when building IPC snapshots.
    pub tasmota_powered: Cell<bool>,
}

impl VmRuntime {
    pub fn new(name: String, cfg: VmConfig, cpus: VmCpuLayout) -> Self {
        Self {
            name,
            cfg,
            cpus,
            cgroups: None,
            qemu: None,
            pinned_threads: false,
            pinned_irqs: false,
            irq_pinning_complete: Arc::new(AtomicBool::new(false)),
            vfio_transitions: Vec::new(),
            events: Vec::new(),
            hugepages_node: None,
            hugepages_pages: 0,
            hugepages_bytes: 0,
            hugepages_active: false,
            cpu_plan: None,
            tasmota_powered: Cell::new(false),
        }
    }

    /// Helper to set cpuset cgroup paths after creation
    pub fn set_cgroups(&mut self, root: impl Into<std::path::PathBuf>) {
        let root = root.into();
        self.cgroups = Some(CgroupPaths {
            root: root.clone(),
            vm: root.join("vfio_vm"),
            host: root.join("vfio_host"),
        });
    }

    /// Append a VM-scoped event with explicit kind and message.
    ///
    /// This is intentionally minimal; richer helpers (for specific phases
    /// or subsystems) can be layered on top without changing the shape of
    /// `CoreEvent` or `VmRuntime`.
    pub fn push_event(&mut self, kind: CoreEventKind, message: impl Into<String>) {
        self.events.push(CoreEvent {
            kind,
            message: message.into(),
        });
    }

    /// Convenience helper for informational events.
    pub fn push_info(&mut self, message: impl Into<String>) {
        self.push_event(CoreEventKind::Info, message);
    }

    /// Convenience helper for warning events.
    pub fn push_warning(&mut self, message: impl Into<String>) {
        self.push_event(CoreEventKind::Warning, message);
    }

    /// Convenience helper for error events (semantic, not necessarily
    /// fatal errors in the state machine).
    pub fn push_error(&mut self, message: impl Into<String>) {
        self.push_event(CoreEventKind::Error, message);
    }

    /// Convenience helper for system/structural VM events (e.g. VM
    /// created, entering steady state, starting shutdown).
    pub fn push_system(&mut self, message: impl Into<String>) {
        self.push_event(CoreEventKind::System, message);
    }
}
