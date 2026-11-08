//! Presentation. Draws only: no adapter, no command and no filesystem access.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};

use crate::app::{Screen, UiState};
use crate::domain::UNAVAILABLE;

const TITLE: &str = " OMARCHY CONTROL CENTER ";
const FOOTER: &str = " [↑↓] move  [i] inventory  [a] adapters  [?] help  [q] quit ";

fn muted(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::new().fg(Color::DarkGray))
}

fn semantic(color: Color, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color))
}

pub fn draw(frame: &mut Frame, state: &UiState) {
    let area = frame.area();
    let layout =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(1)])
            .split(area);

    draw_header(frame, layout[0], state);
    match state.screen {
        Screen::Inventory => draw_inventory(frame, layout[1], state),
        Screen::Adapters => draw_adapters(frame, layout[1], state),
        Screen::Help => draw_help(frame, layout[1], state),
    }
    draw_footer(frame, layout[2], state);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &UiState) {
    let available = state.snapshot.statuses.iter().filter(|status| status.available()).count();
    let total = state.snapshot.statuses.len();
    let loading = if state.loading { " · loading…" } else { "" };

    let mut spans = vec![
        Span::styled(
            format!("{} components", state.snapshot.components.len()),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        muted("   ·   "),
        Span::styled(
            format!("adapters {available}/{total} available"),
            Style::new().fg(if available == total { Color::Green } else { Color::Yellow }),
        ),
        muted(&format!("   ·   {}{loading}", state.system.display_name())),
    ];
    if state.read_only {
        spans.push(muted("   ·   "));
        spans.push(semantic(Color::Yellow, "READ-ONLY"));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(TITLE, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &UiState) {
    let text = if state.status_line.is_empty() {
        Line::from(muted(FOOTER))
    } else {
        Line::from(vec![semantic(Color::Yellow, &state.status_line), muted("   "), muted(FOOTER)])
    };
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_inventory(frame: &mut Frame, area: Rect, state: &UiState) {
    if state.snapshot.components.is_empty() {
        let message = if state.loading {
            "Loading the inventory…"
        } else {
            "No components were detected. Open the adapters view (a) to see why."
        };
        let block = Block::default().borders(Borders::ALL).title(" INSTALLED COMPONENTS ");
        frame.render_widget(Paragraph::new(message).block(block).wrap(Wrap { trim: true }), area);
        return;
    }

    let header = Row::new(vec!["NAME", "VERSION", "SOURCE", "STATUS"]).style(
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    );
    let rows = state.snapshot.components.iter().map(|component| {
        Row::new(vec![
            Cell::from(component.name.clone()),
            Cell::from(component.version.clone().unwrap_or_else(|| UNAVAILABLE.to_string())),
            Cell::from(component.source.label()),
            Cell::from(component.status.label()),
        ])
    });

    let table = Table::new(
        rows,
        [Constraint::Percentage(45), Constraint::Percentage(20), Constraint::Percentage(30), Constraint::Min(8)],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" INSTALLED COMPONENTS "))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut table_state = TableState::default();
    if !state.snapshot.components.is_empty() {
        table_state.select(Some(state.selected));
    }
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_adapters(frame: &mut Frame, area: Rect, state: &UiState) {
    let header = Row::new(vec!["ADAPTER", "STATE", "TOOL", "DETAIL"]).style(
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    );

    let rows = state.snapshot.statuses.iter().map(|status| {
        let (state_text, color) = match &status.availability {
            crate::domain::AdapterAvailability::Available => ("available", Color::Green),
            crate::domain::AdapterAvailability::Unavailable { .. } => ("unavailable", Color::Yellow),
        };
        let detail = match &status.availability {
            crate::domain::AdapterAvailability::Available => {
                status.notes.first().cloned().unwrap_or_default()
            }
            crate::domain::AdapterAvailability::Unavailable { reason } => reason.clone(),
        };
        Row::new(vec![
            Cell::from(status.display_name.clone()),
            Cell::from(Span::styled(state_text, Style::new().fg(color))),
            Cell::from(
                status.tool.as_ref().map(|path| path.display().to_string()).unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
            Cell::from(detail),
        ])
    });

    let table = Table::new(
        rows,
        [Constraint::Length(18), Constraint::Length(13), Constraint::Percentage(35), Constraint::Min(20)],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" ADAPTERS "));

    frame.render_widget(table, area);

    if !state.snapshot.failures.is_empty() {
        let height = (state.snapshot.failures.len() as u16 + 2).min(area.height.saturating_sub(2));
        let failures_area =
            Rect { x: area.x, y: area.y + area.height.saturating_sub(height), width: area.width, height };
        let lines: Vec<Line> = state
            .snapshot
            .failures
            .iter()
            .map(|failure| {
                Line::from(vec![
                    semantic(Color::Red, format!("{} ", failure.adapter)),
                    muted(&format!("{}: ", failure.stage)),
                    Span::raw(failure.message.clone()),
                ])
            })
            .collect();
        frame.render_widget(Clear, failures_area);
        frame.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" ADAPTER FAILURES ").border_style(Style::new().fg(Color::Red)))
                .wrap(Wrap { trim: true }),
            failures_area,
        );
    }
}

fn draw_help(frame: &mut Frame, area: Rect, _state: &UiState) {
    let lines = vec![
        Line::from(vec![semantic(Color::Cyan, "Navigation")]),
        Line::from("  ↑ / ↓, k / j   move through the list"),
        Line::from("  i              inventory"),
        Line::from("  a              adapter status"),
        Line::from("  ?              this help"),
        Line::from("  q, Q, Esc      quit"),
        Line::from(""),
        Line::from(vec![semantic(Color::Cyan, "States")]),
        Line::from("  available      the tool was found and can be queried"),
        Line::from("  unavailable    the tool is missing; the reason is shown, nothing else breaks"),
        Line::from(format!("  {UNAVAILABLE:<14} the source does not provide this value")),
    ];
    let block = Block::default().borders(Borders::ALL).title(" HELP ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}
