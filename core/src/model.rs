use std::process::Child;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::config::{VmConfig, ModeConfig};

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

/// Detected host capabilities relevant to scheduling and policy.
///
/// This is intentionally conservative for now:
/// - Detects NUMA nodes via /sys/devices/system/node
/// - Can be extended later with GPU count, IOMMU layout, etc.
#[derive(Debug, Clone, Serialize)]
pub struct HostCapabilities {
    /// List of NUMA node ids present on the system, e.g. [0, 1, 2, 3].
    pub numa_nodes: Vec<u16>,
}

impl HostCapabilities {
    /// Best-effort detection of host capabilities.
    /// This never fails; on error it simply reports fewer capabilities.
    pub fn detect() -> Self {
        let mut numa_nodes = Vec::new();
        let nodes_dir = Path::new("/sys/devices/system/node");

        if let Ok(entries) = fs::read_dir(nodes_dir) {
            for entry_res in entries {
                let entry = match entry_res {
                    Ok(e) => e,
                    Err(_) => continue,
                };

                let name = match entry.file_name().into_string() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                if let Some(rest) = name.strip_prefix("node") {
                    if let Ok(id) = rest.parse::<u16>() {
                        numa_nodes.push(id);
                    }
                }
            }
            numa_nodes.sort_unstable();
            numa_nodes.dedup();
        }

        HostCapabilities { numa_nodes }
    }

    /// Convenience helper: returns true if this looks like a NUMA system.
    pub fn is_numa(&self) -> bool {
        !self.numa_nodes.is_empty()
    }
}

/// Effective modes for a particular VM, after combining:
/// - VM config (ModeConfig)
/// - HostCapabilities
///
/// These are what the state machine and modules should consult when
/// deciding how to behave.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveModes {
    /// Single-GPU mode: VM temporarily claims the only GPU and returns
    /// it to the host afterward.
    pub single_gpu: bool,

    /// Dedicated GPU mode: the VM’s GPU is considered permanently
    /// dedicated to the guest (no rebind to host).
    pub dedicated_gpu: bool,

    /// NUMA-aware mode: enable NUMA-sensitive placement of CPUs, IRQs
    /// and memory. On NUMA hosts, this typically defaults to true.
    pub numa_aware: bool,

    /// Systems hygiene around launch (drop caches, reclaim, etc.).
    pub reset_on_launch: bool,

    /// DDC input switching around VM launch.
    pub ddc_control: bool,

    /// Tasmota/external power control.
    pub tasmota_power: bool,
}

impl EffectiveModes {
    /// Resolve effective modes for a VM given:
    /// - vm_name (for logging / future heuristics)
    /// - the VM's ModeConfig
    /// - host capabilities
    ///
    /// Policy (for now):
    /// - numa_aware: defaults to true if host is NUMA, false otherwise.
    /// - reset_on_launch: defaults to true (cheap safety).
    /// - others default to false unless explicitly enabled in config.
    pub fn resolve(vm_name: &str, cfg: &VmConfig, caps: &HostCapabilities) -> Self {
        let ModeConfig {
            single_gpu,
            dedicated_gpu,
            numa_aware,
            reset_on_launch,
            ddc_control,
            tasmota_power,
        } = cfg.modes.clone();

        let host_is_numa = caps.is_numa();

        // NUMA awareness: auto-enable on NUMA hosts unless explicitly disabled.
        let numa_aware = numa_aware.unwrap_or(host_is_numa);

        // For now, default reset_on_launch to true (cheap, safe) unless disabled.
        let reset_on_launch = reset_on_launch.unwrap_or(true);

        // For now, we do not try to automatically infer single_gpu vs dedicated_gpu
        // from PCI layout; these are explicit configuration knobs.
        let single_gpu = single_gpu.unwrap_or(false);
        let dedicated_gpu = dedicated_gpu.unwrap_or(false);

        let ddc_control = ddc_control.unwrap_or(false);
        let tasmota_power = tasmota_power.unwrap_or(false);

        // Future: debug logging here once we want trace-level introspection,
        // e.g. tracing::debug!(vm = vm_name, ?caps, ?cfg.modes, ?modes, "resolved VM modes");

        EffectiveModes {
            single_gpu,
            dedicated_gpu,
            numa_aware,
            reset_on_launch,
            ddc_control,
            tasmota_power,
        }
    }
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

    /// Detected host capabilities (NUMA, etc.).
    pub caps: HostCapabilities,

    /// Effective modes for this VM (single_gpu, dedicated, numa_aware...).
    pub modes: EffectiveModes,
}

impl VmRuntime {
    pub fn new(name: String, cfg: VmConfig, cpus: VmCpuLayout) -> Self {
        let caps = HostCapabilities::detect();
        let modes = EffectiveModes::resolve(&name, &cfg, &caps);

        Self {
            name,
            cfg,
            cpus,
            cgroups: None,
            qemu: None,
            pinned_threads: false,
            pinned_irqs: false,
            caps,
            modes,
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
