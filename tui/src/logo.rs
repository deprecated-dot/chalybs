use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::VisualEffects;
use crate::theme;
use std::f32::consts::PI;

// PNG renderer lives in `tui/src/logo_png.rs` as a sibling module.
// Declared at crate root via `mod logo_png;`.
use crate::logo_png;

/// ---------------------------------------------------------------------------
/// PUBLIC BREATHING SIGNALS
/// ---------------------------------------------------------------------------
///
/// These provide stable, deterministic, long-lived signals that the rest of
/// the TUI can use for coherent animation rhythms. They never mutate internal
/// logo behavior and never alter existing rendering logic.
///
/// `logo_breath_factor`
///     Primary brightness modulation factor for logo + sparkline coherence.
///     Range: ~0.88..1.12 (smoothly eased)
///
/// `logo_breath_coherence`
///     A second harmonic-like signal useful for panel border aura,
///     matrix drift coherence, sparkline offset shaping, etc.
///     Range: ~0.0..1.0 (eased)
///

/// Shared breathing curve for the logo + TUI coherence.
///
/// Returns a brightness factor in roughly [0.88, 1.12], using a
/// sinusoidal base shaped by a smooth cubic easing. Deterministic.
pub fn logo_breath_factor(tick: u64) -> f32 {
    let period: f32 = 120.0;
    let t = tick as f32;
    let angle = (t * (2.0 * PI / period)) % (2.0 * PI);

    // Base sinusoid: -1..1
    let wave = angle.sin();

    // Normalize to 0..1
    let raw = (wave + 1.0) * 0.5;

    // Smooth cubic easing (ease-in-out)
    let eased = raw * raw * (3.0 - 2.0 * raw);

    // Map eased value back to brightness ~0.88..1.12
    1.0 + (eased - 0.5) * 0.24
}

/// Secondary coherence function:
///
/// A smoothly eased 0..1 factor derived from a slightly phase-advanced
/// variant of the breathing wave. This is used by the UI sparkline,
/// matrix drift alignment, and panel aura shaping.
///
/// Deterministic and side-effect-free.
pub fn logo_breath_coherence(tick: u64, salt: u64) -> f32 {
    let period: f32 = 120.0;
    let t = tick as f32 + (salt as f32 * 7.31); // per-caller salt
    let angle = (t * (2.0 * PI / period)) % (2.0 * PI);

    let wave = angle.sin();
    let raw = (wave + 1.0) * 0.5;

    // Same cubic smoothing as main breathing
    raw * raw * (3.0 - 2.0 * raw)
}

/// ---------------------------------------------------------------------------
/// INTERNAL BREATHING LOGIC (PER GLYPH)
/// ---------------------------------------------------------------------------

/// Deterministic tiny RNG from (tick, salt).
fn small_rng(tick: u64, salt: u64) -> u64 {
    let mut x = tick ^ (salt.wrapping_mul(0x9E37_79B1_85EB_CA87));
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x.wrapping_mul(0x2545_F491_4F6C_DD1D)
}

/// Breathing factor with per-glyph salt + micro-jitter on rising edge.
///
/// This preserves your original logo breathing style exactly while introducing
/// internal micro-jitter that stays *purely internal* to logo rendering
/// (sparkline uses only the public, clean `logo_breath_factor`).
fn compute_breath_factor_with_salt(tick: u64, salt: u64) -> f32 {
    let base = logo_breath_factor(tick);

    let period: f32 = 120.0;
    let t = tick as f32 + (salt as f32 * 3.17);
    let angle = (t * (2.0 * PI / period)) % (2.0 * PI);

    let rising = angle < PI;

    if rising {
        let r = small_rng(tick, salt) & 0xFF;
        let jitter = ((r as f32) / 255.0) * 0.02 - 0.01; // [-0.01 .. +0.01]
        (base + jitter).max(0.8).min(1.2)
    } else {
        base
    }
}

/// Internal helper: compute a gently "breathing" style for the rune glyph,
/// driven by `tick` and `effects.logo_reactive`.
fn logo_rune_style(tick: u64, effects: &VisualEffects) -> ratatui::style::Style {
    let base_color = crate::theme::palette::ACCENT_PINK;

    if !effects.logo_reactive {
        // Non-reactive: match the original static style exactly.
        return theme::header_title().fg(base_color);
    }

    // Single glyph for now, but kept general.
    let salt = 7_u64;
    let factor = compute_breath_factor_with_salt(tick, salt);

    // Apply brightness shaping to the base accent colors.
    let bright_pink = crate::theme::adjust_brightness(base_color, factor);

    // Preserve the original phased structure (DIM/neutral/BOLD + occasional purple).
    let phase = (tick / 8) % 16;

    match phase {
        0 | 1 => theme::header_title()
            .fg(bright_pink)
            .add_modifier(Modifier::DIM),

        2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 => theme::header_title().fg(bright_pink),

        10 | 11 | 12 => theme::header_title()
            .fg(bright_pink)
            .add_modifier(Modifier::BOLD),

        _ => {
            let purple = crate::theme::palette::ACCENT_PURPLE;
            let bright_purple = crate::theme::adjust_brightness(purple, factor);
            theme::header_title()
                .fg(bright_purple)
                .add_modifier(Modifier::BOLD)
        }
    }
}

/// ---------------------------------------------------------------------------
/// DRAWING THE LOGO
/// ---------------------------------------------------------------------------

/// Public logo renderer used by the TUI.
///
/// Hybrid behaviour:
///   - We *offer* the upper portion of the region to the PNG renderer
///     (`logo_png::draw_png_logo`).
///   - If it returns `true`, we still render a breathing "CHALYBS ⟐"
///     caption in the bottom rows (no extra tagline).
///   - If PNG is unavailable, we fall back to the full ASCII logo in
///     the entire region (exactly as before).
pub fn draw_logo(f: &mut Frame, area: Rect, tick: u64, effects: &VisualEffects) {
    // If the area is too small to split, keep behaviour simple:
    // try PNG once, else draw full ASCII.
    if area.height <= 3 {
        if logo_png::draw_png_logo(f, area, tick, effects) {
            return;
        }
        draw_ascii_logo(f, area, tick, effects);
        return;
    }

    // Normal case: 7-row status slot.
    //
    // Reserve the *bottom 2 rows* for the breathing caption, and
    // give the remaining top rows to the PNG renderer.
    let caption_height: u16 = 2;
    let png_height = area.height.saturating_sub(caption_height);

    let png_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: png_height,
    };

    let caption_area = Rect {
        x: area.x,
        y: area.y + png_height,
        width: area.width,
        height: caption_height,
    };

    let used_png = logo_png::draw_png_logo(f, png_area, tick, effects);

    if used_png {
        // Hybrid path: PNG above, breathing caption below.
        draw_logo_caption(f, caption_area, tick, effects);
    } else {
        // PNG unavailable or disabled: use the full ASCII logo exactly
        // as before (including the 3-line rune mark).
        draw_ascii_logo(f, area, tick, effects);
    }
}

/// ASCII logo renderer: the original implementation, unchanged.
///
/// Separated out so we can call it explicitly from `AsciiLogoRenderer`
/// and from the PNG dispatcher as a stable fallback.
fn draw_ascii_logo(f: &mut Frame, area: Rect, tick: u64, effects: &VisualEffects) {
    let rune_style = logo_rune_style(tick, effects);

    let title = Line::from(vec![
        Span::styled("CHALYBS ", theme::header_title()),
        Span::styled("⟐", rune_style),
    ]);

    // A simple "runic slash C" undermark.
    let rune_lines = vec![
        Line::from(Span::styled(
            "   ╱╲",
            theme::normal_text().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("  ╱ ", theme::normal_text().add_modifier(Modifier::BOLD)),
            Span::styled("C", theme::header_title()),
            Span::styled(" ╲", theme::normal_text().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(Span::styled(
            " ╱   ╲",
            theme::normal_text().add_modifier(Modifier::BOLD),
        )),
    ];

    let mut text = Vec::new();
    text.push(title);
    text.push(Line::from(""));
    text.extend(rune_lines);

    let block = Block::default()
        .borders(Borders::NONE)
        .style(theme::dim_text());

    let paragraph = Paragraph::new(text).block(block);

    f.render_widget(paragraph, area);
}

/// Compact dual-tone caption used in hybrid PNG mode.
///
/// This deliberately fits in the reserved caption rows:
///   - "CHALYBS ⟐" with breathing rune.
///   - **No extra tagline** (tagline already lives in the header).
///
/// And now (Option A): **fully transparent background**, allowing PNG halo
/// to shine through.
fn draw_logo_caption(f: &mut Frame, area: Rect, tick: u64, effects: &VisualEffects) {
    let rune_style = logo_rune_style(tick, effects);

    let line = Line::from(vec![
        Span::styled("CHALYBS ", theme::header_title()),
        Span::styled("⟐", rune_style),
    ]);

    // *** Option A transparency fix ***
    // Style ONLY sets FG; no BG. No block background either.
    let style = Style::default().fg(theme::palette::TEXT_DIM);

    let block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default());

    let paragraph = Paragraph::new(vec![line]).style(style).block(block);

    f.render_widget(paragraph, area);
}

/// ---------------------------------------------------------------------------
/// FUTURE EXTENSION: Renderers
/// ---------------------------------------------------------------------------

pub trait LogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect);
}

pub struct AsciiLogoRenderer;

impl LogoRenderer for AsciiLogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect) {
        let effects = VisualEffects::default_enabled();
        // Explicitly use ASCII helper so this renderer is *always* the
        // text-based version, even once PNG is fully wired.
        draw_ascii_logo(f, area, 0, &effects);
    }
}

#[cfg(feature = "kitty-graphics")]
pub struct KittyImageLogoRenderer;

#[cfg(feature = "kitty-graphics")]
impl LogoRenderer for KittyImageLogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect) {
        // For now this still just renders a stub; the main PNG path is
        // handled via `draw_logo` / `logo_png::draw_png_logo`.
        let block = Block::default().borders(Borders::NONE);
        let text = Paragraph::new("kitty logo renderer (stub)").block(block);
        f.render_widget(text, area);
    }
}
