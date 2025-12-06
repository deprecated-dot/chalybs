// core/src/cpu/mod.rs

//! Phase 10: CPU topology / cpuset / vCPU pinning orchestration.
//!
//! This module provides a single, coherent surface for CPU-related
//! orchestration in the VM lifecycle, while delegating all concrete
//! work to the existing cpuset + affinity subsystems.
//!
//! Design goals:
//!   - No behavior drift: existing cpuset/affinity semantics are
//!     preserved exactly.
//!   - Deterministic entrypoints that the state machine can call for
//!     each bring-up / shutdown phase.
//!   - Future-proof: this is where we will later introduce explicit
//!     CPU affinity "plans" and restore summaries (Phase 10+), without
//!     touching the state machine again.

pub mod detect;

use crate::errors::Result;
use crate::model::VmRuntime;

/// Reserve CPUs for this VM by creating the cpuset hierarchy.
///
/// This is a thin wrapper around `cpuset::create_cpuset`, preserving
/// existing semantics and error behavior.
pub fn reserve_cpus(rt: &mut VmRuntime) -> Result<()> {
    crate::cpuset::create_cpuset(rt)
}

/// Wait for QEMU threads to appear so they can be pinned.
///
/// This delegates to `affinity::wait_for_qemu_threads` unchanged.
pub fn wait_for_qemu_threads(rt: &VmRuntime) -> Result<()> {
    crate::affinity::wait_for_qemu_threads(rt)
}

/// Pin vCPU threads to the VM's cpuset based on CPU layout.
///
/// This delegates to `affinity::pin_vcpus` unchanged.
pub fn pin_vcpus(rt: &VmRuntime) -> Result<()> {
    crate::affinity::pin_vcpus(rt)
}

/// Cleanup CPU-related resources for this VM on shutdown.
///
/// Currently this is a direct wrapper around `cpuset::destroy_cpuset`.
/// In the future, this is the natural place to add more elaborate
/// restore semantics (e.g. restoring original affinities) without
/// changing the state machine.
pub fn cleanup_cpus(rt: &mut VmRuntime) -> Result<()> {
    crate::cpuset::destroy_cpuset(rt)
}
