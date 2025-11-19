// core/src/affinity.rs

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use nix::sched::{sched_setaffinity, CpuSet as NixCpuSet};
use nix::unistd::Pid;
use procfs::process::Process;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

/// Parse a QEMU vCPU thread name like "CPU 3/KVM" -> Some(3)
fn parse_vcpu_name(name: &str) -> Option<u32> {
    if !name.starts_with("CPU ") || !name.ends_with("/KVM") {
        return None;
    }
    let inner = &name[4..name.len().saturating_sub(4)];
    inner.parse::<u32>().ok()
}

/// QMP "execute" command
#[derive(Debug, Serialize)]
struct QmpCommand<'a> {
    execute: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<Value>,
}

/// QMP "query-cpus-fast" entry (we only care about cpu-index + thread-id)
#[derive(Debug, Deserialize)]
struct QmpCpuEntry {
    #[serde(rename = "cpu-index")]
    cpu_index: u32,
    #[serde(rename = "thread-id")]
    thread_id: i32,
}

/// Connect to the QMP socket and return (writer, reader).
fn qmp_connect(path: &str) -> Result<(UnixStream, BufReader<UnixStream>)> {
    let stream = UnixStream::connect(path).map_err(|e| {
        ChalybsError::Affinity(format!("failed to connect to QMP socket {}: {e}", path))
    })?;

    let reader = BufReader::new(stream.try_clone().map_err(|e| {
        ChalybsError::Affinity(format!(
            "failed to clone QMP socket {} for reader: {e}",
            path
        ))
    })?);

    Ok((stream, reader))
}

/// Perform the initial QMP greeting + qmp_capabilities handshake.
fn qmp_handshake(path: &str, writer: &mut UnixStream, reader: &mut BufReader<UnixStream>) -> Result<()> {
    // Read greeting line
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| {
        ChalybsError::Affinity(format!("failed to read QMP greeting from {}: {e}", path))
    })?;

    // We don't care about the exact structure, just that it's valid JSON.
    serde_json::from_str::<Value>(&line).map_err(|e| {
        ChalybsError::Affinity(format!("failed to parse QMP greeting from {}: {e}", path))
    })?;

    // Send qmp_capabilities
    let cmd = QmpCommand {
        execute: "qmp_capabilities",
        arguments: None,
    };
    let s = serde_json::to_string(&cmd).map_err(|e| {
        ChalybsError::Affinity(format!("failed to serialize qmp_capabilities: {e}"))
    })?;

    writer.write_all(s.as_bytes()).map_err(|e| {
        ChalybsError::Affinity(format!("failed to write qmp_capabilities to {}: {e}", path))
    })?;
    writer.write_all(b"\n").map_err(|e| {
        ChalybsError::Affinity(format!("failed to terminate qmp_capabilities on {}: {e}", path))
    })?;
    writer.flush().map_err(|e| {
        ChalybsError::Affinity(format!("failed to flush qmp_capabilities to {}: {e}", path))
    })?;

    // Read capabilities response (ignore contents, but ensure it's valid)
    line.clear();
    reader.read_line(&mut line).map_err(|e| {
        ChalybsError::Affinity(format!("failed to read qmp_capabilities response from {}: {e}", path))
    })?;
    serde_json::from_str::<Value>(&line).map_err(|e| {
        ChalybsError::Affinity(format!(
            "failed to parse qmp_capabilities response from {}: {e}",
            path
        ))
    })?;

    Ok(())
}

/// Query vCPU mapping via QMP (query-cpus-fast).
///
/// Returns Vec<(tid, vcpu_index)>.
fn discover_vcpus_qmp(path: &str) -> Result<Vec<(i32, u32)>> {
    let (mut writer, mut reader) = qmp_connect(path)?;

    qmp_handshake(path, &mut writer, &mut reader)?;

    // Send query-cpus-fast
    let cmd = QmpCommand {
        execute: "query-cpus-fast",
        arguments: None,
    };
    let s = serde_json::to_string(&cmd).map_err(|e| {
        ChalybsError::Affinity(format!("failed to serialize query-cpus-fast: {e}"))
    })?;

    writer.write_all(s.as_bytes()).map_err(|e| {
        ChalybsError::Affinity(format!("failed to write query-cpus-fast to {}: {e}", path))
    })?;
    writer.write_all(b"\n").map_err(|e| {
        ChalybsError::Affinity(format!("failed to terminate query-cpus-fast on {}: {e}", path))
    })?;
    writer.flush().map_err(|e| {
        ChalybsError::Affinity(format!("failed to flush query-cpus-fast to {}: {e}", path))
    })?;

    // Read response
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| {
        ChalybsError::Affinity(format!("failed to read query-cpus-fast response from {}: {e}", path))
    })?;

    let v: Value = serde_json::from_str(&line).map_err(|e| {
        ChalybsError::Affinity(format!(
            "failed to parse query-cpus-fast response from {}: {e}",
            path
        ))
    })?;

    if let Some(err) = v.get("error") {
        return Err(ChalybsError::Affinity(format!(
            "QMP query-cpus-fast returned error: {err}"
        )));
    }

    let arr = v
        .get("return")
        .and_then(|r| r.as_array())
        .ok_or_else(|| {
            ChalybsError::Affinity(
                "QMP query-cpus-fast response missing 'return' array".into(),
            )
        })?;

    let mut out = Vec::new();
    for item in arr {
        let entry: QmpCpuEntry = serde_json::from_value(item.clone()).map_err(|e| {
            ChalybsError::Affinity(format!(
                "failed to deserialize query-cpus-fast entry: {e}"
            ))
        })?;
        out.push((entry.thread_id, entry.cpu_index));
    }

    Ok(out)
}

/// Fallback: discover vCPUs via /proc/<pid>/task stat names "CPU N/KVM".
fn discover_vcpus_procfs(pid: i32) -> Result<Vec<(i32, u32)>> {
    let proc = Process::new(pid).map_err(|e| {
        ChalybsError::Affinity(format!("procfs: failed to open /proc/{pid}: {e}"))
    })?;

    let tasks = proc.tasks().map_err(|e| {
        ChalybsError::Affinity(format!(
            "procfs: failed to list tasks for pid {pid}: {e}"
        ))
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

/// Try QMP first; if that fails or returns nothing, fall back to /proc.
///
/// Returns Vec<(tid, vcpu_index)>.
fn discover_vcpus(rt: &VmRuntime, pid: i32) -> Result<Vec<(i32, u32)>> {
    let qmp_path = format!("/run/chalybs/{}.qmp", rt.name);

    match discover_vcpus_qmp(&qmp_path) {
        Ok(v) if !v.is_empty() => {
            debug!(
                vm = %rt.name,
                pid,
                path = %qmp_path,
                num = v.len(),
                "discovered vCPUs via QMP"
            );
            return Ok(v);
        }
        Ok(_) => {
            debug!(
                vm = %rt.name,
                pid,
                path = %qmp_path,
                "QMP discovery returned no vCPUs; falling back to /proc"
            );
        }
        Err(e) => {
            debug!(
                vm = %rt.name,
                pid,
                path = %qmp_path,
                error = %format!("{e}"),
                "QMP discovery failed; falling back to /proc"
            );
        }
    }

    discover_vcpus_procfs(pid)
}

/// Ensure that all expected vCPU threads (0..num_vcpus-1) exist for QEMU.
///
/// Uses QMP (with /proc fallback) in a retry loop as a readiness barrier
/// before we attempt to pin vCPUs.
pub fn wait_for_qemu_threads(rt: &VmRuntime) -> Result<()> {
    let q = rt.qemu.as_ref().ok_or_else(|| {
        ChalybsError::Affinity("QEMU state not present in runtime".into())
    })?;

    let pid = q.pid;
    let expected = rt.cfg.qemu.num_vcpus;

    const MAX_ITER: u32 = 400;
    const SLEEP_MS: u64 = 5;

    info!(
        vm = %rt.name,
        pid,
        expected,
        "waiting for QEMU vCPU threads via QMP/procfs"
    );

    for _ in 0..MAX_ITER {
        let threads_res = discover_vcpus(rt, pid);

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
                            "all vCPU threads discovered"
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
                    "discover_vcpus failed; will retry"
                );
            }
        }

        thread::sleep(Duration::from_millis(SLEEP_MS));
    }

    Err(ChalybsError::Affinity(format!(
        "timed out waiting for {} vCPU threads for QEMU pid {}",
        expected, pid
    )))
}

/// Pin each QEMU vCPU thread to its configured host CPU.
///
/// Mapping:
///   vCPU index N -> rt.cpus.vm.cpus[N]
pub fn pin_vcpus(rt: &VmRuntime) -> Result<()> {
    let q = rt.qemu.as_ref().ok_or_else(|| {
        ChalybsError::Affinity("QEMU state not present in runtime".into())
    })?;

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

    let threads = discover_vcpus(rt, pid)?;

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
