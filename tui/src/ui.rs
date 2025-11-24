use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, AppEventKind, VmState};
use crate::logo;
use crate::theme;

/// Top-level draw entrypoint.
pub fn draw(f: &mut Frame, app: &App) {
    let size = f.size();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);

    draw_header(f, vertical[0]);
    draw_body(f, app, vertical[1]);
    draw_footer(f, app, vertical[2]);

    if app.vm_detail_open {
        draw_vm_detail_modal(f, app);
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        Span::styled("Chalybs ", theme::header_title()),
        Span::styled(
            "– Forged in Linux. Tempered in Rust. Honed on bare metal.",
            theme::dim_text(),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(theme::dim_text());

    let paragraph = Paragraph::new(title).block(block);

    f.render_widget(paragraph, area);
}

fn draw_body(f: &mut Frame, app: &App, area: Rect) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(44),
            Constraint::Percentage(30),
        ])
        .split(area);

    draw_status_panel(f, app, columns[0]);
    draw_events_panel(f, app, columns[1]);
    draw_shell_panel(f, app, columns[2]);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
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

fn draw_status_panel(f: &mut Frame, app: &App, area: Rect) {
    let splits = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(area);

    logo::draw_logo(f, splits[0]);
    draw_vm_status(f, app, splits[1]);
}

fn draw_vm_status(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(Span::styled("VMs", theme::block_title()))
        .borders(Borders::ALL)
        .border_style(theme::dim_text());

    if app.vms.is_empty() {
        f.render_widget(
            Paragraph::new("no VMs configured")
                .style(theme::dim_text())
                .block(block),
            area,
        );
        return;
    }

    // Explicit type to avoid E0283 (collect inference).
    let items: Vec<ListItem> = app
        .vms
        .iter()
        .enumerate()
        .map(|(idx, vm)| {
            let selected = idx == app.selected_vm;

            let marker = if selected { "▶" } else { " " };

            let (glyph, glyph_style) = match vm.state {
                VmState::Running => ("●", theme::glyph_ok()),
                VmState::Starting | VmState::ShuttingDown => ("●", theme::glyph_warn()),
                VmState::Stopped => ("●", theme::glyph_err()),
            };

            let state_style = match vm.state {
                VmState::Running => theme::status_ok(),
                VmState::Starting | VmState::ShuttingDown => theme::status_warn(),
                VmState::Stopped => theme::dim_text(),
            };

            let state_label = match vm.state {
                VmState::Running => "running",
                VmState::Starting => "starting",
                VmState::ShuttingDown => "shutting down",
                VmState::Stopped => "stopped",
            };

            ListItem::new(Line::from(vec![
                Span::styled(marker, theme::dim_text()),
                Span::raw(" "),
                Span::styled(glyph, glyph_style),
                Span::raw(" "),
                Span::styled(&vm.name, theme::normal_text()),
                Span::raw("  "),
                Span::styled(state_label, state_style),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);

    f.render_widget(list, area);
}

fn draw_events_panel(f: &mut Frame, app: &App, area: Rect) {
    let title_text = if app.events_scroll_locked {
        "Events [LOCKED]"
    } else {
        "Events"
    };

    let block = Block::default()
        .title(Span::styled(title_text, theme::block_title()))
        .borders(Borders::ALL)
        .border_style(theme::dim_text());

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

    let items: Vec<ListItem> = window
        .iter()
        .map(|evt| {
            let style = match evt.kind {
                AppEventKind::Info => theme::event_info(),
                AppEventKind::Warning => theme::event_warning(),
                AppEventKind::Error => theme::event_error(),
                AppEventKind::Shell => theme::event_shell(),
                AppEventKind::System => theme::event_system(),
            };

            ListItem::new(Line::from(Span::styled(&evt.message, style)))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(theme::event_shell());

    f.render_widget(list, area);
}

fn draw_shell_panel(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .title(Span::styled("Chalybs shell", theme::block_title()))
        .borders(Borders::ALL)
        .border_style(theme::dim_text());

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
fn draw_vm_detail_modal(f: &mut Frame, app: &App) {
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
        .border_style(theme::dim_text())
        .style(theme::modal_bg());

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });

    // Clear just the modal region to prevent any bleed.
    f.render_widget(Clear, area);
    f.render_widget(paragraph, area);
}
