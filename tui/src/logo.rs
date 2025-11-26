use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::app::VisualEffects;
use crate::theme;

/// Internal helper: compute a gently "breathing" style for the rune glyph,
/// driven by `tick` and `effects.logo_reactive`.
fn logo_rune_style(tick: u64, effects: &VisualEffects) -> ratatui::style::Style {
    let base = theme::header_title().fg(crate::theme::palette::ACCENT_PINK);

    if !effects.logo_reactive {
        return base;
    }

    // Very gentle, slow pulse — no RGB unicorn puke.
    let phase = (tick / 8) % 16;

    match phase {
        // Slightly dimmed part of the cycle.
        0 | 1 => base.add_modifier(Modifier::DIM),

        // Neutral most of the time.
        2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 => base,

        // Brief brighter "focus" moments.
        10 | 11 | 12 => base.add_modifier(Modifier::BOLD),

        // Rare alternate hue to hint at a "charge" build-up.
        _ => base
            .fg(crate::theme::palette::ACCENT_PURPLE)
            .add_modifier(Modifier::BOLD),
    }
}

/// Render the current Chalybs logo representation.
///
/// This is intentionally kept simple: stylized text + rune that
/// mirrors the shield/slash logo without trying to reproduce the
/// full image in ASCII.
///
/// `tick` and `effects` are used for a subtle reactive "breathing"
/// effect on the rune glyph.
pub fn draw_logo(f: &mut Frame, area: Rect, tick: u64, effects: &VisualEffects) {
    let rune_style = logo_rune_style(tick, effects);

    let title = Line::from(vec![
        Span::styled("CHALYBS ", theme::header_title()),
        Span::styled("⟐", rune_style),
    ]);

    // A simple "runic slash C" mark under the title.
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

/// Logo renderer abstraction for future backends.
///
/// Right now the TUI calls `draw_logo` directly, but once the
/// kitty-graphics path is wired, this trait can be used to switch
/// between:
///
/// - ASCII / text-mode-only logo
/// - kitty image rendering (PNG derived from the official logo)
pub trait LogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect);
}

/// Default renderer used by the TUI today.
pub struct AsciiLogoRenderer;

impl LogoRenderer for AsciiLogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect) {
        // For generic renderers we don't have `tick` or `effects`,
        // so we call into the animated logo with a neutral baseline.
        let effects = VisualEffects::default_enabled();
        draw_logo(f, area, 0, &effects);
    }
}

#[cfg(feature = "kitty-graphics")]
/// Placeholder for a future kitty-graphics-powered renderer.
///
/// This intentionally does *not* pull in any extra crates yet; it just
/// describes the intended integration point. When ready, this will:
///
/// - Decode the official Chalybs logo PNG.
/// - Render it via kitty’s graphics protocol in terminals that support it.
/// - Fallback to `AsciiLogoRenderer` when not available.
pub struct KittyImageLogoRenderer;

#[cfg(feature = "kitty-graphics")]
impl LogoRenderer for KittyImageLogoRenderer {
    fn render(&self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::NONE);
        let text = Paragraph::new("kitty logo renderer (stub)").block(block);
        f.render_widget(text, area);
    }
}
