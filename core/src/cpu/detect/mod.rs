// core/src/cpu/detect/mod.rs
//
// Host CPU detection and classification subsystem.
//
// This module is responsible for:
//   - Discovering basic CPU identity via raw CPUID on x86/x86_64
//     (see cpuid.rs for details).
//   - Classifying the host into a coarse CpuArch family.
//   - Mapping CpuArch into a QEMU CPU model string for -cpu.
//   - (Foundational) Discovering host NUMA topology as a basis for
//     future CPU/NUMA/hugepage validation.
//
// It is intentionally conservative and table-driven. Unknown or
// unsupported hosts return None, allowing callers to fall back to
// "host" or explicit config values without guessing.

mod classify;
mod cpuid;
mod qemu_map;
mod topology;

pub use classify::CpuArch;
pub use cpuid::{CpuIdentity, CpuVendor};
pub use topology::{HostNumaNode, HostNumaTopology};

/// Attempt to derive a suitable QEMU CPU model string for the host.
///
/// Behavior:
///   - On success, returns Some(model_string), e.g. "EPYC-v2".
///   - On failure or unknown host, returns None and the caller must
///     fall back to "host" or an explicit cpu_model.
///
/// This function is side-effect free except for logging.
pub fn autodetect_qemu_cpu_model() -> Option<String> {
    let ident = match cpuid::detect_cpu_identity() {
        Some(id) => id,
        None => {
            tracing::info!(
                "cpu_detect: CPUID did not yield a usable CPU identity; \
                 leaving CPU model undefined"
            );
            return None;
        }
    };

    let arch = classify::classify(&ident);
    let model = match qemu_map::map_arch_to_qemu_model(arch) {
        Some(m) => m,
        None => {
            tracing::info!(
                ?ident,
                ?arch,
                "cpu_detect: no QEMU CPU model mapping for this host; caller must fall back"
            );
            return None;
        }
    };

    // Observability-only NUMA telemetry for now.
    topology::log_host_numa_topology(&ident, arch);

    tracing::info!(
        ?ident,
        ?arch,
        model = model,
        "cpu_detect: mapped host CPU to QEMU CPU model"
    );

    Some(model.to_string())
}

/// Low-level helper: detect host CPU identity via raw CPUID on
/// supported architectures.
///
/// This is a thin wrapper over the internal cpuid module so that
/// higher layers do not need to reach into private submodules.
pub fn detect_cpu_identity() -> Option<CpuIdentity> {
    cpuid::detect_cpu_identity()
}

/// Low-level helper: classify a CpuIdentity into a CpuArch bucket.
///
/// This is a thin wrapper over the internal classifier to keep the
/// public surface small and stable.
pub fn classify_cpu(ident: &CpuIdentity) -> CpuArch {
    classify::classify(ident)
}
