//! Presentation. Draws only: no adapter, no command and no filesystem access.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};

use crate::app::{Focus, Screen, UiState};
use crate::domain::{AdapterAvailability, Component, UNAVAILABLE, format_size, shown};

const TITLE: &str = " OMARCHY CONTROL CENTER ";
const SIDEBAR_WIDTH: u16 = 24;
/// Below this width the sidebar is hidden instead of squeezing the list.
const SIDEBAR_MIN_WIDTH: u16 = 74;
/// Below this height the details pane is stacked away.
const DETAILS_MIN_HEIGHT: u16 = 22;

fn muted(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(Color::DarkGray))
}

fn accent(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(Color::Cyan))
}

fn semantic(color: Color, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color))
}

fn bold(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().add_modifier(Modifier::BOLD))
}

pub fn draw(frame: &mut Frame, state: &UiState) {
    let area = frame.area();
    let layout =
        Layout::vertical([Constraint::Length(3), Constraint::Min(3), Constraint::Length(2)])
            .split(area);

    draw_header(frame, layout[0], state);
    match state.screen {
        Screen::Inventory => draw_inventory(frame, layout[1], state),
        Screen::Details => draw_details_screen(frame, layout[1], state),
        Screen::Plan => draw_plan(frame, layout[1], state),
        Screen::Result => draw_result(frame, layout[1], state),
        Screen::History => draw_history(frame, layout[1], state),
        Screen::Adapters => draw_adapters(frame, layout[1], state),
        Screen::Help => draw_help(frame, layout[1]),
    }
    draw_footer(frame, layout[2], state);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &UiState) {
    let available = state.snapshot.statuses.iter().filter(|status| status.available()).count();
    let total = state.snapshot.statuses.len();
    let loading = if state.loading { " · loading…" } else { "" };
    let hidden = if state.group_filter.is_some() {
        format!(" · filter: {}", state.sidebar_label(state.sidebar_index).0)
    } else {
        String::new()
    };

    let mut spans = vec![
        Span::styled(
            format!("{} components", state.visible().len()),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        muted("   ·   "),
        Span::styled(
            format!("adapters {available}/{total}"),
            Style::new().fg(if available == total { Color::Green } else { Color::Yellow }),
        ),
        muted(&format!("   ·   {}{hidden}{loading}", state.system.display_name())),
        muted("   ·   "),
        search_span(state),
    ];
    if state.read_only {
        spans.push(muted("   ·   "));
        spans.push(semantic(Color::Yellow, "READ-ONLY"));
    }
    if state.snapshot.failure_count() > 0 {
        spans.push(muted("   ·   "));
        spans.push(semantic(Color::Red, format!("{} adapter issue(s)", state.snapshot.failure_count())));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::DarkGray))
        .title(Span::styled(TITLE, Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn search_span(state: &UiState) -> Span<'static> {
    if state.searching {
        return Span::styled(
            format!("search: {}▏", state.query),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        );
    }
    if state.query.trim().is_empty() {
        return muted("[/] search");
    }
    Span::styled(format!("search: {}", state.query), Style::new().fg(Color::Cyan))
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &UiState) {
    let keys = match (state.screen, state.searching) {
        (_, true) => " [type] filter  [Enter] keep  [Esc] clear ",
        (Screen::Inventory, false) => {
            " [↑↓] move  [Enter] details  [/] search  [d] remove  [H] history  [?] help  [q] quit "
        }
        (Screen::Details, false) => " [Esc] back  [a] adapters  [?] help  [q] quit ",
        (Screen::Plan, false) => " [type] the exact name  [Enter] confirm  [Esc] cancel ",
        (Screen::Result, false) => " [Enter] back to the inventory ",
        (Screen::History, false) => " [↑↓] browse  [Esc] back  [i] inventory ",
        (Screen::Adapters, false) => " [i] inventory  [Esc] back  [?] help  [q] quit ",
        _ => " [i] inventory  [Esc] back  [?] help  [q] quit ",
    };

    let status = if !state.status_line.is_empty() {
        semantic(Color::Yellow, state.status_line.clone())
    } else if let Some(notice) = state.notices.first() {
        semantic(Color::Yellow, notice.clone())
    } else {
        muted("")
    };

    frame.render_widget(Paragraph::new(Line::from(status)), Rect { height: 1, ..area });
    frame.render_widget(
        Paragraph::new(Line::from(muted(keys))),
        Rect { y: area.y + 1, height: 1, ..area },
    );
}

fn draw_inventory(frame: &mut Frame, area: Rect, state: &UiState) {
    let show_sidebar = area.width >= SIDEBAR_MIN_WIDTH;
    let columns = Layout::horizontal([
        show_sidebar.then_some(Constraint::Length(SIDEBAR_WIDTH)).unwrap_or(Constraint::Length(0)),
        Constraint::Min(20),
    ])
    .split(area);

    if show_sidebar {
        draw_sidebar(frame, columns[0], state);
    }
    let main = columns[1];

    let show_details = area.height >= DETAILS_MIN_HEIGHT;
    let rows = Layout::vertical([
        Constraint::Min(6),
        show_details.then_some(Constraint::Percentage(42)).unwrap_or(Constraint::Length(0)),
    ])
    .split(main);

    draw_list(frame, rows[0], state);
    if show_details {
        draw_details_pane(frame, rows[1], state);
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, state: &UiState) {
    let entries = state.sidebar_groups();
    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let (label, count) = state.sidebar_label(index);
            ListItem::new(Line::from(vec![
                Span::raw(label.to_string()),
                muted(format!(" {count}")),
            ]))
        })
        .collect();

    let border = if state.focus == Focus::Sidebar { Color::Cyan } else { Color::DarkGray };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title(" CATEGORIES "),
        )
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut list_state = ListState::default();
    list_state.select(Some(state.sidebar_index.min(entries.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_list(frame: &mut Frame, area: Rect, state: &UiState) {
    let visible = state.visible();
    let border = if state.focus == Focus::List { Color::Cyan } else { Color::DarkGray };
    let title = match state.group_filter {
        Some(group) => format!(" {} ", group.label().to_uppercase()),
        None => " INSTALLED COMPONENTS ".to_string(),
    };

    if visible.is_empty() {
        let message = if state.loading {
            "Loading the inventory…"
        } else if state.snapshot.components.is_empty() {
            "No components were detected. Open the adapters view (a) to see why."
        } else {
            "No component in this category."
        };
        frame.render_widget(
            Paragraph::new(message)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::new().fg(border))
                        .title(title),
                )
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let header = Row::new(vec!["NAME", "VERSION", "SOURCE", "STATE"]).style(
        Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
    );
    let rows = visible.iter().map(|component| {
        Row::new(vec![
            Cell::from(component.name.clone()),
            Cell::from(component.version.clone().unwrap_or_else(|| UNAVAILABLE.to_string())),
            Cell::from(component.source.label()),
            Cell::from(state_cell(component)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(46),
            Constraint::Percentage(22),
            Constraint::Percentage(28),
            Constraint::Min(8),
        ],
    )
    .header(header)
    .block(
        Block::default().borders(Borders::ALL).border_style(Style::new().fg(border)).title(title),
    )
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected.min(visible.len().saturating_sub(1))));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn state_cell(component: &Component) -> Span<'static> {
    match component.status {
        crate::domain::ComponentStatus::Orphan => semantic(Color::Yellow, "orphan"),
        crate::domain::ComponentStatus::Installed => muted("installed"),
        crate::domain::ComponentStatus::Unknown => muted("unknown"),
    }
}

fn capability_list(component: &Component) -> String {
    if component.capabilities.is_empty() {
        return UNAVAILABLE.to_string();
    }
    component
        .capabilities
        .iter()
        .map(|capability| capability.label())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn detail_lines(component: &Component) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(vec![bold(component.name.clone()), muted("  "), accent(shown(component.version.as_deref()).to_string())]),
        Line::from(vec![
            muted("Source      "),
            semantic(Color::Green, component.source.label()),
            muted("   Category  "),
            Span::raw(component.category.label().to_string()),
        ]),
        Line::from(vec![
            muted("State       "),
            state_cell(component),
            muted("   Confidence  "),
            Span::raw(component.confidence.label().to_string()),
        ]),
        Line::from(vec![
            muted("Size        "),
            Span::raw(
                component
                    .installed_size
                    .map(format_size)
                    .unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
            muted("   Installed  "),
            Span::raw(shown(component.installed_at.as_deref()).to_string()),
        ]),
        Line::from(vec![
            muted("Location    "),
            Span::raw(
                component
                    .location
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
        ]),
        Line::from(vec![
            muted("Capabilities  "),
            Span::raw(capability_list(component)),
        ]),
        Line::from(vec![
            muted("Depends on  "),
            Span::raw(summarise(&component.depends_on)),
        ]),
        Line::from(vec![
            muted("Required by "),
            Span::raw(summarise(&component.required_by)),
        ]),
        Line::from(vec![muted("Detected by "), muted(component.detection.label())]),
    ];

    if let Some(description) = &component.description {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::raw(description.clone())));
    }
    for field in &component.extra {
        lines.push(Line::from(vec![
            muted(format!("{:<13}", field.label)),
            Span::raw(field.value.clone()),
        ]));
    }
    lines
}

/// Shows the first few entries and says how many were left out, instead of
/// pretending the list was complete.
fn summarise(items: &[String]) -> String {
    if items.is_empty() {
        return "none".to_string();
    }
    const LIMIT: usize = 6;
    if items.len() <= LIMIT {
        return items.join(", ");
    }
    format!("{} … (+{} more)", items[..LIMIT].join(", "), items.len() - LIMIT)
}

fn draw_details_pane(frame: &mut Frame, area: Rect, state: &UiState) {
    let block = Block::default().borders(Borders::ALL).title(" SELECTED ");
    let Some(component) = state.selected_component() else {
        frame.render_widget(Paragraph::new("Nothing selected").block(block), area);
        return;
    };
    let lines = detail_lines(component);
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn draw_details_screen(frame: &mut Frame, area: Rect, state: &UiState) {
    let Some(component) = state.selected_component() else {
        frame.render_widget(
            Paragraph::new("Nothing selected").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };
    frame.render_widget(
        Paragraph::new(detail_lines(component))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::Cyan))
                    .title(" DETAILS "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_plan(frame: &mut Frame, area: Rect, state: &UiState) {
    let Some(pending) = &state.pending else {
        frame.render_widget(
            Paragraph::new("No operation is being prepared.").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };
    let plan = &pending.plan;
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            bold(plan.target_name.clone()),
            muted("  "),
            accent(shown(plan.target_version.as_deref()).to_string()),
            muted("   "),
            Span::raw(plan.adapter.clone()),
        ]),
        Line::from(vec![
            muted("Action      "),
            semantic(Color::Yellow, plan.kind.label().to_uppercase()),
            muted("   identity  "),
            Span::raw(plan.target.to_string()),
        ]),
        Line::from(""),
        Line::from(accent("WHAT WILL RUN")),
    ];

    if plan.commands.is_empty() {
        lines.push(Line::from(semantic(Color::Red, "the adapter proposed no command: nothing to run")));
    }
    for command in &plan.commands {
        let mut spans = vec![semantic(Color::Green, format!("  $ {}", command.display()))];
        if command.requires_root {
            spans.push(muted("  [root]"));
        }
        if command.requires_tty {
            spans.push(muted("  [terminal]"));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(vec![muted(format!("    {}", command.rationale))]));
    }

    if plan.requires_tty() {
        lines.push(Line::from(vec![muted(
            "  The terminal is handed to this command while it runs, so a privilege prompt \
             works normally. No password is ever read or stored by this program.",
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(accent("WHAT ELSE COULD BE AFFECTED")));
    if plan.affected.is_empty() {
        lines.push(Line::from(muted("  nothing reported beyond the target itself")));
    } else {
        for affected in &plan.affected {
            lines.push(Line::from(vec![semantic(Color::Yellow, "  "), Span::raw(affected.clone())]));
        }
    }

    if !plan.blocking.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(Color::Red, "BLOCKED — THIS OPERATION WILL NOT RUN")));
        for reason in &plan.blocking {
            lines.push(Line::from(vec![semantic(Color::Red, "  "), Span::raw(reason.clone())]));
        }
    }

    if !plan.unknown.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(Color::Yellow, "NOT DETERMINED")));
        for fact in &plan.unknown {
            lines.push(Line::from(vec![semantic(Color::Yellow, "  "), Span::raw(fact.clone())]));
        }
    }

    if !plan.notes.is_empty() {
        lines.push(Line::from(""));
        for note in &plan.notes {
            lines.push(Line::from(vec![muted("  "), muted(note.clone())]));
        }
    }

    lines.push(Line::from(""));
    if pending.is_blocked() {
        lines.push(Line::from(semantic(
            Color::Red,
            "Nothing can be confirmed while the plan is blocked. Press Esc.",
        )));
    } else {
        lines.push(Line::from(vec![
            bold("Type  "),
            accent(pending.confirmation_phrase().to_string()),
            bold("  to confirm: "),
            Span::styled(state.plan_input.clone(), Style::new().fg(Color::Yellow)),
            Span::styled("▏", Style::new().fg(Color::Cyan)),
        ]));
        let satisfied = pending.confirmation_satisfied(&state.plan_input);
        lines.push(Line::from(if satisfied {
            semantic(Color::Green, "Press Enter to execute exactly the command above.")
        } else {
            muted("Enter stays disabled until the name matches exactly.")
        }));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(Color::Yellow))
                    .title(" OPERATION PLAN "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_result(frame: &mut Frame, area: Rect, state: &UiState) {
    let Some(record) = &state.last_record else {
        frame.render_widget(
            Paragraph::new("No operation has run in this session.").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let banner = if record.success {
        semantic(Color::Green, format!("{} succeeded", record.kind))
    } else {
        semantic(Color::Red, format!("{} failed", record.kind))
    };

    let mut lines = vec![
        Line::from(banner),
        Line::from(""),
        Line::from(vec![muted("Component   "), bold(record.component_name.clone()), muted(format!("  ({})", record.component))]),
        Line::from(vec![
            muted("Exit code   "),
            Span::raw(
                record
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "terminated by a signal".to_string()),
            ),
        ]),
        Line::from(vec![muted("Started     "), Span::raw(record.started_at())]),
        Line::from(vec![muted("Command     "), Span::raw(record.command.clone())]),
        Line::from(vec![muted("Confirmed   "), Span::raw(record.confirmed_by.clone())]),
        Line::from(vec![muted("Summary     "), Span::raw(record.summary.clone())]),
    ];

    if !record.error_tail.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(Color::Red, "ERROR OUTPUT")));
        for line in record.error_tail.lines().take(12) {
            lines.push(Line::from(Span::raw(line.to_string())));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(muted(
        "The exit code and the error output are kept verbatim: this screen never reports a \
         failure as success.",
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(if record.success { Color::Green } else { Color::Red }))
                    .title(" OPERATION RESULT "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_history(frame: &mut Frame, area: Rect, state: &UiState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" OPERATION HISTORY ({}) ", state.history_entries.len()));

    if state.history_entries.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "No operation has been recorded yet.\n\nEvery executed operation is appended here \
                 with its timestamp, component, command, exit code and error summary. No password, \
                 token or configuration content is ever stored.",
            )
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    for (index, record) in state.history_entries.iter().enumerate() {
        let marker = if record.success {
            semantic(Color::Green, "ok  ")
        } else {
            semantic(Color::Red, "fail")
        };
        let style = if index == state.history_selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        items.push(
            ListItem::new(vec![
                Line::from(vec![
                    marker,
                    muted(format!(" {} ", record.started_at())),
                    bold(record.component_name.clone()),
                    muted(format!("  {} · {}", record.kind, record.adapter)),
                    muted(format!("  exit {:?}", record.exit_code)),
                ]),
                Line::from(vec![muted(format!("     $ {}", record.command))]),
            ])
            .style(style),
        );
    }

    let list = List::new(items).block(block).highlight_style(Style::new());
    let mut list_state = ListState::default();
    list_state.select(Some(state.history_selected.min(state.history_entries.len() - 1)));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_adapters(frame: &mut Frame, area: Rect, state: &UiState) {
    let header =
        Row::new(vec!["ADAPTER", "STATE", "TOOL", "DETAIL"]).style(
            Style::new().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
        );

    let rows = state.snapshot.statuses.iter().map(|status| {
        let (state_text, color) = match &status.availability {
            AdapterAvailability::Available => ("available", Color::Green),
            AdapterAvailability::Unavailable { .. } => ("unavailable", Color::Yellow),
        };
        let detail = match &status.availability {
            AdapterAvailability::Available => status.notes.join(" · "),
            AdapterAvailability::Unavailable { reason } => reason.clone(),
        };
        let tool = match (&status.tool, &status.tool_version) {
            (Some(path), Some(version)) => format!("{} ({version})", path.display()),
            (Some(path), None) => path.display().to_string(),
            _ => UNAVAILABLE.to_string(),
        };
        Row::new(vec![
            Cell::from(status.display_name.clone()),
            Cell::from(Span::styled(state_text, Style::new().fg(color))),
            Cell::from(tool),
            Cell::from(detail),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(12),
            Constraint::Percentage(36),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" ADAPTERS "));

    frame.render_widget(table, area);

    if state.snapshot.failure_count() > 0 {
        let lines: Vec<Line> = state
            .snapshot
            .failures
            .iter()
            .map(|failure| {
                Line::from(vec![
                    semantic(Color::Red, format!("{} ", failure.adapter)),
                    muted(format!("{}: ", failure.stage)),
                    Span::raw(failure.message.clone()),
                ])
            })
            .chain(state.snapshot.duplicates.iter().map(|duplicate| {
                Line::from(vec![
                    semantic(Color::Yellow, "duplicate identity "),
                    Span::raw(duplicate.id.to_string()),
                    muted(format!(" kept by {}, rejected from {}", duplicate.kept, duplicate.rejected)),
                ])
            }))
            .collect();

        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2)).max(3);
        let failure_area = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(height),
            width: area.width,
            height,
        };
        frame.render_widget(Clear, failure_area);
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::new().fg(Color::Red))
                        .title(" ADAPTER ISSUES "),
                )
                .wrap(Wrap { trim: true }),
            failure_area,
        );
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows = [
        ("Navigation", ""),
        ("  ↑ / ↓, k / j", "move inside the focused pane"),
        ("  Tab", "switch between the category sidebar and the list"),
        ("  ← / →, h / l", "same as Tab"),
        ("  Enter", "open the details of the selected component"),
        ("  Esc", "go back, then quit from the inventory"),
        ("  /", "search by name, identifier, description or source"),
        ("  d / u", "preview a removal or an update for the selected component"),
        ("  H", "operation history: what ran, when, and how it ended"),
        ("  i / a / ?", "inventory, adapter status, this help"),
        ("  q, Q", "quit (never interrupts a running operation)"),
        ("", ""),
        ("Reading the list", ""),
        ("  STATE", "installed or orphan; orphans are informative only"),
        ("  SOURCE", "the manager that owns the component: pacman, AUR/foreign, flatpak"),
        ("", ""),
        ("Honest facts", ""),
        ("  No disponible", "the source does not provide this value"),
        ("  unavailable", "the tool is not installed; the reason is shown and nothing else breaks"),
        ("  Detection", "the exact command that produced the fact"),
        ("", ""),
        ("Safety", ""),
        ("  read-only", "start with --read-only to hide every destructive action"),
        ("  confirmation", "destructive actions always show a plan and require the exact name"),
    ];

    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, description)| {
            if key.is_empty() {
                Line::from("")
            } else if description.is_empty() {
                Line::from(accent(*key))
            } else {
                Line::from(vec![bold(format!("{key:<20}")), Span::raw(*description)])
            }
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" HELP "))
            .wrap(Wrap { trim: false }),
        area,
    );
}
