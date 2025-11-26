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
