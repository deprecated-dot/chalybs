use ratatui::style::{Color, Modifier, Style};

/// Palette derived from the current "perfect" Chalybs logo.
///
/// Intentionally compact: just enough to cover the TUI surfaces
/// without locking us into any particular style forever.
pub mod palette {
    use super::Color;

    // Deep background, meant to sit comfortably on a dark, semi-transparent
    // terminal without clashing.
    pub const BG: Color = Color::Rgb(8, 10, 16);
    pub const PANEL_BG: Color = Color::Rgb(12, 16, 24);

    // Logo-driven accents.
    pub const ACCENT_TEAL: Color = Color::Rgb(0, 220, 200);
    pub const ACCENT_PINK: Color = Color::Rgb(255, 45, 135);
    pub const ACCENT_PURPLE: Color = Color::Rgb(160, 80, 200);

    // Semantic colors.
    pub const SUCCESS: Color = Color::Rgb(120, 230, 160);
    pub const WARNING: Color = Color::Rgb(255, 190, 90);
    pub const ERROR: Color = Color::Rgb(255, 100, 100);

    pub const TEXT_NORMAL: Color = Color::Rgb(215, 220, 230);
    pub const TEXT_DIM: Color = Color::Rgb(130, 140, 155);
}

/// Convenience helpers for common styles.
///
/// These are deliberately small, composable building blocks rather than
/// one-off "theme explosion".
pub fn block_title() -> Style {
    Style::default()
        .fg(palette::ACCENT_TEAL)
        .add_modifier(Modifier::BOLD)
}

pub fn header_title() -> Style {
    Style::default()
        .fg(palette::ACCENT_TEAL)
        .add_modifier(Modifier::BOLD)
}

pub fn footer_text() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

pub fn normal_text() -> Style {
    Style::default().fg(palette::TEXT_NORMAL)
}

pub fn dim_text() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

pub fn status_ok() -> Style {
    Style::default()
        .fg(palette::SUCCESS)
        .add_modifier(Modifier::BOLD)
}

pub fn status_warn() -> Style {
    Style::default()
        .fg(palette::WARNING)
        .add_modifier(Modifier::BOLD)
}

pub fn status_err() -> Style {
    Style::default()
        .fg(palette::ERROR)
        .add_modifier(Modifier::BOLD)
}

/// VM state glyph styles: small, bright indicators next to the name.
pub fn glyph_ok() -> Style {
    Style::default()
        .fg(palette::ACCENT_TEAL)
        .add_modifier(Modifier::BOLD)
}

pub fn glyph_warn() -> Style {
    Style::default()
        .fg(palette::WARNING)
        .add_modifier(Modifier::BOLD)
}

pub fn glyph_err() -> Style {
    Style::default()
        .fg(palette::ERROR)
        .add_modifier(Modifier::BOLD)
}

/// Event styles for the middle column.
pub fn event_info() -> Style {
    Style::default().fg(palette::TEXT_NORMAL)
}

pub fn event_warning() -> Style {
    Style::default().fg(palette::WARNING)
}

pub fn event_error() -> Style {
    Style::default().fg(palette::ERROR)
}

pub fn event_shell() -> Style {
    Style::default().fg(palette::ACCENT_PINK)
}

pub fn event_system() -> Style {
    Style::default().fg(palette::ACCENT_PURPLE)
}

/// Background style for the full-screen scrim behind the modal.
pub fn scrim_bg() -> Style {
    Style::default().bg(palette::BG).fg(palette::TEXT_DIM)
}

/// Background style for the modal itself.
pub fn modal_bg() -> Style {
    Style::default()
        .bg(palette::PANEL_BG)
        .fg(palette::TEXT_NORMAL)
}

/// ---------------------------------------------------------------------------
/// BRIGHTNESS HELPERS
/// ---------------------------------------------------------------------------
///
/// These helpers are used by the logo breathing engine and the new
/// A+C sparkline coherence system. They never modify palette constants
/// and are purely multiplicative brightness transforms.

/// Clamp helper: bound RGB channel to [0,255].
fn clamp_channel(v: f32) -> u8 {
    if v < 0.0 {
        0
    } else if v > 255.0 {
        255
    } else {
        v as u8
    }
}

/// Adjust brightness of an RGB color by a multiplicative factor.
/// Produces more aggressive shaping used by the main logo breathing.
///
/// This is exactly the same as in your canonical version — unchanged.
pub(crate) fn adjust_brightness(color: Color, factor: f32) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let fr = clamp_channel(r as f32 * factor);
            let fg = clamp_channel(g as f32 * factor);
            let fb = clamp_channel(b as f32 * factor);
            Color::Rgb(fr, fg, fb)
        }
        other => other,
    }
}

/// A softer brightness modulation used by the sparkline coherence path.
///
/// This avoids aggressive peaks and dips, keeping sparkline glyphs readable
/// while still subtly tying their amplitude to breathing + matrix phase.
///
/// - factor ~0.8..1.2 → compressed into ~0.92..1.08
/// - fully deterministic
/// - non-destructive
pub(crate) fn adjust_brightness_soft(color: Color, factor: f32) -> Color {
    // Compress the brightness range before applying:
    // This keeps the sparkline motion subtle and avoids hard jumps.
    let softened = 1.0 + ((factor - 1.0) * 0.35);

    match color {
        Color::Rgb(r, g, b) => {
            let fr = clamp_channel(r as f32 * softened);
            let fg = clamp_channel(g as f32 * softened);
            let fb = clamp_channel(b as f32 * softened);
            Color::Rgb(fr, fg, fb)
        }
        other => other,
    }
}
