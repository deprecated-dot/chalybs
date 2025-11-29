//! Deterministic, purely cosmetic glitch profile for the TUI.
//!
//! This module never mutates application state. It derives a rare,
//! short-lived "glitch window" from the global tick counter and
//! exposes helpers the UI can use to:
//!
//! - jitter ticks (phase jitter / ghost-frame feel)
//! - apply region masks
//! - inject pseudo-ANSI junk
//! - add small spatial offsets
//!
//! All behaviour is deterministic: given the same tick stream, the
//! same glitch sequence will occur.

#[derive(Clone, Copy, Debug)]
pub enum GlitchMode {
    ColorInvert,
    PhaseJitter,
    GhostFrame,
    JunkInjection,
    SpatialJitter,
}

#[derive(Clone, Copy, Debug)]
pub struct GlitchProfile {
    pub active: bool,
    pub strength: u8, // 0..3
    pub mode: GlitchMode,
    pub regions: u8, // bitmask of REGION_* constants
    pub phase: u8,   // small 0..2 index within a glitch burst
}

// Region bitmasks. Multiple can be set at once.
pub const REGION_HEADER: u8 = 0b0000_0001;
pub const REGION_VMS: u8 = 0b0000_0010;
pub const REGION_EVENTS: u8 = 0b0000_0100;
pub const REGION_SHELL: u8 = 0b0000_1000;
pub const REGION_BORDERS: u8 = 0b0001_0000;
pub const REGION_SPARK: u8 = 0b0010_0000;

impl GlitchProfile {
    pub fn inactive() -> Self {
        GlitchProfile {
            active: false,
            strength: 0,
            mode: GlitchMode::PhaseJitter,
            regions: 0,
            phase: 0,
        }
    }
}

/// DEBUG MULTIPLIER
///
/// This lets you dramatically increase glitch frequency while tuning.
/// It preserves deterministic output. Production mode = 0.
///
/// Meaning:
///   0 → 1/256     (default production behavior)
///   1 → 1/128
///   2 → 1/64
///   3 → 1/32
///   4 → 1/16
///   5 → 1/8       (fairly wild)
///
/// Safety: This does NOT affect which glitch mode/regions occur,
/// only the frequency of opportunities for a burst.
///
/// Set this to a higher number while testing UI glitch visibility.
const GLITCH_DEBUG_MULTIPLIER: u8 = 4;

/// Simple 64-bit mixing function used to derive pseudo-random-looking
/// values from a coarse tick.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
    x
}

/// Compute the glitch profile for a given tick.
///
/// Frequency is influenced by GLITCH_DEBUG_MULTIPLIER but still
/// remains deterministic.
pub fn glitch_profile(tick: u64) -> GlitchProfile {
    // Avoid doing anything during the very first frames after startup.
    if tick < 128 {
        return GlitchProfile::inactive();
    }

    // Coarse tick → multi-frame burst window
    let coarse = tick / 3;
    let seed = mix64(coarse);

    // Debug-adjusted mask:
    //
    // For multiplier M:
    //     effective_mask = 0xFF >> M
    //
    // Example:
    //   M=0 → mask=0xFF (1/256)
    //   M=1 → mask=0x7F (1/128)
    //   M=2 → mask=0x3F (1/64)
    //   M=3 → mask=0x1F (1/32)
    //
    let shift = GLITCH_DEBUG_MULTIPLIER.min(7); // avoid shifting past 0
    let effective_mask = 0xFFu64 >> shift;

    // Rare chance of activating a glitch.
    if (seed & effective_mask) != 0 {
        return GlitchProfile::inactive();
    }

    // Strength in 1..3.
    let mut strength = ((seed >> 8) & 0x3) as u8 + 1;
    if strength > 3 {
        strength = 3;
    }

    // Mode index 0..4 mapped to the five glitch types.
    let mode_idx = ((seed >> 16) % 5) as u8;
    let mode = match mode_idx {
        0 => GlitchMode::ColorInvert,
        1 => GlitchMode::PhaseJitter,
        2 => GlitchMode::GhostFrame,
        3 => GlitchMode::JunkInjection,
        _ => GlitchMode::SpatialJitter,
    };

    // Region mask: avoid the shell by default.
    let mut regions: u8 = 0;
    let rbits = (seed >> 24) & 0xFF;

    if (rbits & 0x01) != 0 {
        regions |= REGION_HEADER;
    }
    if (rbits & 0x02) != 0 {
        regions |= REGION_VMS;
    }
    if (rbits & 0x04) != 0 {
        regions |= REGION_EVENTS;
    }
    if (rbits & 0x08) != 0 {
        regions |= REGION_BORDERS;
    }
    if (rbits & 0x10) != 0 {
        regions |= REGION_SPARK;
    }

    // Always ensure at least one region participates.
    if regions == 0 {
        regions = REGION_EVENTS;
    }

    // Phase within the 3-frame coarse window.
    let phase = (tick % 3) as u8;

    GlitchProfile {
        active: true,
        strength,
        mode,
        regions,
        phase,
    }
}

/// Check whether the glitch profile is active for a given region mask.
pub fn affects_region(profile: &GlitchProfile, region_mask: u8) -> bool {
    profile.active && (profile.regions & region_mask) != 0
}

/// Adjust a base tick for a particular region, depending on the glitch mode.
///
/// - PhaseJitter: push the tick forward slightly to de-phase motion.
/// - GhostFrame: pull the tick backward to create a "stale" frame feel.
/// - Others: no tick adjustment.
pub fn tick_for_region(base_tick: u64, profile: &GlitchProfile, region_mask: u8) -> u64 {
    if !affects_region(profile, region_mask) {
        return base_tick;
    }

    match profile.mode {
        GlitchMode::PhaseJitter => {
            // Mild forward jerk based on phase and strength.
            let delta = (profile.phase as u64 + 1) * (3 + profile.strength as u64 * 2);
            base_tick.wrapping_add(delta)
        }
        GlitchMode::GhostFrame => {
            // Pull back by 1..3 ticks, clamped to avoid underflow panic.
            let back = (profile.phase as u64 + 1).min(3);
            base_tick.saturating_sub(back)
        }
        _ => base_tick,
    }
}

/// Small horizontal jitter in [-1, 1] for spatial offsets.
///
/// Only active in SpatialJitter mode and when the region is affected.
pub fn space_jitter(profile: &GlitchProfile, region_mask: u8, lane: u16) -> i8 {
    if !affects_region(profile, region_mask) {
        return 0;
    }

    if !matches!(profile.mode, GlitchMode::SpatialJitter) {
        return 0;
    }

    // Derive a tiny offset from a local seed.
    let local = mix64(
        (lane as u64)
            .wrapping_mul(0x9E37_79B1_85EB_CA87)
            .wrapping_add(profile.phase as u64 * 31),
    );

    match (local & 0x3) as u8 {
        0 => -1,
        1 => 1,
        _ => 0,
    }
}

/// Optional pseudo-ANSI junk prefix for a given row in the events panel.
///
/// Only active when:
/// - glitch is active
/// - mode == JunkInjection
/// - region includes EVENTS
/// - a rare per-row condition is met
pub fn junk_prefix(row_index: usize, tick: u64, profile: &GlitchProfile) -> Option<String> {
    if !affects_region(profile, REGION_EVENTS) {
        return None;
    }

    if !matches!(profile.mode, GlitchMode::JunkInjection) {
        return None;
    }

    // Keep this very sparse: only some rows in some glitch bursts.
    let key = (row_index as u64)
        .wrapping_add(tick.wrapping_mul(17))
        .wrapping_add(profile.phase as u64 * 37);
    let seed = mix64(key);

    // About 1 in 8 rows during a JunkInjection burst get junk.
    if (seed & 0x7) != 0 {
        return None;
    }

    // Build a short string (2..4 glyphs) of pseudo-ANSI noise.
    const GLYPHS: &[char] = &[
        '░', '▒', '▓', '█', '▀', '▄', '▌', '▐', '≡', '≣', '◆', '◈', '▪', '▫',
    ];

    let count = 2 + ((seed >> 8) % 3) as usize; // 2..4
    let mut out = String::new();

    for i in 0..count {
        let idx = ((seed >> (12 + i * 4)) & 0xF) as usize;
        let g = GLYPHS[idx % GLYPHS.len()];
        out.push(g);
    }

    Some(out)
}
