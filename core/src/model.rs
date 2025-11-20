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

/// Unified runtime object passed through state machine
#[derive(Debug)]
pub struct VmRuntime {
    pub name: String,
    pub cfg: VmConfig,         // Entire VM config
    pub cpus: VmCpuLayout,     // Parsed CPU layout
    pub cgroups: Option<CgroupPaths>,
    pub qemu: Option<QemuState>,

    pub pinned_threads: bool,
    pub pinned_irqs: bool,
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
