// core/src/hugepages.rs

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use nix::errno::Errno;
use nix::mount::{mount, umount, MsFlags};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;
use crate::util::parse_cpu_list;

const HUGEPAGES_MOUNT: &str = "/dev/hugepages";
// Marker is kept in the Chalybs control plane, not on hugetlbfs itself.
const HUGEPAGES_MARKER: &str = "/run/chalybs/.hugepages";
const PROC_MOUNTS: &str = "/proc/mounts";
const PROC_DROP_CACHES: &str = "/proc/sys/vm/drop_caches";
const PROC_COMPACT_MEMORY: &str = "/proc/sys/vm/compact_memory";
const PROC_NR_HUGEPAGES: &str = "/proc/sys/vm/nr_hugepages";

fn node_hugepages_dir(node: u16) -> PathBuf {
    Path::new("/sys/devices/system/node")
        .join(format!("node{node}"))
        .join("hugepages")
        .join("hugepages-2048kB")
}

fn read_u64(path: &Path, label: &str) -> Result<u64> {
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
}

fn read_node_hugepage_counters(node: u16) -> Result<(u64, u64)> {
    let dir = node_hugepages_dir(node);
    let nr_path = dir.join("nr_hugepages");
    let free_path = dir.join("free_hugepages");

    let nr_total = read_u64(&nr_path, "nr_hugepages")?;
    let free_pages = read_u64(&free_path, "free_hugepages")?;

    Ok((nr_total, free_pages))
}

fn write_node_nr_hugepages(node: u16, pages: u64) -> Result<()> {
    let dir = node_hugepages_dir(node);
    let nr_path = dir.join("nr_hugepages");
    let value = format!("{pages}\n");

    fs::write(&nr_path, value).map_err(|e| {
        ChalybsError::Other(format!(
            "hugepages: failed to write nr_hugepages for node {node} at {}: {e}",
            nr_path.display()
        ))
    })
}

fn ensure_hugetlbfs_mount(_rt: &VmRuntime) -> Result<()> {
    let mount_path = Path::new(HUGEPAGES_MOUNT);

    if !mount_path.exists() {
        fs::create_dir_all(mount_path).map_err(|e| {
            ChalybsError::Other(format!(
                "hugepages: failed to create {}: {e}",
                mount_path.display()
            ))
        })?;
    }

    // Best-effort check if already mounted.
    let already_mounted = match fs::read_to_string(PROC_MOUNTS) {
        Ok(contents) => contents.lines().any(|line| {
            let mut parts = line.split_whitespace();
            let _dev = parts.next();
            let mp = parts.next();
            mp == Some(HUGEPAGES_MOUNT)
        }),
        Err(e) => {
            warn!(
                "hugepages: failed to read {} for mount detection: {e}; attempting mount anyway",
                PROC_MOUNTS
            );
            false
        }
    };

    if already_mounted {
        info!("hugepages: hugetlbfs already mounted on {HUGEPAGES_MOUNT}; leaving as-is");
        return Ok(());
    }

    info!("hugepages: mounting hugetlbfs on {HUGEPAGES_MOUNT}");

    match mount(
        Some("hugetlbfs"),
        HUGEPAGES_MOUNT,
        Some("hugetlbfs"),
        MsFlags::empty(),
        None::<&str>,
    ) {
        Ok(()) => {
            // Write a marker in the Chalybs control plane so we know
            // this mount is Chalybs-managed and can be auto-unmounted.
            if let Err(e) = (|| -> std::io::Result<()> {
                fs::create_dir_all("/run/chalybs")?;
                fs::write(HUGEPAGES_MARKER, b"chalybs\n")?;
                Ok(())
            })() {
                warn!(
                    "hugepages: failed to write marker {}: {e}; mount will not be auto-unmounted",
                    HUGEPAGES_MARKER
                );
            }
            Ok(())
        }
        Err(Errno::EBUSY) => {
            // Someone else raced us; treat as success, but note that we
            // will not manage this mount unless our marker exists.
            warn!(
                "hugepages: mount on {} returned EBUSY; assuming already mounted by another entity",
                HUGEPAGES_MOUNT
            );
            Ok(())
        }
        Err(e) => Err(ChalybsError::Other(format!(
            "hugepages: failed to mount hugetlbfs on {HUGEPAGES_MOUNT}: {e}"
        ))),
    }
}

fn cleanup_hugetlbfs_mount() -> Result<()> {
    let marker_path = Path::new(HUGEPAGES_MARKER);

    if !marker_path.exists() {
        // Either not Chalybs-managed, or marker was never created; leave it alone.
        return Ok(());
    }

    info!(
        "hugepages: unmounting Chalybs-managed hugetlbfs from {}",
        HUGEPAGES_MOUNT
    );

    if let Err(e) = umount(HUGEPAGES_MOUNT) {
        warn!(
            "hugepages: failed to unmount {}: {e}; leaving mount in place",
            HUGEPAGES_MOUNT
        );
        // Even if unmount fails, still attempt to remove the marker below
        // so we don't keep claiming ownership incorrectly.
    }

    if let Err(e) = fs::remove_file(marker_path) {
        // ENOENT here is not a structural error (race, manual cleanup, etc.);
        // keep it out of the warning path to avoid log clutter.
        if e.kind() != std::io::ErrorKind::NotFound {
            warn!(
                "hugepages: failed to remove marker {}: {e}",
                marker_path.display()
            );
        } else {
            debug!(
                "hugepages: marker {} vanished before removal (NotFound); ignoring",
                marker_path.display()
            );
        }
    }

    Ok(())
}

fn drop_caches_and_compact(vm_name: &str, phase: &str) -> Result<()> {
    info!(
        vm = vm_name,
        phase, "hugepages: requesting pagecache drop + memory compaction"
    );

    // Both operations are best-effort; failures are logged but not fatal.
    if let Err(e) = fs::write(PROC_DROP_CACHES, b"3\n") {
        warn!(
            vm = vm_name,
            phase,
            path = PROC_DROP_CACHES,
            "hugepages: failed to write drop_caches: {e}"
        );
    }

    if let Err(e) = fs::write(PROC_COMPACT_MEMORY, b"1\n") {
        warn!(
            vm = vm_name,
            phase,
            path = PROC_COMPACT_MEMORY,
            "hugepages: failed to write compact_memory: {e}"
        );
    }

    Ok(())
}

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

/// Hugepage manager (Phase 12, **writey** semantics):
///
/// - If qemu.hugepages = false → no-op (logs and returns Ok).
/// - If qemu.hugepages = true:
///     * pick node (config/topology)
///     * compute required 2MiB pages from mem_mb
///     * ensure /dev/hugepages hugetlbfs mount exists
///     * read node-local nr_hugepages and free_hugepages
///     * if insufficient:
///         - request drop_caches + compact_memory
///         - raise node-local nr_hugepages to the required count
///         - re-check, with a small number of deterministic attempts
///     * record the outcome in VmRuntime.{hugepages_*}
///
/// Any shortfall after attempts is treated as a deterministic, blocking
/// error **before** VFIO / cpuset staging. QEMU CLI remains under
/// operator control via vm.qemu.args/post_args.
pub fn provision_for_vm(rt: &mut VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    if !q.hugepages {
        info!(
            vm = %rt.name,
            "hugepages: disabled in config; skipping hugepage provisioning"
        );
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

    ensure_hugetlbfs_mount(rt)?;

    // First, check current node-local pool.
    let (mut nr_total, mut free_pages) = read_node_hugepage_counters(node)?;

    info!(
        vm = %rt.name,
        node,
        required_pages = pages,
        nr_hugepages = nr_total,
        free_hugepages = free_pages,
        "hugepages: initial node-local 2MiB hugepage state"
    );

    if free_pages >= pages {
        let total_bytes = pages * hugepage_size_bytes;

        rt.hugepages_node = Some(node);
        rt.hugepages_pages = pages;
        rt.hugepages_bytes = total_bytes;
        rt.hugepages_active = true;

        rt.push_info(format!(
            "hugepages: using existing {} x 2MiB pages ({:.1} GiB) on NUMA node {} for VM {}",
            pages,
            total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            node,
            rt.name
        ));

        return Ok(());
    }

    // Not enough free pages; we may also need to increase total pages.
    const MAX_ATTEMPTS: u8 = 3;

    for attempt in 1..=MAX_ATTEMPTS {
        drop_caches_and_compact(&rt.name, "provision")?;

        // Refresh counters after compaction.
        let (nr_before, free_before) = read_node_hugepage_counters(node)?;

        info!(
            vm = %rt.name,
            node,
            attempt,
            required_pages = pages,
            nr_hugepages = nr_before,
            free_hugepages = free_before,
            "hugepages: post-compaction node state"
        );

        if nr_before < pages {
            info!(
                vm = %rt.name,
                node,
                attempt,
                target_pages = pages,
                "hugepages: raising node-local nr_hugepages"
            );
            write_node_nr_hugepages(node, pages)?;
        }

        // Re-check after raising the pool (or attempting to).
        let (nr_total2, free_pages2) = read_node_hugepage_counters(node)?;

        info!(
            vm = %rt.name,
            node,
            attempt,
            required_pages = pages,
            nr_hugepages = nr_total2,
            free_hugepages = free_pages2,
            "hugepages: verification after raise/compaction"
        );

        nr_total = nr_total2;
        free_pages = free_pages2;

        if free_pages >= pages {
            let total_bytes = pages * hugepage_size_bytes;

            rt.hugepages_node = Some(node);
            rt.hugepages_pages = pages;
            rt.hugepages_bytes = total_bytes;
            rt.hugepages_active = true;

            rt.push_info(format!(
                "hugepages: provisioned {} x 2MiB pages ({:.1} GiB) on NUMA node {} \
                 for VM {} after {} attempt(s)",
                pages,
                total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                node,
                rt.name,
                attempt
            ));

            return Ok(());
        }
    }

    // If we get here, all attempts failed to provide enough free pages.
    Err(ChalybsError::Other(format!(
        "hugepages: insufficient 2MiB hugepages on NUMA node {node} after compaction/raise attempts: \
         required {pages}, nr_hugepages={nr_total}, free_hugepages={free_pages}. \
         Consider increasing large-page capacity or reducing VM RAM."
    )))
}

/// Hugepage teardown for a VM.
///
/// Semantics:
///   - If hugepages were not active for this VM → no-op.
///   - Otherwise:
///       * Best-effort reset of node-local nr_hugepages to 0.
///       * Best-effort reset of global /proc/sys/vm/nr_hugepages to 0
///         (mirroring your legacy script).
///       * Best-effort drop_caches + compact_memory.
///       * Best-effort unmount of /dev/hugepages only if the mount is
///         marked as Chalybs-managed.
///   - Errors are **not** fatal; they are logged as warnings and the
///     function returns Ok so as not to block shutdown.
pub fn cleanup_for_vm(rt: &mut VmRuntime) -> Result<()> {
    if !rt.hugepages_active {
        info!(
            vm = %rt.name,
            "hugepages: cleanup skipped; VM did not use hugepages"
        );
        return Ok(());
    }

    let node = match rt.hugepages_node {
        Some(n) => n,
        None => {
            warn!(
                vm = %rt.name,
                "hugepages: cleanup requested but no NUMA node recorded; skipping node-local reset"
            );
            // Still attempt global cleanup + compaction + unmount below.
            0
        }
    };

    // Best-effort node-local reset.
    if rt.hugepages_node.is_some() {
        if let Err(e) = write_node_nr_hugepages(node, 0) {
            warn!(
                vm = %rt.name,
                node,
                "hugepages: failed to reset node-local nr_hugepages to 0: {e}"
            );
        } else {
            info!(
                vm = %rt.name,
                node,
                "hugepages: reset node-local nr_hugepages to 0"
            );
        }
    }

    // Best-effort global reset (mirrors your legacy script).
    if let Err(e) = fs::write(PROC_NR_HUGEPAGES, b"0\n") {
        warn!(
            vm = %rt.name,
            path = PROC_NR_HUGEPAGES,
            "hugepages: failed to reset global nr_hugepages to 0: {e}"
        );
    } else {
        info!(
            vm = %rt.name,
            path = PROC_NR_HUGEPAGES,
            "hugepages: reset global nr_hugepages to 0"
        );
    }

    // Best-effort post-teardown compaction.
    let _ = drop_caches_and_compact(&rt.name, "cleanup");

    // Best-effort hugetlbfs unmount if Chalybs-managed.
    let _ = cleanup_hugetlbfs_mount();

    rt.hugepages_active = false;
    rt.hugepages_node = None;
    rt.hugepages_pages = 0;
    rt.hugepages_bytes = 0;

    rt.push_info("hugepages: cleanup completed for VM");

    Ok(())
}
