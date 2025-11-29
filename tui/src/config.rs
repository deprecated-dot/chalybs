//! TUI configuration loader for global Chalybs config.
//!
//! Sources (precedence, highest first):
//!   1. Environment: `CHALYBS_TUI_LOGO_HALO`
//!   2. Global TOML: `/etc/chalybs/chalybs.toml` `[tui]` section
//!   3. Built-in default (C3)
//!
//! The goal here is **no userspace TUI-specific config files** and
//! minimal coupling: the TUI reads only the bits it cares about.

use std::env;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Profile for the PNG logo halo (Set C).
///
/// This is the single canonical enum used by:
///   - Global config (e.g. /etc/chalybs/chalybs.toml [tui] section)
///   - Environment override (CHALYBS_TUI_LOGO_HALO)
///   - Runtime shell command: `effects halo <profile>`
#[derive(Clone, Copy, Debug)]
pub enum LogoHaloProfile {
    /// No halo at all.
    Off,
    /// Standard C3 wings.
    C3,
    /// Thinner, more subtle wings.
    C3Narrow,
    /// Heavier wings closer toward the logo.
    C3Wide,
    /// Heaviest wings.
    C3ExtraWide,
}

impl LogoHaloProfile {
    /// Parse a profile name from a user/config string.
    ///
    /// Accepted (case-insensitive):
    ///
    ///   Modern names:
    ///     - "off"
    ///     - "c3"
    ///     - "c3narrow", "narrow", "c3_narrow", "s1"
    ///     - "c3wide", "wide", "c3_wide", "s2"
    ///     - "c3extrawide", "extrawide", "c3_xwide", "c3xwide", "s3"
    ///
    ///   Back-compat shims from the earlier Basic/C3/C3D set:
    ///     - "none"   → Off
    ///     - "basic"  → C3
    ///     - "c3d"    → C3ExtraWide
    pub fn from_str_kind<S: AsRef<str>>(s: S) -> Option<Self> {
        let s = s.as_ref().to_ascii_lowercase();
        match s.as_str() {
            // modern
            "off" | "none" => Some(LogoHaloProfile::Off),
            "c3" => Some(LogoHaloProfile::C3),
            "c3narrow" | "narrow" | "c3_narrow" | "s1" => Some(LogoHaloProfile::C3Narrow),
            "c3wide" | "wide" | "c3_wide" | "s2" => Some(LogoHaloProfile::C3Wide),
            "c3extrawide" | "extrawide" | "c3_xwide" | "c3xwide" | "s3" => {
                Some(LogoHaloProfile::C3ExtraWide)
            }

            // back-compat
            "basic" => Some(LogoHaloProfile::C3),
            "c3d" => Some(LogoHaloProfile::C3ExtraWide),

            _ => None,
        }
    }

    /// Stable, lower-case string for status / events.
    pub fn as_str(&self) -> &'static str {
        match self {
            LogoHaloProfile::Off => "off",
            LogoHaloProfile::C3 => "c3",
            LogoHaloProfile::C3Narrow => "c3narrow",
            LogoHaloProfile::C3Wide => "c3wide",
            LogoHaloProfile::C3ExtraWide => "c3extrawide",
        }
    }
}

/// Minimal TUI config extracted from the global chalybs.toml.
///
/// Currently only carries the halo profile, but this is where any future
/// TUI knobs that should live in the global config would go.
#[derive(Clone, Debug)]
pub struct TuiConfig {
    pub logo_halo: LogoHaloProfile,
}

/// Shape of the bits we care about in /etc/chalybs/chalybs.toml.
///
/// We deliberately only deserialize the `[tui]` table. All other keys
/// are ignored so the core config schema can evolve independently.
#[derive(Debug, Deserialize)]
struct RawRootConfig {
    #[serde(default)]
    tui: Option<RawTuiSection>,
}

#[derive(Debug, Deserialize)]
struct RawTuiSection {
    #[serde(default)]
    logo_halo: Option<String>,
}

impl TuiConfig {
    /// Load TUI config using the precedence rules described above.
    ///
    /// If nothing is configured, returns `None` and callers should fall
    /// back to their own defaults (which for halo is `C3`).
    pub fn load() -> Option<Self> {
        // 1) Environment override.
        if let Some(profile) = halo_from_env() {
            return Some(TuiConfig { logo_halo: profile });
        }

        // 2) Global TOML.
        if let Some(profile) = halo_from_global_toml() {
            return Some(TuiConfig { logo_halo: profile });
        }

        // 3) No explicit config; let callers use their own defaults.
        None
    }
}

/// Try to parse CHALYBS_TUI_LOGO_HALO.
fn halo_from_env() -> Option<LogoHaloProfile> {
    let raw = env::var("CHALYBS_TUI_LOGO_HALO").ok()?;
    LogoHaloProfile::from_str_kind(raw.trim())
}

/// Try to extract `[tui].logo_halo` from /etc/chalybs/chalybs.toml.
fn halo_from_global_toml() -> Option<LogoHaloProfile> {
    let path = Path::new("/etc/chalybs/chalybs.toml");
    let data = fs::read_to_string(path).ok()?;
    let root: RawRootConfig = toml::from_str(&data).ok()?;
    let section = root.tui?;
    let raw = section.logo_halo?;
    LogoHaloProfile::from_str_kind(raw.trim())
}
