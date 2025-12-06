// core/src/cpuplan/mod.rs
//
// CPU planning subsystem.
//
// This module sits *above* raw CPU detection and NUMA topology
// discovery and *below* the VM state machine / QEMU launcher. Its
// role is to take:
//
//   - Host CPU identity (via crate::cpu::detect::CpuIdentity)
//   - Host NUMA topology (via crate::cpu::detect::HostNumaTopology)
//   - VM CPU layout (VmCpuLayout: host vs. vm CPU sets)
//
// …and produce an immutable, deterministic "CPU plan" that other
// subsystems can consume without re-deriving topology.
//
// Design goals:
//
//   - Pure and table-driven where possible.
//   - No /proc parsing or userland heuristics; all host identity
//     comes from the cpu::detect subsystem (raw CPUID + sysfs).
//   - No side effects: this layer does not touch cpusets, cgroups,
//     or QEMU arguments directly.
//   - Explicit, structured error surface for validation; policy
//     (warn vs. hard-error) lives in higher layers.
//
// The CPU plan is the *foundation* for future validation phases:
//   - NUMA alignment checks (host vs. VM CPU sets).
//   - SMT/HT isolation safety when isolation/enforce is enabled.
//   - Hugepage node consistency checks (Phase 12 + 12B).
//
// For now, the builder + validator are intentionally conservative:
// they focus on constructing a stable CpuPlan structure and
// providing hooks for future, explicit rules.

mod builder;
mod plan;
mod validate;

pub use builder::{build_cpu_plan, CpuPlanInputs};
pub use plan::{CpuPlan, CpuPlanNode};
pub use validate::{validate_cpu_plan, CpuPlanValidationError};
