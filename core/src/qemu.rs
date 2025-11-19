use std::fs;
use std::process::{Command, Stdio};

use tracing::{debug, info, warn};

use crate::errors::{ChalybsError, Result};
use crate::model::{VmRuntime, QemuState};

/// Preflight checks for QEMU and firmware paths.
pub fn preflight(rt: &VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    if !std::path::Path::new(&q.binary).exists() {
        return Err(ChalybsError::Qemu(format!(
            "QEMU binary not found: {}",
            q.binary
        )));
    }

    if !std::path::Path::new(&q.ovmf_code).exists() {
        return Err(ChalybsError::Qemu(format!(
            "OVMF code not found: {}",
            q.ovmf_code
        )));
    }

    if !std::path::Path::new(&q.ovmf_vars).exists() {
        return Err(ChalybsError::Qemu(format!(
            "OVMF vars not found: {}",
            q.ovmf_vars
        )));
    }

    Ok(())
}

/// Launch QEMU and move it into the vm cpuset.
pub fn launch(rt: &mut VmRuntime) -> Result<()> {
    let q = &rt.cfg.qemu;

    let mut cmd = Command::new(&q.binary);

    cmd.arg("-enable-kvm")
        .arg("-cpu")
        .arg("host")
        .arg("-smp")
        .arg(q.num_vcpus.to_string())
        .arg("-m")
        .arg(q.mem_mb.to_string())
        .arg("-machine")
        .arg("q35,accel=kvm")
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,readonly,file={}", q.ovmf_code))
        .arg("-drive")
        .arg(format!("if=pflash,format=raw,file={}", q.ovmf_vars));

    if !q.args.trim().is_empty() {
        for tok in q.args.split_whitespace() {
            cmd.arg(tok);
        }
    }

    cmd.stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    debug!("launching QEMU: {:?}", cmd);

    let child = cmd.spawn().map_err(|e| {
        ChalybsError::Qemu(format!("failed to spawn QEMU: {e}"))
    })?;

    let pid = child.id() as i32;

    info!(pid, "spawned QEMU process");

    // Move QEMU into vm cpuset if configured.
    if let Some(cg) = &rt.cgroups {
        let procs_path = cg.vm.join("cgroup.procs");
        if procs_path.exists() {
            fs::write(&procs_path, format!("{pid}\n")).map_err(|e| {
                ChalybsError::Qemu(format!(
                    "failed to write QEMU pid to {}: {e}",
                    procs_path.display()
                ))
            })?;
            info!(pid, path = %procs_path.display(), "moved QEMU to vm cpuset");
        } else {
            warn!(
                path = %procs_path.display(),
                "vm cpuset cgroup.procs not found; QEMU not moved into cpuset"
            );
        }
    }

    rt.qemu = Some(QemuState { pid, child });

    Ok(())
}

/// Attempt a graceful QEMU shutdown via SIGTERM and wait.
pub fn shutdown(rt: &mut VmRuntime) -> Result<()> {
    let Some(mut q) = rt.qemu.take() else {
        return Ok(());
    };

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    let pid = Pid::from_raw(q.pid);

    info!(pid = q.pid, "sending SIGTERM to QEMU");
    if let Err(e) = kill(pid, Signal::SIGTERM) {
        return Err(ChalybsError::Qemu(format!(
            "failed to send SIGTERM to QEMU {}: {e}",
            q.pid
        )));
    }

    let status = q.child.wait().map_err(|e| {
        ChalybsError::Qemu(format!(
            "failed to wait for QEMU {} to exit: {e}",
            q.pid
        ))
    })?;

    info!(pid = q.pid, ?status, "QEMU exited");
    Ok(())
}
