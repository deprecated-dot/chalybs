use std::fs;
use std::path::Path;

use crate::errors::Result;

/// Public API function exposed to CLI.
/// For now, this is a simple OpenRC-friendly introspection of:
///   /sys/fs/cgroup/vfio_vm
///   /sys/fs/cgroup/vfio_host
pub fn cpuset_status(vm: &str) -> Result<String> {
    let vm_dir = Path::new("/sys/fs/cgroup/vfio_vm");
    let host_dir = Path::new("/sys/fs/cgroup/vfio_host");

    let mut out = String::new();
    out.push_str(&format!("VM: {vm}\n"));
    out.push_str("=== cpuset status ===\n");

    // VM cpuset
    if vm_dir.exists() {
        let cpus = read_opt(&vm_dir.join("cpuset.cpus"))?.unwrap_or_else(|| "<empty>".into());
        let mems = read_opt(&vm_dir.join("cpuset.mems"))?.unwrap_or_else(|| "<empty>".into());
        out.push_str(&format!("vm:\n  path: {}\n  cpus: {}\n  mems: {}\n",
                              vm_dir.display(), cpus, mems));
    } else {
        out.push_str("vm:\n  <cpuset /sys/fs/cgroup/vfio_vm not present>\n");
    }

    // Host cpuset
    if host_dir.exists() {
        let cpus = read_opt(&host_dir.join("cpuset.cpus"))?.unwrap_or_else(|| "<empty>".into());
        let mems = read_opt(&host_dir.join("cpuset.mems"))?.unwrap_or_else(|| "<empty>".into());
        out.push_str(&format!("host:\n  path: {}\n  cpus: {}\n  mems: {}\n",
                              host_dir.display(), cpus, mems));
    } else {
        out.push_str("host:\n  <cpuset /sys/fs/cgroup/vfio_host not present>\n");
    }

    Ok(out)
}

fn read_opt(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = fs::read_to_string(path)?;
    Ok(Some(s.trim().to_string()))
}
