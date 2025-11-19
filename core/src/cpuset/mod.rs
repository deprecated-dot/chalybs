mod status;
pub use status::cpuset_status;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::errors::{ChalybsError, Result};
use crate::model::{CgroupPaths, VmRuntime};
use crate::util::parse_cpu_list;

/// Internal representation of NUMA topology.
#[derive(Debug, Clone)]
struct NumaTopology {
    /// Map: node_id -> CPUs on that node
    node_cpus: BTreeMap<u32, Vec<u32>>,
    /// All online CPUs for the system
    online_cpus: Vec<u32>,
}

/// Discover NUMA topology from /sys/devices/system/node.
///
/// If no NUMA nodes are present, this collapses to a single node (0)
/// containing all online CPUs.
fn discover_numa_topology() -> Result<NumaTopology> {
    let node_root = Path::new("/sys/devices/system/node");

    // Always get the online CPU list.
    let online_str =
        fs::read_to_string("/sys/devices/system/cpu/online")?;
    let mut online_cpus = parse_cpu_list(online_str.trim())?;
    online_cpus.sort_unstable();
    online_cpus.dedup();

    let mut node_cpus = BTreeMap::<u32, Vec<u32>>::new();

    if node_root.is_dir() {
        for entry in fs::read_dir(node_root)? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    debug!("failed to read NUMA node entry: {e}");
                    continue;
                }
            };

            let name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };

            if !name.starts_with("node") {
                continue;
            }

            let node_id_str = &name[4..];
            let node_id: u32 = match node_id_str.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            let cpulist_path = entry.path().join("cpulist");
            if !cpulist_path.exists() {
                continue;
            }

            let cpulist_str = fs::read_to_string(&cpulist_path)?;
            let mut cpus = parse_cpu_list(cpulist_str.trim())?;
            cpus.sort_unstable();
            cpus.dedup();

            node_cpus.insert(node_id, cpus);
        }
    }

    // If no node_* directories were discovered, treat as single-node.
    if node_cpus.is_empty() {
        node_cpus.insert(0, online_cpus.clone());
    }

    Ok(NumaTopology {
        node_cpus,
        online_cpus,
    })
}

/// Given a topology and a list of CPUs, return the set of NUMA nodes
/// that contain any of those CPUs.
fn nodes_for_cpus(topo: &NumaTopology, cpus: &[u32]) -> BTreeSet<u32> {
    let mut nodes = BTreeSet::new();
    let cpu_set: BTreeSet<u32> = cpus.iter().copied().collect();

    for (node_id, node_cpus) in &topo.node_cpus {
        if node_cpus.iter().any(|c| cpu_set.contains(c)) {
            nodes.insert(*node_id);
        }
    }

    nodes
}

/// Utility: format a Vec<u32> as a simple comma-separated list, e.g. "0,1,2,3".
fn format_cpu_list(cpus: &[u32]) -> String {
    let mut v = cpus.to_vec();
    v.sort_unstable();
    v.dedup();
    v.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Utility: format a set of node IDs as "0,2,3".
fn format_node_list(nodes: &BTreeSet<u32>) -> String {
    nodes
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Public helper used by the CLI to derive host_cpus when the config does
/// not specify them explicitly.
///
/// Implements C2:
/// - If NUMA nodes are present:
///     host_cpus = all CPUs on nodes NOT used by vm_cpus
/// - If single-node:
///     host_cpus = online_cpus - vm_cpus
pub fn derive_host_cpus_from_topology(vm_cpus: &[u32]) -> Result<Vec<u32>> {
    if vm_cpus.is_empty() {
        return Err(ChalybsError::Cgroup(
            "cannot derive host_cpus from empty vm_cpus".into(),
        ));
    }

    let topo = discover_numa_topology()?;
    let vm_nodes = nodes_for_cpus(&topo, vm_cpus);

    if vm_nodes.is_empty() {
        return Err(ChalybsError::Cgroup(
            "no NUMA nodes found for vm_cpus; topology inconsistent".into(),
        ));
    }

    // Identify host nodes: all nodes minus vm_nodes.
    let mut host_nodes: BTreeSet<u32> =
        topo.node_cpus.keys().copied().collect();
    for n in &vm_nodes {
        host_nodes.remove(n);
    }

    let host_cpus: Vec<u32> = if !host_nodes.is_empty() {
        // NUMA-aware complement: all CPUs on nodes not used by vm_cpus.
        let mut v = Vec::new();
        for n in &host_nodes {
            if let Some(cpus) = topo.node_cpus.get(n) {
                v.extend_from_slice(cpus);
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    } else {
        // Fallback: single-node or vm consumes all nodes.
        // Derive host_cpus as online - vm_cpus.
        let vm_set: BTreeSet<u32> = vm_cpus.iter().copied().collect();
        let mut v = Vec::new();
        for c in topo.online_cpus {
            if !vm_set.contains(&c) {
                v.push(c);
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    };

    if host_cpus.is_empty() {
        return Err(ChalybsError::Cgroup(
            "derived host_cpus is empty (vm_cpus may consume all CPUs)".into(),
        ));
    }

    Ok(host_cpus)
}

/// cpuset preflight — verify that the cgroup v2 root is present.
pub fn preflight(_rt: &VmRuntime) -> Result<()> {
    let root = Path::new("/sys/fs/cgroup");
    if !root.is_dir() {
        return Err(ChalybsError::Cgroup(
            "/sys/fs/cgroup is not a directory or not mounted".into(),
        ));
    }
    Ok(())
}

/// Create/ensure the cpusets for vfio_vm and vfio_host, and write
/// cpuset.cpus / cpuset.mems for each based on the runtime CPU layout.
///
/// This function is NUMA-aware:
/// - vm mems are the NUMA nodes that contain vm_cpus
/// - host mems are the NUMA nodes that contain host_cpus
pub fn create_cpuset(rt: &mut VmRuntime) -> Result<()> {
    let root = PathBuf::from("/sys/fs/cgroup");
    let vm_path = root.join("vfio_vm");
    let host_path = root.join("vfio_host");

    fs::create_dir_all(&vm_path)?;
    fs::create_dir_all(&host_path)?;

    let topo = discover_numa_topology()?;

    let vm_cpus = &rt.cpus.vm.cpus;
    let host_cpus = &rt.cpus.host.cpus;

    if vm_cpus.is_empty() {
        return Err(ChalybsError::Cgroup(
            "vm cpuset is empty; cannot create vfio_vm cpuset".into(),
        ));
    }
    if host_cpus.is_empty() {
        return Err(ChalybsError::Cgroup(
            "host cpuset is empty; cannot create vfio_host cpuset".into(),
        ));
    }

    let vm_nodes = nodes_for_cpus(&topo, vm_cpus);
    let host_nodes = nodes_for_cpus(&topo, host_cpus);

    if vm_nodes.is_empty() {
        return Err(ChalybsError::Cgroup(
            "no NUMA nodes found for vm_cpus when creating cpuset".into(),
        ));
    }
    if host_nodes.is_empty() {
        return Err(ChalybsError::Cgroup(
            "no NUMA nodes found for host_cpus when creating cpuset".into(),
        ));
    }

    let vm_cpus_str = format_cpu_list(vm_cpus);
    let host_cpus_str = format_cpu_list(host_cpus);
    let vm_mems_str = format_node_list(&vm_nodes);
    let host_mems_str = format_node_list(&host_nodes);

    fs::write(vm_path.join("cpuset.cpus"), vm_cpus_str.as_bytes())?;
    fs::write(vm_path.join("cpuset.mems"), vm_mems_str.as_bytes())?;
    fs::write(host_path.join("cpuset.cpus"), host_cpus_str.as_bytes())?;
    fs::write(host_path.join("cpuset.mems"), host_mems_str.as_bytes())?;

    info!(
        vm_cpus = %vm_cpus_str,
        vm_mems = %vm_mems_str,
        host_cpus = %host_cpus_str,
        host_mems = %host_mems_str,
        "configured vfio_vm and vfio_host cpusets"
    );

    // Record cgroup paths in the runtime so QEMU launch can move the
    // process into the vm cpuset.
    rt.cgroups = Some(CgroupPaths {
        root: root.clone(),
        vm: vm_path,
        host: host_path,
    });

    Ok(())
}

/// cpuset teardown — currently non-destructive.
///
/// We deliberately *do not* remove the vfio_vm / vfio_host directories
/// or reset cpuset.cpus/mems here, to avoid surprising the user’s
/// system configuration. This can be extended later if desired.
pub fn destroy_cpuset(_rt: &mut VmRuntime) -> Result<()> {
    // No-op for now.
    Ok(())
}
