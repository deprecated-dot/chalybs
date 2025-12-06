// core/src/cpu/detect/validate.rs
//
// Hybrid CPU / NUMA topology validation (diagnostic-only).
//
// This module is deliberately pure and decoupled from the rest of the
// Chalybs runtime. It does not know about VmRuntime, cpusets, cgroups,
// or hugepage provisioning APIs. Instead, it consumes an explicit,
// pre-normalized topology + placement description and returns a
// structured report.
//
// Design goals:
//   - Deterministic: given the same input, it will always emit the same
//     set of issues in the same order for a given Chalybs release.
//   - Side-effect free: no syscalls, no /proc parsing, no logging.
//     Callers decide *when* and *how* to log or enforce the results.
//   - Strictly diagnostic at this layer: no implicit "fixups" or
//     heuristics. We only observe and report.
//   - Chalybs-agnostic: callers can adapt from any internal model
//     (VmRuntime, cpuset state, hugepages state, etc.) into the input
//     structs defined here.
//
// Typical usage pattern (outside this module):
//   1. Discover host NUMA topology (e.g. via sysfs or existing
//      chalybs_core::cpu::detect::topology logic).
//   2. Derive the VM's effective vCPU → CPU mapping and memory/hugepage
//      node selection from your planned cpusets and hugepage plan.
//   3. Build a CpuTopologyInput describing that relationship.
//   4. Call validate_cpu_topology(&input) to obtain a CpuTopologyReport.
//   5. Decide at a higher layer whether to:
//        - log warnings only (soft mode), or
//        - treat errors as fatal and refuse to launch (hard mode).

use std::collections::{HashMap, HashSet};

/// Severity level of a topology issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuTopologySeverity {
    Info,
    Warning,
    Error,
}

/// A single topology issue detected during validation.
///
/// `code` is a stable, human-readable identifier intended to be used
/// in logs, tests, or future documentation. `message` is free-form
/// but deterministic for a given Chalybs release.
#[derive(Debug, Clone)]
pub struct CpuTopologyIssue {
    pub severity: CpuTopologySeverity,
    pub code: &'static str,
    pub message: String,
}

/// Aggregate validation result.
///
/// This is intentionally simple: a flat list of issues. Helpers are
/// provided to check for the presence of warnings/errors.
#[derive(Debug, Clone, Default)]
pub struct CpuTopologyReport {
    pub issues: Vec<CpuTopologyIssue>,
}

impl CpuTopologyReport {
    /// Returns true if there are no Error-level issues.
    pub fn is_ok(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|i| matches!(i.severity, CpuTopologySeverity::Error))
    }

    /// Returns true if there are *no* issues at all.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// Returns true if there is at least one Warning-level issue.
    pub fn has_warnings(&self) -> bool {
        self.issues
            .iter()
            .any(|i| matches!(i.severity, CpuTopologySeverity::Warning))
    }

    /// Returns true if there is at least one Error-level issue.
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|i| matches!(i.severity, CpuTopologySeverity::Error))
    }
}

/// Description of a single host NUMA node.
///
/// `id` is the kernel/node id (e.g. 0, 1, 2, ...).
/// `logical_cpus` is the set of logical CPU IDs (as seen in /sys or
/// sched_setaffinity) that belong to this node.
#[derive(Debug, Clone)]
pub struct HostNumaNode {
    pub id: u16,
    pub logical_cpus: Vec<u32>,
}

/// Full host NUMA CPU topology summary.
///
/// The expectation is that the caller has already observed the real
/// system (e.g. via sysfs) and constructed this map in a deterministic
/// way. This module does not perform any discovery on its own.
#[derive(Debug, Clone)]
pub struct HostNumaTopology {
    pub nodes: Vec<HostNumaNode>,
}

/// Description of a VM's CPU + memory placement relative to the host.
///
/// This is expressed entirely in host coordinates:
///   - `vm_vcpus` gives the host CPU IDs that the VM's vCPUs
///     will be pinned to (one entry per vCPU).
///   - `hugepage_nodes` are the host NUMA node IDs from which
///     RAM/hugepages will be allocated.
///   - `requested_vcpu_count` is the intended vCPU count from config
///     (qemu.num_vcpus). It may be used to cross-check consistency.
#[derive(Debug, Clone)]
pub struct VmCpuPlacement {
    /// Requested vCPU count from configuration (e.g. qemu.num_vcpus).
    pub requested_vcpu_count: u32,

    /// Host logical CPU IDs the VM's vCPUs will be pinned to.
    ///
    /// This should contain exactly one entry per vCPU if pinning is
    /// fully deterministic. In cases where the mapping is not fully
    /// decided yet, callers may pass fewer entries; that inconsistency
    /// will be reported as a diagnostic.
    pub vm_vcpus: Vec<u32>,

    /// Host NUMA node IDs that will provide RAM / hugepages for this VM.
    ///
    /// Typically this will be a single node id, but mixed-node allocations
    /// (intentional or accidental) are supported and will be reported.
    pub hugepage_nodes: Vec<u16>,
}

/// Bundled input to the topology validator.
///
/// This keeps the public API surface small and lets us evolve the
/// internal checks without changing every call site.
#[derive(Debug, Clone)]
pub struct CpuTopologyInput {
    pub host: HostNumaTopology,
    pub vm: VmCpuPlacement,
}

/// Perform a hybrid CPU/NUMA topology validation.
///
/// This function is *pure*: it does not read /proc, does not log, and
/// has no side effects. It only returns a structured report describing
/// what it found. Callers decide whether to:
//    - treat warnings as informative only, or
///   - treat errors as fatal and abort the VM launch.
pub fn validate_cpu_topology(input: &CpuTopologyInput) -> CpuTopologyReport {
    let mut report = CpuTopologyReport::default();

    // Fast path: empty or degenerate host topology.
    if input.host.nodes.is_empty() {
        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "host_topology_empty",
            message: "host NUMA topology is empty; cannot perform CPU/NUMA validation"
                .to_string(),
        });
        return report;
    }

    // Build a CPU → node_id map for quick lookup.
    let mut cpu_to_node: HashMap<u32, u16> = HashMap::new();
    let mut node_capacities: HashMap<u16, usize> = HashMap::new();

    for node in &input.host.nodes {
        let capacity = node.logical_cpus.len();
        node_capacities.insert(node.id, capacity);

        for &cpu_id in &node.logical_cpus {
            cpu_to_node.insert(cpu_id, node.id);
        }
    }

    // 1) Check basic consistency of requested_vcpu_count vs vm_vcpus length.
    if input.vm.requested_vcpu_count == 0 {
        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "vcpu_count_zero",
            message: "requested_vcpu_count is zero; VM will have no vCPUs".to_string(),
        });
    } else if input.vm.vm_vcpus.len() as u32 != input.vm.requested_vcpu_count {
        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "vcpu_mapping_incomplete",
            message: format!(
                "vCPU mapping is incomplete or inconsistent: requested {} vCPUs, \
but only {} host CPU assignments are present",
                input.vm.requested_vcpu_count,
                input.vm.vm_vcpus.len()
            ),
        });
    }

    // 2) Determine which NUMA nodes are used by the VM's vCPUs.
    let mut vcpu_nodes: HashSet<u16> = HashSet::new();
    let mut unmapped_vcpus: Vec<u32> = Vec::new();

    for &cpu_id in &input.vm.vm_vcpus {
        match cpu_to_node.get(&cpu_id).copied() {
            Some(node_id) => {
                vcpu_nodes.insert(node_id);
            }
            None => {
                unmapped_vcpus.push(cpu_id);
            }
        }
    }

    if !unmapped_vcpus.is_empty() {
        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Error,
            code: "vcpu_host_cpu_unmapped",
            message: format!(
                "one or more vCPUs are assigned to host CPUs that do not appear \
in the host NUMA topology: {:?}",
                unmapped_vcpus
            ),
        });
    }

    // 3) Determine which NUMA nodes will supply memory / hugepages.
    let mut mem_nodes: HashSet<u16> = HashSet::new();
    for &node_id in &input.vm.hugepage_nodes {
        mem_nodes.insert(node_id);
        if !node_capacities.contains_key(&node_id) {
            report.issues.push(CpuTopologyIssue {
                severity: CpuTopologySeverity::Warning,
                code: "memory_node_unknown",
                message: format!(
                    "memory/hugepage node {} is not present in host CPU topology; \
cannot verify CPU-locality for this node",
                    node_id
                ),
            });
        }
    }

    // 4) Check for cross-node vCPU placement.
    if vcpu_nodes.len() > 1 {
        let mut nodes: Vec<u16> = vcpu_nodes.iter().copied().collect();
        nodes.sort_unstable();

        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "vcpu_cross_node",
            message: format!(
                "vCPUs are spread across multiple NUMA nodes: {:?}; this may \
increase cross-node memory latency",
                nodes
            ),
        });
    }

    // 5) Check for mixed-node memory / hugepage allocations.
    if mem_nodes.len() > 1 {
        let mut nodes: Vec<u16> = mem_nodes.iter().copied().collect();
        nodes.sort_unstable();

        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "memory_cross_node",
            message: format!(
                "memory/hugepages are allocated from multiple NUMA nodes: {:?}; \
this may reduce locality for vCPUs",
                nodes
            ),
        });
    }

    // 6) Check correspondence between vCPU nodes and memory nodes.
    if !vcpu_nodes.is_empty() && !mem_nodes.is_empty() && vcpu_nodes != mem_nodes {
        let mut vcpu_list: Vec<u16> = vcpu_nodes.iter().copied().collect();
        let mut mem_list: Vec<u16> = mem_nodes.iter().copied().collect();
        vcpu_list.sort_unstable();
        mem_list.sort_unstable();

        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Warning,
            code: "vcpu_memory_node_mismatch",
            message: format!(
                "NUMA nodes for vCPUs ({:?}) do not match NUMA nodes for memory/hugepages ({:?}); \
this may introduce cross-node memory access",
                vcpu_list, mem_list
            ),
        });
    }

    // 7) Capacity / oversubscription check per node: number of vCPUs assigned
    //    vs number of logical CPUs available on that node.
    if !input.vm.vm_vcpus.is_empty() {
        let mut vcpus_per_node: HashMap<u16, usize> = HashMap::new();
        for &cpu_id in &input.vm.vm_vcpus {
            if let Some(node_id) = cpu_to_node.get(&cpu_id).copied() {
                *vcpus_per_node.entry(node_id).or_insert(0) += 1;
            }
        }

        let mut oversubscribed_nodes: Vec<(u16, usize, usize)> = Vec::new();

        for (node_id, used_vcpus) in vcpus_per_node {
            if let Some(&capacity) = node_capacities.get(&node_id) {
                if used_vcpus > capacity {
                    oversubscribed_nodes.push((node_id, used_vcpus, capacity));
                }
            } else {
                report.issues.push(CpuTopologyIssue {
                    severity: CpuTopologySeverity::Warning,
                    code: "node_capacity_unknown",
                    message: format!(
                        "cannot verify capacity of NUMA node {} used by vCPUs; \
node is missing from capacity map",
                        node_id
                    ),
                });
            }
        }

        if !oversubscribed_nodes.is_empty() {
            oversubscribed_nodes.sort_by_key(|(node_id, _, _)| *node_id);

            let details: Vec<String> = oversubscribed_nodes
                .iter()
                .map(|(node_id, used, cap)| format!("node {}: {} vCPUs > {} CPUs", node_id, used, cap))
                .collect();

            report.issues.push(CpuTopologyIssue {
                severity: CpuTopologySeverity::Error,
                code: "node_oversubscribed",
                message: format!(
                    "one or more NUMA nodes are oversubscribed by vCPUs relative to \
their logical CPU capacity: {}",
                    details.join("; ")
                ),
            });
        }
    }

    // 8) Informational: single-node, fully-local case.
    if report.issues.is_empty()
        && vcpu_nodes.len() == 1
        && mem_nodes.len() == 1
        && vcpu_nodes == mem_nodes
    {
        let node_id = *vcpu_nodes.iter().next().unwrap_or(&0);
        report.issues.push(CpuTopologyIssue {
            severity: CpuTopologySeverity::Info,
            code: "topology_local_and_aligned",
            message: format!(
                "vCPUs and memory/hugepages are confined to NUMA node {}; topology appears local and aligned",
                node_id
            ),
        });
    }

    report
}
