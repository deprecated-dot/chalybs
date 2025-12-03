// core/src/affinity.rs

use std::collections::{BTreeSet, HashMap};
use std::thread;
use std::time::Duration;

use nix::sched::{sched_setaffinity, CpuSet as NixCpuSet};
use nix::unistd::Pid;
use procfs::process::Process;
use tracing::{debug, info, warn};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

/// Parse a QEMU vCPU thread name and extract the vCPU index.
///
/// Historically your threads show up as "CPU N/KVM". To make this
/// robust against minor QEMU variations, we accept:
///
///   - "CPU N"
///   - "CPU N/KVM"
///   - "CPU N/kvm"
///   - "CPU N/qemu"
///   - "CPU N/<anything>"
///
/// and simply parse the first integer after "CPU ".
fn parse_vcpu_name(name: &str) -> Option<u32> {
    if !name.starts_with("CPU ") {
        return None;
    }

    let rest = &name[4..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();

    if digits.is_empty() {
        return None;
    }

    digits.parse::<u32>().ok()
}

/// Discover vCPUs via /proc/<pid>/task stat names.
///
/// We scan /proc/<pid>/task/*, read each thread's comm, and look for
/// names of the form "CPU N..." where N is the vCPU index.
///
/// Returns Vec<(tid, vcpu_index)>.
fn discover_vcpus(pid: i32) -> Result<Vec<(i32, u32)>> {
    let proc = Process::new(pid)
        .map_err(|e| ChalybsError::Affinity(format!("procfs: failed to open /proc/{pid}: {e}")))?;

    let tasks = proc.tasks().map_err(|e| {
        ChalybsError::Affinity(format!("procfs: failed to list tasks for pid {pid}: {e}"))
    })?;

    let mut out = Vec::new();

    for t_res in tasks {
        let t = match t_res {
            Ok(t) => t,
            Err(e) => {
                debug!("procfs: skipping task entry for pid {pid}: {e}");
                continue;
            }
        };

        let stat = match t.stat() {
            Ok(s) => s,
            Err(e) => {
                debug!("procfs: failed to read stat for tid {}: {e}", t.tid);
                continue;
            }
        };

        if let Some(idx) = parse_vcpu_name(&stat.comm) {
            out.push((t.tid, idx));
        }
    }

    Ok(out)
}

/// Ensure that all expected vCPU threads (0..num_vcpus-1) exist for QEMU.
///
/// This is a deterministic readiness barrier using only procfs:
/// we retry for a bounded number of iterations until we've observed
/// every vCPU index 0..=num_vcpus-1 at least once.
///
/// If the set never becomes complete, we treat it as a hard failure
/// and surface a clear error.
pub fn wait_for_qemu_threads(rt: &VmRuntime) -> Result<()> {
    let q = rt
        .qemu
        .as_ref()
        .ok_or_else(|| ChalybsError::Affinity("QEMU state not present in runtime".into()))?;

    let pid = q.pid;
    let expected = rt.cfg.qemu.num_vcpus;

    // Deterministic retry window: 400 * 5ms = 2000ms total.
    const MAX_ITER: u32 = 400;
    const SLEEP_MS: u64 = 5;

    info!(
        vm = %rt.name,
        pid,
        expected,
        "waiting for QEMU vCPU threads via procfs"
    );

    for _ in 0..MAX_ITER {
        let threads_res = discover_vcpus(pid);

        match threads_res {
            Ok(threads) => {
                let mut indices: BTreeSet<u32> = BTreeSet::new();
                for &(_, idx) in &threads {
                    indices.insert(idx);
                }

                if indices.len() as u32 == expected {
                    // Ensure indices are exactly 0..expected-1
                    let mut missing = Vec::new();
                    for v in 0..expected {
                        if !indices.contains(&v) {
                            missing.push(v);
                        }
                    }

                    if missing.is_empty() {
                        info!(
                            vm = %rt.name,
                            pid,
                            num_threads = threads.len(),
                            found = ?indices,
                            "all vCPU threads discovered via procfs"
                        );
                        return Ok(());
                    } else {
                        debug!(
                            vm = %rt.name,
                            pid,
                            ?missing,
                            "vCPU threads present but some indices are missing"
                        );
                    }
                } else {
                    debug!(
                        vm = %rt.name,
                        pid,
                        found = indices.len(),
                        expected,
                        "vCPU indices not complete yet"
                    );
                }
            }
            Err(e) => {
                debug!(
                    vm = %rt.name,
                    pid,
                    error = %format!("{e}"),
                    "procfs discover_vcpus failed; will retry"
                );
            }
        }

        thread::sleep(Duration::from_millis(SLEEP_MS));
    }

    Err(ChalybsError::Affinity(format!(
        "timed out waiting for {} vCPU threads for QEMU pid {} via procfs",
        expected, pid
    )))
}

/// Pin each QEMU vCPU thread to its configured host CPU.
///
/// Mapping:
///   vCPU index N -> rt.cpus.vm.cpus[N]
pub fn pin_vcpus(rt: &VmRuntime) -> Result<()> {
    let q = rt
        .qemu
        .as_ref()
        .ok_or_else(|| ChalybsError::Affinity("QEMU state not present in runtime".into()))?;

    let pid = q.pid;
    let expected = rt.cfg.qemu.num_vcpus as usize;
    let vm_cpus = &rt.cpus.vm.cpus;

    if vm_cpus.len() < expected {
        return Err(ChalybsError::Affinity(format!(
            "vm_cpus length {} < expected num_vcpus {}",
            vm_cpus.len(),
            expected
        )));
    }

    let threads = discover_vcpus(pid)?;

    // Build map: vcpu_index -> tid
    let mut by_index: HashMap<u32, i32> = HashMap::new();
    for (tid, idx) in threads {
        by_index.insert(idx, tid);
    }

    // Validate that all indices 0..expected-1 are present
    let mut missing = Vec::new();
    for idx in 0..expected {
        if !by_index.contains_key(&(idx as u32)) {
            missing.push(idx as u32);
        }
    }

    if !missing.is_empty() {
        return Err(ChalybsError::Affinity(format!(
            "missing vCPU threads for indices: {:?}",
            missing
        )));
    }

    // Perform pinning
    for (idx_u32, tid) in &by_index {
        let idx = *idx_u32 as usize;
        if idx >= vm_cpus.len() {
            warn!(
                vm = %rt.name,
                tid,
                vcpu_index = idx_u32,
                "skipping vCPU index beyond configured vm_cpus"
            );
            continue;
        }

        let host_cpu = vm_cpus[idx];

        let mut set = NixCpuSet::new();
        set.set(host_cpu as usize).map_err(|e| {
            ChalybsError::Affinity(format!(
                "failed to build cpuset for vCPU {} (host CPU {}): {e}",
                idx_u32, host_cpu
            ))
        })?;

        let tpid = Pid::from_raw(*tid);
        sched_setaffinity(tpid, &set).map_err(|e| {
            ChalybsError::Affinity(format!(
                "failed to set affinity: tid={} vcpu={} cpu={} err={e}",
                tid, idx_u32, host_cpu
            ))
        })?;

        info!(
            vm = %rt.name,
            tid,
            vcpu_index = idx_u32,
            host_cpu,
            "pinned vCPU thread to host CPU"
        );
    }

    Ok(())
}
