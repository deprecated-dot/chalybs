// core/src/cpu/detect/classify.rs
//
// Deterministic classification of CpuIdentity into a coarse CpuArch.
//
// This layer does NOT know anything about QEMU. It only reasons about
// CPU vendor + family (+ model, where safely known) and produces a
// Chalybs-internal "architecture class".
//
// Design and invariants:
//
//   - Classification is based on *architectural* signals derived from
//     CPUID (via core/src/cpu/detect/cpuid.rs), not distro/userland
//     formatting of /proc/cpuinfo.
//   - The mapping is deliberately conservative. Unknown or ambiguous
//     values are classified as CpuArch::Unknown rather than guessed.
//   - The enum is structured to support a richer AMD Zen split while
//     keeping Intel and ARM conservative for now.
//
// Current AMD mapping (summary):
//
//   vendor = Amd:
//
//     family = 0x17 (23 decimal) → pre-Zen3 "17h" generation:
//       - model <  0x30 → AmdZen1OrZenPlus
//       - model >= 0x30 → AmdZen2
//
//     family = 0x19 (25 decimal) → "19h" generation:
//       - all models    → AmdZen3OrZen4
//
//   Anything else AMD → Unknown.
//
// Intel and ARM:
//
//   - Intel family == 6 → IntelCoreFamily
//   - ARM               → Arm64Generic
//   - Everything else   → Unknown
//
// We can refine Intel (Nehalem → Skylake, etc.) and ARM later in a
// table-driven way without touching QEMU wiring; only CpuArch and this
// classifier would need updates.

use super::cpuid::{CpuIdentity, CpuVendor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuArch {
    /// AMD Zen / Zen+ era CPUs within family 0x17 (17h) with lower model
    /// numbers. This bucket is intentionally broad but safely covers the
    /// earlier 17h designs.
    AmdZen1OrZenPlus,

    /// AMD Zen 2 era CPUs within family 0x17 (17h) with higher model
    /// numbers. Still part of the 17h family, but architecturally closer
    /// to the Zen 2 generation.
    AmdZen2,

    /// AMD Zen 3 / Zen 4 era CPUs within family 0x19 (19h). For the
    /// purposes of QEMU CPU models, these are currently treated as a
    /// unified "new generation" bucket.
    AmdZen3OrZen4,

    /// Intel "Core" era CPUs (family == 6). This is intentionally coarse
    /// for now and can be split into Nehalem/Haswell/Skylake later with
    /// explicit tables.
    IntelCoreFamily,

    /// 64-bit ARM CPUs.
    Arm64Generic,

    /// Anything not covered by the above.
    Unknown,
}

pub fn classify(ident: &CpuIdentity) -> CpuArch {
    match ident.vendor {
        CpuVendor::Amd => classify_amd(ident),
        CpuVendor::Intel => classify_intel(ident),
        CpuVendor::Arm => CpuArch::Arm64Generic,
        CpuVendor::Unknown => CpuArch::Unknown,
    }
}

fn classify_amd(ident: &CpuIdentity) -> CpuArch {
    match ident.family {
        // AMD Family 17h (0x17, 23 decimal): Zen / Zen+ / Zen 2 era.
        //
        // We use a coarse model split to avoid overfitting to incomplete
        // tables:
        //   - "early" models   → Zen / Zen+
        //   - "later" models   → Zen 2
        //
        // This is intentionally conservative and can be refined in a
        // table-driven way once we accumulate more coverage.
        0x17 => {
            if ident.model < 0x30 {
                CpuArch::AmdZen1OrZenPlus
            } else {
                CpuArch::AmdZen2
            }
        }

        // AMD Family 19h (0x19, 25 decimal): Zen 3 / Zen 4 era.
        //
        // Documentation and public discussion indicate both Zen 3 and
        // Zen 4 report as family 19h with differing model numbers. For
        // Chalybs' current purposes, we treat these as a single "new
        // generation" bucket.
        0x19 => CpuArch::AmdZen3OrZen4,

        // Any other AMD family is currently outside our conservative
        // coverage and is treated as unknown.
        _ => CpuArch::Unknown,
    }
}

fn classify_intel(ident: &CpuIdentity) -> CpuArch {
    // Modern Intel Core, Xeon, etc. generally report family 6.
    // We intentionally do not try to distinguish micro-architectures
    // by model here yet; that would require an explicit CPUID model
    // table. For now, treat family 6 as a unified IntelCoreFamily and
    // fall back for others.
    if ident.family == 6 {
        CpuArch::IntelCoreFamily
    } else {
        CpuArch::Unknown
    }
}
