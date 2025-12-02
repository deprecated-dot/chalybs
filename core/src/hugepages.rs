// core/src/hugepages.rs

use std::fs;
use std::path::Path;

use tracing::{debug, info};

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
///
/// This helper is read-only and does not modify sysfs.
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
        ChalybsError::Other(format!(
            "hugepages: failed to parse host CPU list '{}' for NUMA detection: {e}",
            rt.cfg.cpu.host_cpus
        ))
    })?;

    let node_root = Path::new("/sys/devices/system/node");
    let mut best_node: Option<u16> = None;
    let mut best_overlap = 0usize;

    for entry in fs::read_dir(node_root).map_err(|e| {
        ChalybsError::Other(format!(
            "hugepages: failed to list NUMA nodes in {}: {e}",
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
        Err(ChalybsError::Other(
            "hugepages: failed to determine NUMA node (no overlap between host CPUs and NUMA nodes)"
                .into(),
        ))
    }
}

/// Hugepage bring-up semantics (Phase 12, read-only):
///
/// - If qemu.hugepages = false → no-op (logs and returns Ok).
/// - If qemu.hugepages = true:
///     * pick node (config/topology)
///     * compute required 2MiB pages from mem_mb
///     * read node-local nr_hugepages and free_hugepages
///     * **verify** that the node has at least `pages` total and free
///       hugepages available
///     * record the expectation in VmRuntime.{hugepages_*}
///
/// This function **never writes to sysfs**. Any shortfall is treated as
/// a deterministic, blocking error **before** VFIO / cpuset staging.
/// QEMU CLI remains under operator control via vm.qemu.args/post_args.
pub fn provision_for_vm(rt: &mut VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    if !q.hugepages {
        info!(
            vm = %rt.name,
            "hugepages: disabled in config; skipping hugepage verification"
        );
        // Leave rt.hugepages_* at their defaults; hugepages are not active.
        rt.hugepages_active = false;
        rt.hugepages_node = None;
        rt.hugepages_pages = 0;
        rt.hugepages_bytes = 0;
        return Ok(());
    }

    let node = select_hugepage_node(rt)?;

    // 2MiB hugepages: exactly the same size your Bash suite used:
    //   RAM * 1024 / 2  (GiB → MiB → /2MiB)
    let hugepage_size_bytes: u64 = 2 * 1024 * 1024;

    let mem_bytes = q.mem_mb * 1024 * 1024;
    let pages = (mem_bytes + hugepage_size_bytes - 1) / hugepage_size_bytes;

    let node_dir = Path::new("/sys/devices/system/node")
        .join(format!("node{node}"))
        .join("hugepages")
        .join("hugepages-2048kB");

    let nr_path = node_dir.join("nr_hugepages");
    let free_path = node_dir.join("free_hugepages");

    let read_u64 = |path: &Path, label: &str| -> Result<u64> {
        let raw = fs::read_to_string(path).map_err(|e| {
            ChalybsError::Other(format!(
                "hugepages: failed to read {} from {}: {e}",
                label,
                path.display()
            ))
        })?;
        raw.trim().parse::<u64>().map_err(|e| {
            ChalybsError::Other(format!(
                "hugepages: failed to parse {} from {}: {e}",
                label,
                path.display()
            ))
        })
    };

    let nr_total = read_u64(&nr_path, "nr_hugepages")?;
    let free_pages = read_u64(&free_path, "free_hugepages")?;

    info!(
        vm = %rt.name,
        node,
        required_pages = pages,
        nr_hugepages = nr_total,
        free_hugepages = free_pages,
        "hugepages: verification of node-local 2MiB hugepages"
    );

    // Deterministic failure conditions:
    //
    // 1) Node does not have enough total hugepages provisioned.
    if nr_total < pages {
        return Err(ChalybsError::Other(format!(
            "hugepages: insufficient 2MiB hugepages provisioned on NUMA node {node}: \
             required {pages}, current {nr_total} (nr_hugepages). \
             Increase node-local hugepage pool and retry."
        )));
    }

    // 2) Node has enough total, but not enough free hugepages.
    if free_pages < pages {
        return Err(ChalybsError::Other(format!(
            "hugepages: NUMA node {node} has {nr_total} total 2MiB hugepages but only \
             {free_pages} free; VM {} requires {pages} free hugepages for {} MiB of RAM. \
             Stop other hugepage users or increase provisioning and retry.",
            rt.name, q.mem_mb
        )));
    }

    // At this point, we have at least `pages` free 2MiB hugepages on the
    // selected node. We do **not** touch sysfs; we only record the
    // expectation in the runtime for introspection and logging.
    let total_bytes = pages * hugepage_size_bytes;

    rt.hugepages_node = Some(node);
    rt.hugepages_pages = pages;
    rt.hugepages_bytes = total_bytes;
    rt.hugepages_active = true;

    rt.push_info(format!(
        "hugepages: verified availability of {} x 2MiB pages ({:.1} GiB) on NUMA node {} \
         for VM {}",
        pages,
        total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        node,
        rt.name
    ));

    Ok(())
}
