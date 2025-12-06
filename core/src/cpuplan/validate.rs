// core/src/cpuplan/validate.rs
//
// Structural validation of CpuPlan.
//
// This module performs *pure* consistency checks on the CpuPlan
// data model. It does not talk to sysfs, /proc, or CPUID, and it
// does not apply policy (warn vs. hard-error). Instead, it returns
// structured findings that higher layers can interpret based on
// IsolationMode, enforcement level, etc.
//
// Current checks (intentionally conservative):
//
//   - If the host NUMA topology has no nodes, we treat this as a
//     "no topology information" scenario and return no findings.
//     (Detection already logged the absence of NUMA telemetry.)
//   - For each host CPU listed in VmCpuLayout.host.cpus, if the
//     CPU does not appear in *any* NUMA node's cpulist, we emit
//     HostCpuOutsideTopology { cpu }.
//   - For each VM CPU listed in VmCpuLayout.vm.cpus, if the CPU
//     does not appear in *any* NUMA node's cpulist, we emit
//     VmCpuOutsideTopology { cpu }.
//
// This keeps the layer strictly about "is the plan self-consistent
// with the discovered topology?", without deciding what to do with
// those inconsistencies.

use std::collections::HashSet;

use super::plan::CpuPlan;

/// A single structural CPU plan validation finding.
///
/// This is deliberately *not* an error type tied to policy. Higher
/// layers decide whether a given finding is a warning, a hard error,
/// or something else.
#[derive(Debug, Clone)]
pub enum CpuPlanValidationError {
    /// A host CPU listed in the VM's host CPU set does not appear in
    /// any NUMA node in the discovered host topology.
    HostCpuOutsideTopology {
        cpu: u32,
    },

    /// A VM CPU listed in the VM's vCPU set does not appear in any
    /// NUMA node in the discovered host topology.
    VmCpuOutsideTopology {
        cpu: u32,
    },
}

/// Validate a CpuPlan and return all structural findings.
///
/// Semantics:
///   - If topology.nodes is empty, no findings are returned; the
///     absence of NUMA data is treated as "no information", not a
///     structural error at this layer.
///   - Otherwise, we ensure that all host_cpus and vm_cpus
///     referenced by the VmCpuLayout are present in the union of
///     all topology node CPU lists.
///
/// The returned Vec may be empty (no findings) or contain one or
/// more distinct issues. Callers are free to interpret these as
/// warnings or hard errors based on their policy.
pub fn validate_cpu_plan(plan: &CpuPlan) -> Vec<CpuPlanValidationError> {
    let mut findings = Vec::new();

    // If we have no NUMA nodes at all, we don't attempt any topology-
    // based validation here. Detection already logged the absence of
    // NUMA telemetry.
    if plan.topology.nodes.is_empty() {
        return findings;
    }

    // Build the union of all CPUs that appear anywhere in the host NUMA
    // topology. This is our "ground truth" set for topology-aware checks.
    let mut topo_cpus: HashSet<u32> = HashSet::new();
    for node in &plan.topology.nodes {
        for &cpu in &node.cpus {
            topo_cpus.insert(cpu);
        }
    }

    // 1) Host CPU set vs. topology.
    for &cpu in &plan.vm_layout.host.cpus {
        if !topo_cpus.contains(&cpu) {
            findings.push(CpuPlanValidationError::HostCpuOutsideTopology { cpu });
        }
    }

    // 2) VM CPU set vs. topology.
    for &cpu in &plan.vm_layout.vm.cpus {
        if !topo_cpus.contains(&cpu) {
            findings.push(CpuPlanValidationError::VmCpuOutsideTopology { cpu });
        }
    }

    findings
}
