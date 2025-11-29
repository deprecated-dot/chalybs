use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, AppEvent, AppEventKind, VisualEffects, VmState};
use crate::glitch::{
    affects_region, glitch_profile, junk_prefix, space_jitter, tick_for_region, GlitchMode,
    GlitchProfile, REGION_BORDERS, REGION_EVENTS, REGION_HEADER, REGION_SPARK, REGION_VMS,
};
use crate::logo;
use crate::theme;

// Thresholds for dynamic VM list layout.
//
// - >= VM_LAYOUT_WIDTH_FULL   : multi-line, full badge + sparkline
// - >= VM_LAYOUT_WIDTH_MEDIUM : two-line, partial badges
// - <  VM_LAYOUT_WIDTH_MEDIUM : compact name + state only
const VM_LAYOUT_WIDTH_FULL: u16 = 70;
const VM_LAYOUT_WIDTH_MEDIUM: u16 = 50;

/// Top-level draw entrypoint.
pub fn draw(f: &mut Frame, app: &App) {
    let size = f.size();

    // Single, deterministic glitch profile per frame.
    let glitch = glitch_profile(app.tick_count);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);

    draw_header(f, app, vertical[0], &glitch);
    draw_body(f, app, vertical[1], &glitch);
    draw_footer(f, app, vertical[2], &glitch);

    if app.vm_detail_open {
        draw_vm_detail_modal(f, app, &glitch);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    // Header line with optional synthetic "load" gauge driven by tick_count.
    let mut spans = Vec::new();

    // "Chalybs" stays as the stable anchor even during glitches.
    spans.push(Span::styled("Chalybs ", theme::header_title()));

    // Tagline may be color-inverted during header glitches.
    let tagline_style = maybe_invert_style(theme::dim_text(), glitch, REGION_HEADER);
    spans.push(Span::styled(
        "– Forged in Linux. Tempered in Rust. Honed on bare metal.",
        tagline_style,
    ));

    // Optional subtle load sparkline in the header when effects are enabled.
    if app.effects.load_index {
        spans.push(Span::raw("   "));
        spans.push(Span::styled("[load ", tagline_style));

        // For the sparkline we allow phase/ghost effects and color inversion.
        let spark_tick = tick_for_region(app.tick_count, glitch, REGION_SPARK);
        let breath = logo::logo_breath_factor(spark_tick);
        let header_spark = render_sparkline(spark_tick, 0, breath);

        // Color-coherent with the logo: softly modulated accent pink,
        // then optionally inverted under ColorInvert glitches.
        let spark_color = theme::adjust_brightness_soft(theme::palette::ACCENT_PINK, breath);
        let mut spark_style = Style::default().fg(spark_color);
        spark_style = maybe_invert_style(spark_style, glitch, REGION_SPARK);

        // Small horizontal jitter in SpatialJitter mode.
        let jitter = space_jitter(glitch, REGION_SPARK, 0);
        let spark_text = match jitter {
            -1 => format!("{header_spark} "),
            1 => format!(" {header_spark}"),
            _ => header_spark,
        };

        spans.push(Span::styled(spark_text, spark_style));
        spans.push(Span::styled("]", tagline_style));
    }

    let title = Line::from(spans);

    // Border may be inverted as part of header-region glitches.
    let border_style = maybe_invert_style(theme::dim_text(), glitch, REGION_HEADER);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(border_style);

    let paragraph = Paragraph::new(title).block(block);

    f.render_widget(paragraph, area);
}

fn draw_body(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(44),
            Constraint::Percentage(30),
        ])
        .split(area);

    draw_status_panel(f, app, columns[0], glitch);
    draw_events_panel(f, app, columns[1], glitch);
    draw_shell_panel(f, app, columns[2], glitch);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect, _glitch: &GlitchProfile) {
    let mut spans = Vec::new();

    spans.push(Span::styled("q", theme::header_title()));
    spans.push(Span::raw(" quit  "));

    spans.push(Span::styled("↑/↓", theme::header_title()));
    spans.push(Span::raw(" select VM  "));

    spans.push(Span::styled("Enter", theme::header_title()));
    spans.push(Span::raw(" send shell command  "));

    spans.push(Span::styled("F2", theme::header_title()));
    spans.push(Span::raw(" VM detail  "));

    spans.push(Span::styled("Ctrl-S", theme::header_title()));
    spans.push(Span::raw(" lock events  "));

    spans.push(Span::styled("Ctrl-Q", theme::header_title()));
    spans.push(Span::raw(" unlock  "));

    spans.push(Span::styled("PgUp/PgDn", theme::header_title()));
    spans.push(Span::raw(" scroll"));

    if app.events_scroll_locked {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("[locked]", theme::status_warn()));
    }

    let line = Line::from(spans);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(theme::dim_text());

    let paragraph = Paragraph::new(line).block(block).wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

fn draw_status_panel(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(area);

    // Animated logo: subtle rune breathing via tick + effects.
    // Logo itself remains the "true" signal; glitches distort the surrounding
    // panels, not the brand rune.
    logo::draw_logo(f, splits[0], app.tick_count, &app.effects);
    draw_vm_status(f, app, splits[1], glitch);
}

fn draw_vm_status(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    // Border shimmer is still driven by the base tick, but the glitch aura
    // can push intensity and optionally color-invert the border.
    let block = Block::default()
        .title(Span::styled("VMs", theme::block_title()))
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.tick_count, 1, &app.effects, glitch));

    if app.vms.is_empty() {
        f.render_widget(
            Paragraph::new("no VMs configured")
                .style(theme::dim_text())
                .block(block),
            area,
        );
        return;
    }

    // Inner area width determines how rich the VM rows can be.
    let inner = block.inner(area);
    let width = inner.width;
    if width == 0 {
        f.render_widget(block, area);
        return;
    }

    let tick = app.tick_count;
    let effects = &app.effects;

    let items: Vec<ListItem> = app
        .vms
        .iter()
        .enumerate()
        .map(|(idx, vm)| {
            build_vm_list_item(
                vm,
                idx,
                idx == app.selected_vm,
                width,
                tick,
                effects,
                glitch,
            )
        })
        .collect();

    let list = List::new(items).block(block);

    f.render_widget(list, area);
}

/// Build a single VM list item using dynamic layout rules and visual effects.
fn build_vm_list_item<'a>(
    vm: &'a crate::app::VmStatus,
    vm_index: usize,
    selected: bool,
    width: u16,
    tick: u64,
    effects: &VisualEffects,
    glitch: &GlitchProfile,
) -> ListItem<'a> {
    let marker = if selected { "▶" } else { " " };

    // Allow phase/ghost effects to de-phase the VM animation rhythm.
    let vm_tick = tick_for_region(tick, glitch, REGION_VMS);
    let spark_tick = tick_for_region(tick, glitch, REGION_SPARK);

    // Glyph + styles by VM state.
    let mut glyph_style = match vm.state {
        VmState::Running => theme::glyph_ok(),
        VmState::Starting | VmState::ShuttingDown => theme::glyph_warn(),
        VmState::Stopped => theme::glyph_err(),
    };

    // Pulse animation: small breathing effect for *all* VMs now, even stopped.
    let pulse_phase = (vm_tick / 4) % 4;
    let glyph_char = if effects.pulse {
        match pulse_phase {
            0 => "•",
            1 => "●",
            2 => "◉",
            _ => "●",
        }
    } else {
        "●"
    };

    if effects.pulse {
        // Slight extra emphasis at the peak of the pulse.
        if pulse_phase == 2 {
            glyph_style = glyph_style.add_modifier(Modifier::BOLD);
        }
    }

    let (state_label, state_style) = match vm.state {
        VmState::Running => ("RUNNING", theme::status_ok()),
        VmState::Starting => ("STARTING", theme::status_warn()),
        VmState::ShuttingDown => ("SHUTDOWN", theme::status_warn()),
        VmState::Stopped => ("STOPPED", theme::dim_text()),
    };

    let state_badge = Span::styled(format!("[{state_label}]"), state_style);

    // Isolation short code for compact badges: DIS/AUD/ENF/other.
    let iso_code = match vm.isolation_mode.as_str() {
        "disabled" => "DIS",
        "audit" => "AUD",
        "enforce" => "ENF",
        other => other,
    };
    let iso_badge = Span::styled(format!("[ISO: {iso_code}]"), theme::dim_text());

    let tasmota_badge = if vm.tasmota_on {
        Span::styled("[TAS: ON]", theme::status_ok())
    } else {
        Span::styled("[TAS: OFF]", theme::dim_text())
    };

    let cpu_badge = if vm.cpu_pinned {
        Span::styled("[CPU]", theme::status_ok())
    } else {
        Span::styled("[CPU]", theme::dim_text())
    };

    let irq_badge = if vm.irq_pinned {
        Span::styled("[IRQ]", theme::status_ok())
    } else {
        Span::styled("[IRQ]", theme::status_err())
    };

    // Base name line prefix used in all layouts.
    let mut name_prefix = vec![
        Span::styled(marker, theme::dim_text()),
        Span::raw(" "),
        Span::styled(glyph_char, glyph_style),
        Span::raw(" "),
        Span::styled(&vm.name, theme::normal_text()),
    ];

    // Spatial jitter in the VM list name prefix (very subtle).
    let jitter = space_jitter(glitch, REGION_VMS, vm_index as u16);
    if jitter < 0 {
        name_prefix.insert(0, Span::raw(" "));
    } else if jitter > 0 {
        name_prefix.push(Span::raw(" "));
    }

    // Layout selection based on available width.
    if width >= VM_LAYOUT_WIDTH_FULL {
        // Wide: rich layout.
        //
        // Line 1: marker, glyph, name, [STATE], badges (if enabled)
        // Line 2: optional sparkline "load" gauge (if enabled)
        let mut line1 = name_prefix;
        line1.push(Span::raw("  "));
        line1.push(state_badge);

        if effects.badges {
            line1.push(Span::raw("  "));
            line1.push(iso_badge);
            line1.push(Span::raw("  "));
            line1.push(tasmota_badge);
            line1.push(Span::raw("  "));
            line1.push(cpu_badge);
            line1.push(Span::raw(" "));
            line1.push(irq_badge);
        }

        if effects.load_index {
            // VM load sparkline tied to logo breathing + coherence, plus
            // potential phase/ghost distortion via spark_tick.
            let breath = logo::logo_breath_factor(spark_tick);
            let spark = render_sparkline(spark_tick, vm_index, breath);

            let spark_color = theme::adjust_brightness_soft(theme::palette::ACCENT_PINK, breath);
            let mut spark_style = Style::default().fg(spark_color);
            spark_style = maybe_invert_style(spark_style, glitch, REGION_SPARK);

            let line2 = Line::from(vec![
                Span::raw("    "),
                Span::styled("load ", theme::dim_text()),
                Span::styled(spark, spark_style),
            ]);

            ListItem::new(vec![Line::from(line1), line2])
        } else {
            ListItem::new(Line::from(line1))
        }
    } else if width >= VM_LAYOUT_WIDTH_MEDIUM {
        // Medium: two lines when badges are enabled.
        //
        // Line 1: marker, glyph, name, [STATE]
        // Line 2: indented badges for isolation + TAS.
        let mut line1 = name_prefix;
        line1.push(Span::raw("  "));
        line1.push(state_badge);

        if effects.badges {
            let line2 = Line::from(vec![
                Span::raw("    "), // indent under the name
                iso_badge,
                Span::raw("  "),
                tasmota_badge,
            ]);

            ListItem::new(vec![Line::from(line1), line2])
        } else {
            // Badges disabled: single-line layout.
            ListItem::new(vec![Line::from(line1)])
        }
    } else {
        // Narrow: compact layout, name + state only.
        let mut spans = name_prefix;
        spans.push(Span::raw("  "));
        spans.push(state_badge);

        ListItem::new(Line::from(spans))
    }
}

fn draw_events_panel(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    let title_text = if app.events_scroll_locked {
        "Events [LOCKED]"
    } else {
        "Events"
    };

    let block = Block::default()
        .title(Span::styled(title_text, theme::block_title()))
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.tick_count, 2, &app.effects, glitch));

    if app.events.is_empty() {
        f.render_widget(
            Paragraph::new("event stream is empty")
                .style(theme::dim_text())
                .block(block),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    let height = inner.height as usize;
    if height == 0 {
        f.render_widget(block, area);
        return;
    }

    let total = app.events.len();
    let view_len = total.min(height);
    let max_offset = total.saturating_sub(view_len);

    let offset = if app.events_scroll_locked {
        app.events_scroll_offset.min(max_offset)
    } else {
        0
    };

    let end = total.saturating_sub(offset);
    let start = end.saturating_sub(view_len);
    let window = &app.events[start..end];

    let tick = app.tick_count;
    let effects = &app.effects;

    let items: Vec<ListItem> = window
        .iter()
        .enumerate()
        .map(|(row_idx, evt)| build_event_list_item(evt, row_idx, tick, effects, glitch))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme::event_shell());

    f.render_widget(list, area);
}

fn draw_shell_panel(f: &mut Frame, app: &App, area: Rect, glitch: &GlitchProfile) {
    let block = Block::default()
        .title(Span::styled("Chalybs shell", theme::block_title()))
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.tick_count, 3, &app.effects, glitch));

    let inner = block.inner(area);

    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let history_lines: Vec<Line> = app
        .shell_history
        .iter()
        .rev()
        .take(8)
        .rev()
        .map(|cmd| {
            Line::from(vec![
                Span::styled("chalybs> ", theme::event_shell()),
                Span::styled(cmd, theme::normal_text()),
            ])
        })
        .collect();

    f.render_widget(
        Paragraph::new(history_lines).wrap(Wrap { trim: true }),
        splits[0],
    );

    let prompt = "chalybs> ";
    let input_line = Line::from(vec![
        Span::styled(prompt, theme::event_shell()),
        Span::styled(&app.shell_input, theme::normal_text()),
    ]);

    f.render_widget(Paragraph::new(input_line), splits[1]);

    f.render_widget(block, area);
}

/// Centered rectangle helper.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);

    horizontal[1]
}

/// Shaded modal with filled background (Option C).
fn draw_vm_detail_modal(f: &mut Frame, app: &App, glitch: &GlitchProfile) {
    let size = f.size();

    // Scrim over the whole UI: subtle tinted background.
    let scrim = Block::default().style(theme::scrim_bg());
    f.render_widget(scrim, size);

    // Centered modal region.
    let area = centered_rect(60, 60, size);

    let mut lines: Vec<Line> = Vec::new();

    if let Some(vm) = app.selected_vm() {
        let state_label = match vm.state {
            VmState::Running => "running",
            VmState::Starting => "starting",
            VmState::ShuttingDown => "shutting down",
            VmState::Stopped => "stopped",
        };

        lines.push(Line::from(Span::styled(&vm.name, theme::header_title())));
        lines.push(Line::from(""));

        lines.push(Line::from(vec![
            Span::styled("state: ", theme::dim_text()),
            Span::styled(state_label, theme::normal_text()),
        ]));

        lines.push(Line::from(vec![
            Span::styled("CPU pinned: ", theme::dim_text()),
            Span::styled(
                if vm.cpu_pinned { "yes" } else { "no" },
                theme::normal_text(),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("IRQ pinned: ", theme::dim_text()),
            Span::styled(
                if vm.irq_pinned { "yes" } else { "no" },
                theme::normal_text(),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Tasmota: ", theme::dim_text()),
            Span::styled(
                if vm.tasmota_on { "on" } else { "off" },
                theme::normal_text(),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Isolation: ", theme::dim_text()),
            Span::styled(&vm.isolation_mode, theme::normal_text()),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Hugepages: ", theme::dim_text()),
            Span::styled(
                if vm.hugepages { "enabled" } else { "disabled" },
                theme::normal_text(),
            ),
        ]));

        lines.push(Line::from(""));
    } else {
        lines.push(Line::from(Span::styled(
            "no VM selected",
            theme::normal_text(),
        )));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(Span::styled(
        "Press F2 or Esc to close",
        theme::dim_text(),
    )));

    // Modal block with filled background + shaded border.
    let block = Block::default()
        .title(Span::styled("VM detail", theme::block_title()))
        .borders(Borders::ALL)
        .border_style(panel_border_style(app.tick_count, 4, &app.effects, glitch))
        .style(theme::modal_bg());

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });

    // Clear just the modal region to prevent any bleed.
    f.render_widget(Clear, area);
    f.render_widget(paragraph, area);
}

/// Build a single event list item with scanline + matrix-style effects
/// plus optional glitch injection (color inversion, junk, jitter).
fn build_event_list_item(
    evt: &AppEvent,
    row_index: usize,
    tick: u64,
    effects: &VisualEffects,
    glitch: &GlitchProfile,
) -> ListItem<'static> {
    let base_style = match evt.kind {
        AppEventKind::Info => theme::event_info(),
        AppEventKind::Warning => theme::event_warning(),
        AppEventKind::Error => theme::event_error(),
        AppEventKind::Shell => theme::event_shell(),
        AppEventKind::System => theme::event_system(),
    };

    // Color inversion for events in ColorInvert mode.
    let base_style = maybe_invert_style(base_style, glitch, REGION_EVENTS);

    // Allow phase/ghost jitter of scanlines + matrix via adjusted tick.
    let event_tick = tick_for_region(tick, glitch, REGION_EVENTS);

    let style = apply_scanline_style(base_style, row_index, event_tick, effects);

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Optional pseudo-ANSI junk prefix during JunkInjection bursts.
    if let Some(prefix) = junk_prefix(row_index, tick, glitch) {
        spans.push(Span::styled(prefix, theme::event_system()));
        spans.push(Span::raw(" "));
    }

    if effects.matrix {
        // Gibson-ish, low-intensity "data rain": a short noisy column of
        // dim dots that drifts over time, plus rare glitchy glyphs.
        //
        // Loosely aligned with the logo breathing curve so that
        // heavier breaths produce slightly more active "rain".
        let breath = logo_breath_factor_for_events(event_tick);

        let mut noise = String::new();
        for col in 0..6 {
            let mut raw = (event_tick / 2)
                .wrapping_add(row_index as u64 * 7)
                .wrapping_add(col as u64 * 11);

            // During stronger breaths, advance the pattern slightly to
            // give the impression of increased drift velocity.
            let drift = ((breath - 1.0) * 16.0).round() as i64;
            if drift > 0 {
                raw = raw.wrapping_add((drift as u64).wrapping_mul(13));
            }

            let phase = (raw & 0xF) as u8;
            let mut glitch_roll = ((raw >> 4) & 0x1F) as u8;

            // At higher breath, slightly increase glitch probability,
            // still keeping glitches rare and subtle.
            if breath > 1.05 && glitch_roll > 0 {
                glitch_roll = glitch_roll.saturating_sub(1);
            }

            let ch = if glitch_roll == 0 {
                // Rare "glitch" characters.
                match phase % 5 {
                    0 => '░',
                    1 => '▒',
                    2 => '▓',
                    3 => '▌',
                    _ => '▐',
                }
            } else {
                // Normal speckle.
                match phase {
                    0 | 8 => '·',
                    1 | 9 => '˙',
                    2 | 10 => '•',
                    _ => ' ',
                }
            };

            noise.push(ch);
        }

        // Spatial jitter for the noise column.
        let jitter = space_jitter(glitch, REGION_EVENTS, row_index as u16);
        let noise = match jitter {
            -1 => format!("{noise} "),
            1 => format!(" {noise}"),
            _ => noise,
        };

        spans.push(Span::styled(noise, theme::dim_text()));
        spans.push(Span::raw(" "));
    }

    // Clone message into an owned span so we can use 'static here.
    spans.push(Span::styled(evt.message.clone(), style));

    ListItem::new(Line::from(spans))
}

/// Apply a subtle scanline-style shading to events.
fn apply_scanline_style(
    base: Style,
    row_index: usize,
    tick: u64,
    effects: &VisualEffects,
) -> Style {
    if !effects.scanlines {
        return base;
    }

    // Very light banding that drifts slowly over time.
    let band = ((row_index as u64 + tick / 8) % 4) as u8;
    let mut style = base;

    match band {
        0 => {
            style = style.bg(crate::theme::palette::BG);
        }
        1 => {
            style = style.bg(crate::theme::palette::PANEL_BG);
        }
        2 => {
            style = style
                .bg(crate::theme::palette::BG)
                .add_modifier(Modifier::DIM);
        }
        _ => {
            // Leave as-is for one band to avoid over-stylizing.
        }
    }

    style
}

/// Highly stochastic EMI-style shimmer for panel borders.
///
/// Features:
/// - Per-panel salted randomness (panels do not shimmer in sync)
/// - Rare, long-tail brighter bursts with random duration
/// - Two-frequency interference: slow LF wobble + fast HF flicker
/// - Constant ultra-low-amplitude "hiss" for a non-dead static look
/// - All deterministic: derived from tick + salt, no global RNG
///
/// Additionally, we let the logo breathing curve act as a gentle "aura"
/// that nudges the border intensity up or down, and allow ColorInvert
/// glitches to flip the border polarity.
fn panel_border_style(
    tick: u64,
    salt: u64,
    effects: &VisualEffects,
    glitch: &GlitchProfile,
) -> Style {
    if !effects.border_noise {
        // Even with border noise disabled, allow border color inversion
        // when a glitch specifically targets borders.
        let base = theme::dim_text();
        return maybe_invert_style(base, glitch, REGION_BORDERS);
    }

    // Deterministic "noise" seed.
    let seed = tick
        .wrapping_shl(7)
        .wrapping_add(salt.wrapping_mul(0x9E37_79B1_85EB_CA87))
        ^ tick.rotate_left(11);

    // --- Base hiss: tiny, almost imperceptible movement ---
    let hiss_bit = ((seed >> 19) & 0x1) != 0;
    let mut base = if hiss_bit {
        theme::dim_text().add_modifier(Modifier::DIM)
    } else {
        theme::dim_text()
    };

    // --- Long-tail bursts (rare, short-lived brighter "ticks") ---
    let burst_roll = (seed >> 8) & 0xFF;
    let burst_active = burst_roll < 2 && (tick.wrapping_add(salt * 17) % 97) < 7;

    // --- Two-frequency interference (LF + HF) controlling amplitude ---
    let lf_phase = ((tick / 23).wrapping_add(salt * 3)) % 7; // slow drift
    let hf_phase = ((tick / 3).wrapping_add(seed >> 5)) % 5; // quicker flicker

    let mut strength: u8 = 0;

    // LF wobble: gentle, slow movement.
    match lf_phase {
        2 | 4 => strength += 1,
        5 => strength += 2,
        _ => {}
    }

    // HF noise: occasional extra twitch.
    if hf_phase == 1 || hf_phase == 3 {
        strength += 1;
    }

    // Bursts: very rare extra boost.
    if burst_active {
        strength += 2;
    }

    // Logo "aura": let the rune breath subtly influence border strength.
    let breath = logo::logo_breath_factor(tick);
    if breath > 1.05 && strength < 3 {
        strength += 1;
    } else if breath < 0.95 && strength > 0 {
        strength -= 1;
    }

    // Clamp to a small range so we almost always stay subtle.
    if strength > 3 {
        strength = 3;
    }

    // Map amplitude to final style:
    //
    // 0 -> base dim_text / dim_text+DIM
    // 1 -> slightly lifted (dim text, maybe extra DIM)
    // 2 -> normal_text with DIM (barely brighter)
    // 3 -> normal_text BOLD (rare "NO VACANCY" spike)
    let style = match strength {
        0 => base,
        1 => base,
        2 => theme::normal_text().add_modifier(Modifier::DIM),
        _ => theme::normal_text().add_modifier(Modifier::BOLD),
    };

    // Final layer: optional color inversion when glitch affects borders.
    maybe_invert_style(style, glitch, REGION_BORDERS)
}

/// Render a synthetic single-row sparkline used for load index visualization.
///
/// This is deterministic and driven purely by `tick` + `vm_index` for now,
/// but its amplitude is shaped by the shared logo breathing curve so that
/// header + per-VM sparklines feel coherently "alive".
///
/// Option A:
///   - Advance on *every* tick (no more `tick / 2`), so it updates at the
///     TUI tick rate and no longer feels sluggish.
///
/// Option C:
///   - Amplitude driven by `logo_breath_factor`.
///   - Additional subtle shaping via `logo_breath_coherence` per VM.
fn render_sparkline(tick: u64, vm_index: usize, breath: f32) -> String {
    const SPARK: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    let mut out = String::new();
    let len = SPARK.len() as i32;
    let mid = (len - 1) / 2;

    // Map breath ~0.88..1.12 to [0.0 .. 1.0] as an amplitude control.
    let mut amp = (breath - 0.9) / 0.25;
    if amp < 0.0 {
        amp = 0.0;
    } else if amp > 1.0 {
        amp = 1.0;
    }

    // Per-VM coherence factor in 0..1, derived from the same global rhythm.
    let coh = logo::logo_breath_coherence(tick, vm_index as u64 + 1);

    // Blend amplitude with coherence to smooth the motion a bit and keep
    // different VMs from marching in perfect phase.
    let blended_amp = (amp * 0.7 + coh * 0.3).clamp(0.0, 1.0);

    // Fixed-width sparkline of 10 samples for now.
    for lane in 0..10 {
        // Faster clock: advance on *every* tick now (no `/ 2`).
        let base_phase = tick
            .wrapping_add(vm_index as u64 * 13)
            .wrapping_add(lane as u64 * 3)
            % (SPARK.len() as u64);

        let base_idx = base_phase as i32;
        let offset = base_idx - mid;

        // As the blended amplitude increases, preserve more of the offset
        // (larger peaks, richer shape).
        let scaled_offset = (offset as f32 * (0.3 + 0.7 * blended_amp)).round() as i32;
        let idx = (mid + scaled_offset).max(0).min(len - 1);

        out.push(SPARK[idx as usize]);
    }

    out
}

/// Helper: derive a slightly softened breathing factor for events.
///
/// We keep the same underlying global rhythm but give ourselves a hook
/// in case we ever want the events rail to feel a little "slower" or
/// "heavier" than the logo itself.
fn logo_breath_factor_for_events(tick: u64) -> f32 {
    logo::logo_breath_factor(tick)
}

/// Helper: conditional color inversion for a style in a given region.
///
/// This is a purely visual, non-destructive effect used when
/// `GlitchMode::ColorInvert` is active and the profile says the
/// given region is affected.
///
/// Inversion here is approximate, not mathematically perfect:
/// we flip to a light background with a dark-ish foreground that
/// still stays within the Chalybs palette.
fn maybe_invert_style(base: Style, glitch: &GlitchProfile, region_mask: u8) -> Style {
    if !glitch.active {
        return base;
    }

    if !matches!(glitch.mode, GlitchMode::ColorInvert) {
        return base;
    }

    if !affects_region(glitch, region_mask) {
        return base;
    }

    base.bg(theme::palette::TEXT_NORMAL).fg(theme::palette::BG)
}
