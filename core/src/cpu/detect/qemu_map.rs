// core/src/cpu/detect/qemu_map.rs
//
// Mapping from CpuArch → QEMU CPU model string.
//
// This module is intentionally narrow and table-driven. It is the only
// place that "knows" about specific QEMU -cpu model names. The goal is
// to provide a small, deterministic mapping surface that can be expanded
// over time without touching qemu.rs.
//
// Assumptions / invariants:
//
//   - Chalybs is designed and validated against a **pinned QEMU build**
//     (currently 10.1.2). The mapping below assumes that catalog of
//     CPU models is available and stable.
//   - We deliberately do *not* attempt to support arbitrary, older
//     system QEMU binaries. If a user overrides the QEMU binary and it
//     lacks one of these models, that configuration is outside the
//     supported surface; higher layers may fall back to "host" instead.
//
// Current mappings:
//
//   AMD (AmdZen1OrZenPlus → Zen 3 / 4):
//
//     CpuArch::AmdZen1OrZenPlus → "EPYC-v2"
//     CpuArch::AmdZen2         → "EPYC-v3"
//     CpuArch::AmdZen3OrZen4   → "EPYC-v4"
//
//   Intel:
//
//     CpuArch::IntelCoreFamily → "Skylake-Server"
//
//   ARM:
//
//     CpuArch::Arm64Generic    → "cortex-a57"
//
//   Unknown:
//
//     CpuArch::Unknown         → None (caller must fall back)
//
// Notes:
//
//   - The AMD mapping is intentionally biased toward known-good,
//     battle-tested baselines on QEMU 10.1.2.
//   - Both Zen 3 and Zen 4 (family 19h) are currently mapped to
//     "EPYC-v4" as a unified "newer Zen" bucket.
//   - More granular Intel splits (Nehalem, Haswell, etc.) and ARM
//     variants can be added later by extending CpuArch + classify.rs
//     without touching qemu.rs.

use super::classify::CpuArch;

/// Map a classified CpuArch into a QEMU -cpu model string.
///
/// Returns:
///   - Some("<model>") on supported architectures, e.g. "EPYC-v2"
///   - None for CpuArch::Unknown, allowing callers to fall back to
///     "host" or explicit configuration.
///
/// This function is pure and deterministic: given a CpuArch value, it
/// will always return the same result for a given Chalybs release.
pub fn map_arch_to_qemu_model(arch: CpuArch) -> Option<&'static str> {
    match arch {
        CpuArch::AmdZen1OrZenPlus => Some("EPYC-v2"),
        CpuArch::AmdZen2 => Some("EPYC-v3"),
        CpuArch::AmdZen3OrZen4 => Some("EPYC-v4"),

        CpuArch::IntelCoreFamily => Some("Skylake-Server"),

        CpuArch::Arm64Generic => Some("cortex-a57"),

        CpuArch::Unknown => None,
    }
}
