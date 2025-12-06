// core/src/cpu/detect/topology.rs
//
// Host NUMA topology discovery for observability and future validation.
//
// This module is intentionally *host-only* and side-effect free:
//   - It reads Linux sysfs NUMA data from
//       /sys/devices/system/node/node*/cpulist
//   - It produces a simple HostNumaTopology structure
//   - It never mutates state, never guesses, and never applies
//     policy by itself.
//
// Current role (Phase 0 for NUMA validation):
//   - Provide structured, deterministic telemetry about the host
//     NUMA layout alongside CpuIdentity / CpuArch.
//   - Lay the groundwork for a future hybrid validator that can
//     decide (warn vs. hard-error) based on explicit rules.
//
// This is deliberately conservative: if sysfs is missing or
// unreadable, we simply log that fact and do not treat it as a
// failure at this layer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::classify::CpuArch;
use super::cpuid::CpuIdentity;

const SYSFS_NODE_DIR: &str = "/sys/devices/system/node";

#[derive(Debug, Clone)]
pub struct HostNumaNode {
    pub node_id: u32,
    pub cpus: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct HostNumaTopology {
    pub nodes: Vec<HostNumaNode>,
}

impl HostNumaTopology {
    /// Discover host NUMA nodes from sysfs.
    ///
    /// We look for:
    ///   /sys/devices/system/node/node*/cpulist
    ///
    /// For each node that has a parsable cpulist, we produce a
    /// HostNumaNode with a sorted, de-duplicated list of CPUs.
    pub fn from_sysfs() -> io::Result<Self> {
        let base = Path::new(SYSFS_NODE_DIR);
        let mut nodes = Vec::new();

        let entries = match fs::read_dir(base) {
            Ok(e) => e,
            Err(e) => {
                // Propagate as-is; callers decide how to interpret.
                return Err(e);
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let file_name = entry.file_name();
            let name = match file_name.to_str() {
                Some(n) => n,
                None => continue,
            };

            // We only care about directories named "nodeN".
            if !name.starts_with("node") {
                continue;
            }

            let node_id: u32 = match name["node".len()..].parse() {
                Ok(id) => id,
                Err(_) => continue,
            };

            let cpulist_path: PathBuf = entry.path().join("cpulist");
            let cpulist_str = match fs::read_to_string(&cpulist_path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let mut cpus = parse_cpulist(cpulist_str.trim());
            if cpus.is_empty() {
                continue;
            }

            cpus.sort_unstable();
            cpus.dedup();

            nodes.push(HostNumaNode { node_id, cpus });
        }

        Ok(HostNumaTopology { nodes })
    }
}

/// Parse a Linux cpulist string of the form:
///   "0-7,16-23,32,33-35"
/// into a Vec<u32> of individual CPU IDs.
///
/// This parser is deterministic and conservative:
///   - Invalid segments are skipped.
///   - Negative numbers are ignored.
///   - Overflows are silently dropped.
fn parse_cpulist(s: &str) -> Vec<u32> {
    let mut out = Vec::new();

    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        if let Some((start_s, end_s)) = part.split_once('-') {
            let start = match start_s.trim().parse::<u32>() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let end = match end_s.trim().parse::<u32>() {
                Ok(v) => v,
                Err(_) => continue,
            };

            if end < start {
                continue;
            }

            // Inclusive range [start, end].
            for cpu in start..=end {
                out.push(cpu);
            }
        } else {
            // Single CPU index.
            if let Ok(cpu) = part.parse::<u32>() {
                out.push(cpu);
            }
        }
    }

    out
}

/// Log a summary of the host NUMA topology alongside CpuIdentity and
/// CpuArch classification.
///
/// This is *observability-only* for now. It does not enforce any
/// policy or emit warnings/errors about mismatches; that logic will
/// live in a future, explicit validator that can apply the chosen
/// hybrid (warn vs. hard-error) rules.
pub fn log_host_numa_topology(ident: &CpuIdentity, arch: CpuArch) {
    match HostNumaTopology::from_sysfs() {
        Ok(topo) => {
            tracing::info!(
                ?ident,
                ?arch,
                ?topo,
                "cpu_detect: discovered host NUMA topology from sysfs"
            );
        }
        Err(e) => {
            tracing::info!(
                ?ident,
                ?arch,
                error = %e,
                "cpu_detect: failed to read host NUMA topology from sysfs; \
                 continuing without NUMA telemetry"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cpulist;

    #[test]
    fn parse_simple_cpulist() {
        assert_eq!(parse_cpulist("0"), vec![0]);
        assert_eq!(parse_cpulist("3,5"), vec![3, 5]);
        assert_eq!(parse_cpulist("0-3"), vec![0, 1, 2, 3]);
        assert_eq!(parse_cpulist("0-1,4-5,7"), vec![0, 1, 4, 5, 7]);
    }

    #[test]
    fn parse_cpulist_ignores_garbage() {
        // Invalid segments are simply skipped.
        assert_eq!(parse_cpulist("0-x,2-3,y,5"), vec![0, 2, 3, 5]);
    }
}
