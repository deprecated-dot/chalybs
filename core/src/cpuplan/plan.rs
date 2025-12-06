// core/src/cpuplan/plan.rs
//
// Canonical, immutable CPU plan.
//
// This is the pure data model for Chalybs’ CPU planning layer.
// It does **not** derive information, mutate state, touch sysfs,
// parse cpusets, or apply policy. It is a structured container
// produced by CpuPlanBuilder and consumed by validators and the
// state machine.
//
// A CpuPlan captures:
//
//   - Host CPU identity        (vendor, family, model, brand)
//   - Host CpuArch             (Zen1, Zen2, Zen3/4, Intel, etc.)
//   - Host NUMA topology       (node → cpus[])
//   - VM CPU layout            (VmCpuLayout: host vs vm)
//   - Derived per-node mapping (which VM/host CPUs lie on which NUMA)
//   - High-level plan metadata (summaries, invariants)
//
// This file contains *no* logic that depends on Linux, sysfs, or
// CPU detection; it is a pure Rust data model.

use crate::cpu::detect::{CpuArch, CpuIdentity, HostNumaTopology};
use crate::model::VmCpuLayout;

/// A single NUMA node description in the final CPU plan.
///
/// This includes the host CPUs on this node, and also which of those
/// are selected as VM CPUs (intersection with vm_cpus).
#[derive(Debug, Clone)]
pub struct CpuPlanNode {
    /// NUMA node identifier (0,1,2,...)
    pub node_id: u32,

    /// All host CPUs that exist on this NUMA node.
    pub host_cpus: Vec<u32>,

    /// VM CPUs that reside on this node (intersection of host_cpus
    /// with vm CPU set).
    pub vm_cpus: Vec<u32>,
}

/// Immutable, deterministic description of the full CPU plan.
///
/// This is deliberately “dumb data”: no logic, no syscalls.
/// Builders and validators use this structure; other subsystems
/// (hugepages, cpuset, QEMU launch) can query it without re-deriving
/// topology or classification.
#[derive(Debug, Clone)]
pub struct CpuPlan {
    /// Raw CPUID-derived identity (vendor/family/model/brand).
    pub ident: CpuIdentity,

    /// Chalybs-classified architecture bucket (Zen1/2/3/4/etc.).
    pub arch: CpuArch,

    /// NUMA topology directly from sysfs (if available).
    pub topology: HostNumaTopology,

    /// CPU layout from VM config (host CPUs, vm CPUs).
    pub vm_layout: VmCpuLayout,

    /// Node-level breakdown (one CpuPlanNode per NUMA node).
    pub nodes: Vec<CpuPlanNode>,

    /// Whether the VM’s CPU set crosses multiple NUMA nodes.
    ///
    /// This is informational; enforcement lives in validators.
    pub vm_spans_nodes: bool,

    /// Optional: which node has the highest VM CPU density.
    ///
    /// This is used by some validators (and potentially by
    /// future hugepage-node inference logic when configs
    /// explicitly opt-in).
    pub dominant_node: Option<u32>,
}

impl CpuPlan {
    /// Look up a CpuPlanNode by node_id.
    pub fn node(&self, id: u32) -> Option<&CpuPlanNode> {
        self.nodes.iter().find(|n| n.node_id == id)
    }

    /// Return a flat Vec of all VM CPUs in numeric order.
    pub fn vm_cpus(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self.nodes.iter().flat_map(|n| n.vm_cpus.iter().copied()).collect();
        out.sort_unstable();
        out
    }

    /// Return a flat Vec of all host CPUs referenced by the VM layout.
    pub fn host_cpus(&self) -> Vec<u32> {
        let mut out: Vec<u32> = self.vm_layout.host.cpus.clone();
        out.sort_unstable();
        out
    }
}
