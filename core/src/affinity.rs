use std::time::{Duration, Instant};

use tracing::{debug, info};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

use nix::sched::{sched_setaffinity, CpuSet as NixCpuSet};
use nix::unistd::Pid;
use procfs::process::Process;
use regex::Regex;

/// How long we’re willing to wait for all vCPU threads to appear.
const THREAD_WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to poll /proc for QEMU threads.
const THREAD_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wait until all QEMU vCPU threads (CPU N/KVM) are present.
pub fn wait_for_qemu_threads(rt: &VmRuntime) -> Result<()> {
    let qemu = rt
        .qemu
        .as_ref()
        .ok_or_else(|| ChalybsError::State("QEMU not started".into()))?;

    let pid = qemu.pid;
    let expected = rt.cfg.qemu.num_vcpus;
    let re = Regex::new(r"^CPU (\d+)/KVM$").unwrap();
    let start = Instant::now();

    loop {
        let proc = Process::new(pid)
            .map_err(|e| ChalybsError::Affinity(format!("procfs error for pid {pid}: {e}")))?;

        let tasks = proc
            .tasks()
            .map_err(|e| ChalybsError::Affinity(format!("tasks() error for pid {pid}: {e}")))?;

        let mut found_indices = Vec::new();

        for task_res in tasks {
            let task = task_res
                .map_err(|e| ChalybsError::Affinity(format!("task error for pid {pid}: {e}")))?;
            let stat = task
                .stat()
                .map_err(|e| ChalybsError::Affinity(format!("stat() error for tid {}: {e}", task.tid)))?;

            if let Some(caps) = re.captures(&stat.comm) {
                let idx: u32 = caps.get(1).unwrap().as_str().parse().unwrap();
                found_indices.push(idx);
            }
        }

        found_indices.sort_unstable();
        found_indices.dedup();

        debug!(
            pid,
            ?found_indices,
            expected,
            "polled QEMU threads for vCPU stabilization"
        );

        if found_indices.len() as u32 == expected {
            info!(pid, count = found_indices.len(), "all vCPU threads detected");
            return Ok(());
        }

        if start.elapsed() > THREAD_WAIT_TIMEOUT {
            return Err(ChalybsError::Affinity(format!(
                "timeout waiting for vCPU threads: found {}, expected {}",
                found_indices.len(),
                expected
            )));
        }

        std::thread::sleep(THREAD_POLL_INTERVAL);
    }
}

/// Pin each QEMU vCPU thread (CPU N/KVM) to the configured vm CPU list.
pub fn pin_vcpus(rt: &VmRuntime) -> Result<()> {
    let qemu = rt
        .qemu
        .as_ref()
        .ok_or_else(|| ChalybsError::State("QEMU not started".into()))?;

    let pid = qemu.pid;
    let vm_cpus = &rt.cpus.vm.cpus;
    let expected = rt.cfg.qemu.num_vcpus;

    if vm_cpus.len() as u32 != expected {
        return Err(ChalybsError::Affinity(format!(
            "vm_cpus length {} != num_vcpus {}",
            vm_cpus.len(),
            expected
        )));
    }

    let proc = Process::new(pid)
        .map_err(|e| ChalybsError::Affinity(format!("procfs error for pid {pid}: {e}")))?;

    let tasks = proc
        .tasks()
        .map_err(|e| ChalybsError::Affinity(format!("tasks() error for pid {pid}: {e}")))?;

    let re = Regex::new(r"^CPU (\d+)/KVM$").unwrap();

    for task_res in tasks {
        let task = task_res
            .map_err(|e| ChalybsError::Affinity(format!("task error for pid {pid}: {e}")))?;
        let stat = task
            .stat()
            .map_err(|e| ChalybsError::Affinity(format!("stat() error for tid {}: {e}", task.tid)))?;

        if let Some(caps) = re.captures(&stat.comm) {
            let idx: usize = caps.get(1).unwrap().as_str().parse().unwrap();

            if idx >= vm_cpus.len() {
                return Err(ChalybsError::Affinity(format!(
                    "vCPU index {} out of range (vm_cpus len = {})",
                    idx,
                    vm_cpus.len()
                )));
            }

            let cpu = vm_cpus[idx];
            let mut set = NixCpuSet::new();

            set.set(cpu as usize)
                .map_err(|e| ChalybsError::Affinity(format!("failed to set cpuset bit {}: {e}", cpu)))?;

            let tid = Pid::from_raw(task.tid);

            sched_setaffinity(tid, &set).map_err(|e| {
                ChalybsError::Affinity(format!(
                    "sched_setaffinity failed for tid {} (vcpu {} → cpu {}): {e}",
                    task.tid, idx, cpu
                ))
            })?;

            info!(
                tid = task.tid,
                vcpu_index = idx,
                cpu,
                "pinned vCPU thread"
            );
        }
    }

    Ok(())
}
