// core/src/hugepages.rs

use std::fs;
use std::path::Path;

use tracing::{debug, info, warn};

use crate::config::NumaConfig;
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;
use crate::util::parse_cpu_list;

/// Determine which NUMA node to use for hugepage-backed RAM.
///
/// Priority:
///   1. vm.numa.hugepage_node (explicit override)
///   2. vm.numa.node (legacy NODE=2 semantics)
///   3. Detect from the host CPU set by intersecting with each
///      /sys/devices/system/node/nodeX/cpulist and choosing the node
///      with the largest overlap.
///
/// In your current config, this will resolve to node=2 and **must not
/// be changed** unless you explicitly change the TOML.
fn select_hugepage_node(rt: &VmRuntime) -> Result<u16> {
    // 1/2: explicit NUMA configuration.
    if let Some(ncfg) = &rt.cfg.numa {
        if let Some(n) = ncfg.hugepage_node.or(ncfg.node) {
            info!(
                vm = %rt.name,
                node = n,
                "hugepages: using NUMA node from config"
            );
            return Ok(n);
        }
    }

    // 3: derive from host CPU topology.
    let host_cpus = parse_cpu_list(rt.cfg.cpu.host_cpus.trim()).map_err(|e| {
        ChalybsError::Cpu(format!(
            "failed to parse host CPU list '{}' for hugepage NUMA detection: {e}",
            rt.cfg.cpu.host_cpus
        ))
    })?;

    let node_root = Path::new("/sys/devices/system/node");
    let mut best_node: Option<u16> = None;
    let mut best_overlap = 0usize;

    for entry in fs::read_dir(node_root).map_err(|e| {
        ChalybsError::Cpu(format!(
            "failed to list NUMA nodes in {}: {e}",
            node_root.display()
        ))
    })? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };

        if !name.starts_with("node") {
            continue;
        }

        let node_id: u16 = match name["node".len()..].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        let cpulist_path = entry.path().join("cpulist");
        let cpulist_str = match fs::read_to_string(&cpulist_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let node_cpus = match parse_cpu_list(cpulist_str.trim()) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let overlap = host_cpus
            .iter()
            .filter(|cpu| node_cpus.contains(cpu))
            .count();

        debug!(
            vm = %rt.name,
            node = node_id,
            overlap,
            "hugepages: node/CPU overlap candidate"
        );

        if overlap > best_overlap {
            best_overlap = overlap;
            best_node = Some(node_id);
        }
    }

    if let Some(node) = best_node {
        info!(
            vm = %rt.name,
            node,
            overlap = best_overlap,
            "hugepages: selected NUMA node via CPU topology"
        );
        Ok(node)
    } else {
        Err(ChalybsError::Cpu(
            "failed to determine NUMA node for hugepages (no overlap between host CPUs and NUMA nodes)"
                .into(),
        ))
    }
}

/// Provision hugepages for the VM's RAM on a specific NUMA node.
///
/// - If qemu.hugepages = false → no-op.
/// - Otherwise:
///     * pick node (config/topology)
///     * compute required 2MiB pages
///     * write /sys/devices/system/node/nodeX/hugepages/hugepages-2048kB/nr_hugepages
///     * record the outcome in VmRuntime
pub fn provision_for_vm(rt: &mut VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    if !q.hugepages {
        info!(
            vm = %rt.name,
            "hugepages: disabled in config; skipping hugepage provisioning"
        );
        return Ok(());
    }

    let node = select_hugepage_node(rt)?;

    // 2MiB hugepages: exactly the same size your Bash suite used:
    //   RAM * 1024 / 2  (GiB → MiB → /2MiB)
    let hugepage_size_bytes: u64 = 2 * 1024 * 1024;

    let mem_bytes = q.mem_mb * 1024 * 1024;
    let pages = (mem_bytes + hugepage_size_bytes - 1) / hugepage_size_bytes;

    let hp_path = Path::new("/sys/devices/system/node")
        .join(format!("node{node}"))
        .join("hugepages")
        .join("hugepages-2048kB")
        .join("nr_hugepages");

    let current: u64 = fs::read_to_string(&hp_path)
        .map_err(|e| {
            ChalybsError::Cpu(format!(
                "hugepages: failed to read {}: {e}",
                hp_path.display()
            ))
        })?
        .trim()
        .parse()
        .map_err(|e| {
            ChalybsError::Cpu(format!(
                "hugepages: failed to parse current hugepage count from {}: {e}",
                hp_path.display()
            ))
        })?;

    if current != pages {
        info!(
            vm = %rt.name,
            node,
            current,
            target = pages,
            path = %hp_path.display(),
            "hugepages: adjusting node-local hugepage count"
        );

        fs::write(&hp_path, pages.to_string()).map_err(|e| {
            ChalybsError::Cpu(format!(
                "hugepages: failed to write {}: {e}",
                hp_path.display()
            ))
        })?;
    } else {
        info!(
            vm = %rt.name,
            node,
            pages,
            "hugepages: node already has required hugepage count"
        );
    }

    let total_bytes = pages * hugepage_size_bytes;

    rt.hugepages_node = Some(node);
    rt.hugepages_pages = pages;
    rt.hugepages_bytes = total_bytes;
    rt.hugepages_active = true;

    rt.push_info(format!(
        "hugepages: provisioned {} x 2MiB pages ({:.1} GiB) on NUMA node {}",
        pages,
        total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        node
    ));

    Ok(())
}
