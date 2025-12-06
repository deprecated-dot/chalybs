// core/src/cpu/detect/cpuid.rs
//
// Minimal CPU identity extraction using CPUID on x86/x86_64.
//
// This avoids any dependence on /proc/cpuinfo formatting or distro-
// specific quirks. Instead, it uses raw CPUID leaves to derive:
//
//   - vendor string
//   - family
//   - model
//   - stepping
//   - human-readable brand string (if available)
//
// The goal is to provide a stable, conservative CpuIdentity that can
// be classified deterministically without heuristics.
//
// Scope (Option A):
//   - On x86/x86_64, we use the `raw-cpuid` crate to interrogate the
//     hardware directly.
//   - On all other architectures, `detect_cpu_identity()` returns
//     None, and higher layers are expected to fall back to "host" or
//     explicit configuration.
//
// This matches Chalybs' design ethos:
//   - No userland or distro text parsing
//   - No dependence on kernel /proc/cpuinfo formatting
//   - Deterministic, hardware-derived CPU identity

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuVendor {
    Amd,
    Intel,
    Arm,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct CpuIdentity {
    pub vendor: CpuVendor,
    pub family: u32,
    pub model: u32,
    pub stepping: u32,
    pub raw_vendor: Option<String>,
    pub model_name: Option<String>,
}

fn detect_vendor(raw_vendor: &Option<String>, model_name: &Option<String>) -> CpuVendor {
    if let Some(v) = raw_vendor {
        if v == "AuthenticAMD" {
            return CpuVendor::Amd;
        }
        if v == "GenuineIntel" {
            return CpuVendor::Intel;
        }
    }

    // Very conservative ARM detection: rely on model name containing
    // common ARM tokens. This is deterministic string matching, not
    // probabilistic guessing.
    if let Some(name) = model_name {
        let lower = name.to_lowercase();
        if lower.contains("arm") || lower.contains("aarch64") {
            return CpuVendor::Arm;
        }
    }

    CpuVendor::Unknown
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub fn detect_cpu_identity() -> Option<CpuIdentity> {
    use raw_cpuid::CpuId;

    let cpuid = CpuId::new();

    // Vendor string, e.g. "AuthenticAMD", "GenuineIntel".
    let vendor_info = cpuid.get_vendor_info()?;
    let raw_vendor = Some(vendor_info.as_str().to_string());

    // Family / model / stepping come from the "feature info" leaf.
    let feature_info = cpuid.get_feature_info()?;

    let base_family = feature_info.family_id() as u32;
    let ext_family = feature_info.extended_family_id() as u32;
    let family = if base_family == 0x0f {
        base_family + ext_family
    } else {
        base_family
    };

    let base_model = feature_info.model_id() as u32;
    let ext_model = feature_info.extended_model_id() as u32;
    let model = if base_family == 0x06 || base_family == 0x0f {
        (ext_model << 4) + base_model
    } else {
        base_model
    };

    let stepping = feature_info.stepping_id() as u32;

    // Human-readable brand string, if available.
    let model_name = cpuid.get_processor_brand_string().and_then(|b| {
        let s = b.as_str().trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    });

    let vendor = detect_vendor(&raw_vendor, &model_name);

    Some(CpuIdentity {
        vendor,
        family,
        model,
        stepping,
        raw_vendor,
        model_name,
    })
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
pub fn detect_cpu_identity() -> Option<CpuIdentity> {
    // Option A: x86/x86_64 only for now. Non-x86 hosts are outside the
    // current support surface; callers must fall back to "host" or
    // explicit configuration.
    None
}
