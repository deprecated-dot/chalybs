use std::process::Child;

use serde::Serialize;

use crate::config::VmConfig;

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

    /// VFIO driver transitions performed while staging PCI devices for
    /// this VM. Used during shutdown to restore original bindings.
    pub vfio_transitions: Vec<VfioTransition>,
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
            vfio_transitions: Vec::new(),
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
}
