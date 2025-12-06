// core/src/cpuplan/builder.rs
//
// CpuPlanBuilder: pure construction of CpuPlan from already-
// discovered host signals.
//
// Design:
//   - This module does **not** read /proc, sysfs, or run CPUID.
//   - It does **not** apply policy or emit warnings/errors.
//   - It accepts fully-populated, deterministic inputs:
//       * CpuIdentity          (from cpu::detect::cpuid)
//       * CpuArch              (from cpu::detect::classify)
//       * HostNumaTopology     (from cpu::detect::topology)
//       * VmCpuLayout          (from config parsing)
//   - It returns a CpuPlan, which is a pure data model.
//
// The intent is to keep the layering clean:
//
//   cpu::detect/*   → talks to hardware + sysfs, logs telemetry
//   cpuplan::builder→ folds signals into a single CpuPlan struct
//   validators      → consume CpuPlan and apply explicit rules
//
// No Linux paths, no syscalls, no tracing here; just structure.

use std::collections::HashSet;

use crate::cpu::detect::{CpuArch, CpuIdentity, HostNumaTopology};
use crate::model::VmCpuLayout;

use super::{CpuPlan, CpuPlanNode};

/// Immutable inputs required to build a CpuPlan.
///
/// These are meant to be produced by the detection layer and config
/// parsing, then handed to the planning/validation layer without any
/// further host-side IO.
#[derive(Debug, Clone)]
pub struct CpuPlanInputs {
    /// Raw CPUID-derived identity.
    pub ident: CpuIdentity,
    /// Chalybs-classified architecture bucket.
    pub arch: CpuArch,
    /// Host NUMA topology discovered from sysfs.
    pub topology: HostNumaTopology,
    /// VM CPU layout derived from config.
    pub vm_layout: VmCpuLayout,
}

impl CpuPlanInputs {
    pub fn new(
        ident: CpuIdentity,
        arch: CpuArch,
        topology: HostNumaTopology,
        vm_layout: VmCpuLayout,
    ) -> Self {
        Self {
            ident,
            arch,
            topology,
            vm_layout,
        }
    }
}

/// Build an immutable CpuPlan from precomputed inputs.
///
/// This function is pure and deterministic:
///   - It never touches sysfs, /proc, or CPUID.
///   - It performs only in-memory set operations to derive:
///       * per-node host + VM CPU membership
///       * whether the VM spans multiple nodes
///       * which node (if any) is "dominant" for VM CPUs
pub fn build_cpu_plan(inputs: CpuPlanInputs) -> CpuPlan {
    // Precompute VM CPU membership as a set for fast intersection.
    let vm_cpu_set: HashSet<u32> = inputs.vm_layout.vm.cpus.iter().copied().collect();

    let mut nodes: Vec<CpuPlanNode> = Vec::new();

    let mut vm_node_count = 0usize;
    let mut dominant_node: Option<u32> = None;
    let mut dominant_vm_count: usize = 0;

    for node in &inputs.topology.nodes {
        // Host CPUs for this NUMA node come directly from the topology.
        let mut host_cpus = node.cpus.clone();
        host_cpus.sort_unstable();
        host_cpus.dedup();

        // VM CPUs on this node are the intersection of host_cpus and
        // the VM's CPU set.
        let mut vm_cpus: Vec<u32> = host_cpus
            .iter()
            .copied()
            .filter(|cpu| vm_cpu_set.contains(cpu))
            .collect();

        vm_cpus.sort_unstable();
        vm_cpus.dedup();

        if !vm_cpus.is_empty() {
            vm_node_count += 1;

            if vm_cpus.len() > dominant_vm_count {
                dominant_vm_count = vm_cpus.len();
                dominant_node = Some(node.node_id);
            }
        }

        nodes.push(CpuPlanNode {
            node_id: node.node_id,
            host_cpus,
            vm_cpus,
        });
    }

    let vm_spans_nodes = vm_node_count > 1;

    CpuPlan {
        ident: inputs.ident,
        arch: inputs.arch,
        topology: inputs.topology,
        vm_layout: inputs.vm_layout,
        nodes,
        vm_spans_nodes,
        dominant_node,
    }
}
