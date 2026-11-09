//! Presentation. Draws only: no adapter, no command and no filesystem access.

pub mod theme;

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

fn muted(palette: &theme::Palette, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(palette.muted))
}

fn accent(palette: &theme::Palette, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(palette.accent))
}

fn semantic(color: Color, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().fg(color))
}

fn bold(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::new().add_modifier(Modifier::BOLD))
}

pub fn draw(frame: &mut Frame, state: &UiState) {
    let area = frame.area();
    // A theme that defines a background fills the whole frame; the default theme paints
    // nothing, so the terminal keeps its own colours.
    if state.theme.paints_background() {
        frame.render_widget(Block::default().style(state.theme.base_style()), area);
    }
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);

    draw_header(frame, layout[0], state);
    match state.screen {
        // A confirmation is a floating window over the screen it came from, so the list
        // the user was reading stays visible behind it.
        Screen::CleanupConfirm => {
            draw_screen(frame, layout[1], state.previous_screen, state);
            draw_cleanup_confirm(frame, layout[1], state);
        }
        Screen::CleanupResult => {
            draw_screen(frame, layout[1], Screen::Inventory, state);
            draw_cleanup_result(frame, layout[1], state);
        }
        other => draw_screen(frame, layout[1], other, state),
    }
    draw_footer(frame, layout[2], state);
}

fn draw_screen(frame: &mut Frame, area: Rect, screen: Screen, state: &UiState) {
    let p = &state.theme.palette;
    match screen {
        Screen::Inventory => draw_inventory(frame, area, state),
        Screen::Details => draw_details_screen(frame, area, state),
        Screen::Plan => draw_plan(frame, area, state),
        Screen::Cleanup => draw_cleanup(frame, area, state),
        Screen::Quarantine => draw_quarantine(frame, area, state),
        Screen::Themes => draw_themes(frame, area, state),
        Screen::Result => draw_result(frame, area, state),
        Screen::History => draw_history(frame, area, state),
        Screen::Adapters => draw_adapters(frame, area, state),
        Screen::Help => draw_help(frame, area, p),
        // The two modal screens are drawn on top of a background; if one of them ever
        // reaches this point, show the inventory rather than nothing.
        Screen::CleanupConfirm | Screen::CleanupResult => draw_inventory(frame, area, state),
    }
}

/// A centred floating window: cleared area, then a bordered block with the content.
fn modal_area(area: Rect, content_lines: usize) -> Rect {
    let width = area.width.saturating_sub(6).clamp(24, 80);
    let height = (content_lines as u16 + 4).clamp(5, area.height.max(5));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let available = state
        .snapshot
        .statuses
        .iter()
        .filter(|status| status.available())
        .count();
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
            Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
        ),
        muted(p, "   ·   "),
        Span::styled(
            format!("adapters {available}/{total}"),
            Style::new().fg(if available == total {
                p.success
            } else {
                p.warning
            }),
        ),
        muted(
            p,
            format!("   ·   {}{hidden}{loading}", state.system.display_name()),
        ),
        muted(p, "   ·   "),
        search_span(state),
    ];
    if let Some(hint) = &state.update_hint {
        spans.push(muted(p, "   ·   "));
        spans.push(semantic(p.warning, format!("↑ {hint}")));
    }
    if state.read_only {
        spans.push(muted(p, "   ·   "));
        spans.push(semantic(p.warning, "READ-ONLY"));
    }
    if state.snapshot.failure_count() > 0 {
        spans.push(muted(p, "   ·   "));
        spans.push(semantic(
            p.error,
            format!("{} adapter issue(s)", state.snapshot.failure_count()),
        ));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(p.muted))
        .title(Span::styled(
            TITLE,
            Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn search_span(state: &UiState) -> Span<'static> {
    let p = &state.theme.palette;
    if state.searching {
        return Span::styled(
            format!("search: {}▏", state.query),
            Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
        );
    }
    if state.query.trim().is_empty() {
        return muted(p, "[/] search");
    }
    Span::styled(
        format!("search: {}", state.query),
        Style::new().fg(p.accent),
    )
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let keys = match (state.screen, state.searching) {
        (_, true) => " [type] filter  [Enter] keep  [Esc] clear ",
        (Screen::Inventory, false) => {
            " [↑↓] move [Enter] details [/] search [d] remove [t] theme [C] cleanup [V] quarantine [?] help "
        }
        (Screen::Details, false) => " [Esc] back  [a] adapters  [?] help  [q] quit ",
        (Screen::Plan, false) => " [type] the exact name  [Enter] confirm  [Esc] cancel ",
        (Screen::Result, false) => " [Enter] back to the inventory ",
        (Screen::Cleanup, false) => {
            " [space] select  [A] all  [N] none  [Enter] move to quarantine  [Esc] back "
        }
        (Screen::CleanupConfirm, false) => " [type] the phrase  [Enter] confirm  [Esc] cancel ",
        (Screen::CleanupResult, false) => " [Enter] close ",
        (Screen::Quarantine, false) => " [↑↓] browse  [r] restore  [p] purge for good  [Esc] back ",
        (Screen::History, false) => " [↑↓] browse  [Esc] back  [i] inventory ",
        (Screen::Adapters, false) => " [i] inventory  [Esc] back  [?] help  [q] quit ",
        _ => " [i] inventory  [Esc] back  [?] help  [q] quit ",
    };

    let status = if !state.status_line.is_empty() {
        semantic(p.warning, state.status_line.clone())
    } else if let Some(notice) = state.notices.first() {
        semantic(p.warning, notice.clone())
    } else {
        muted(p, "")
    };

    frame.render_widget(
        Paragraph::new(Line::from(status)),
        Rect { height: 1, ..area },
    );
    frame.render_widget(
        Paragraph::new(Line::from(muted(p, keys))),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

fn draw_inventory(frame: &mut Frame, area: Rect, state: &UiState) {
    let show_sidebar = area.width >= SIDEBAR_MIN_WIDTH;
    let columns = Layout::horizontal([
        if show_sidebar {
            Constraint::Length(SIDEBAR_WIDTH)
        } else {
            Constraint::Length(0)
        },
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
        if show_details {
            Constraint::Percentage(42)
        } else {
            Constraint::Length(0)
        },
    ])
    .split(main);

    draw_list(frame, rows[0], state);
    if show_details {
        draw_details_pane(frame, rows[1], state);
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let entries = state.sidebar_groups();
    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let (label, count) = state.sidebar_label(index);
            ListItem::new(Line::from(vec![
                Span::raw(label.to_string()),
                muted(p, format!(" {count}")),
            ]))
        })
        .collect();

    let border = if state.focus == Focus::Sidebar {
        p.accent
    } else {
        p.muted
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(border))
                .title(" CATEGORIES "),
        )
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut list_state = ListState::default();
    list_state.select(Some(
        state.sidebar_index.min(entries.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_list(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let visible = state.visible();
    let border = if state.focus == Focus::List {
        p.accent
    } else {
        p.muted
    };
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

    let header = Row::new(vec!["NAME", "VERSION", "SOURCE", "STATE"])
        .style(Style::new().fg(p.muted).add_modifier(Modifier::BOLD));
    let rows = visible.iter().map(|component| {
        Row::new(vec![
            Cell::from(component.name.clone()),
            Cell::from(
                component
                    .version
                    .clone()
                    .unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
            Cell::from(component.source.label()),
            Cell::from(state_cell(p, component)),
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
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(border))
            .title(title),
    )
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected.min(visible.len().saturating_sub(1))));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn state_cell(palette: &theme::Palette, component: &Component) -> Span<'static> {
    let p = palette;
    match component.status {
        crate::domain::ComponentStatus::Enabled => semantic(p.success, "enabled"),
        crate::domain::ComponentStatus::Disabled => muted(p, "disabled"),
        crate::domain::ComponentStatus::Active => semantic(p.success, "active"),
        crate::domain::ComponentStatus::Inactive => muted(p, "inactive"),
        crate::domain::ComponentStatus::Missing => semantic(p.warning, "missing"),
        crate::domain::ComponentStatus::Orphan => semantic(p.warning, "orphan"),
        crate::domain::ComponentStatus::Installed => muted(p, "installed"),
        crate::domain::ComponentStatus::Unknown => muted(p, "unknown"),
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

fn detail_lines(palette: &theme::Palette, component: &Component) -> Vec<Line<'static>> {
    let p = palette;
    let mut lines = vec![
        Line::from(vec![
            bold(component.name.clone()),
            muted(p, "  "),
            accent(p, shown(component.version.as_deref()).to_string()),
        ]),
        Line::from(vec![
            muted(p, "Source      "),
            semantic(p.success, component.source.label()),
            muted(p, "   Category  "),
            Span::raw(component.category.label().to_string()),
        ]),
        Line::from(vec![
            muted(p, "State       "),
            state_cell(p, component),
            muted(p, "   Confidence  "),
            Span::raw(component.confidence.label().to_string()),
        ]),
        Line::from(vec![
            muted(p, "Size        "),
            Span::raw(
                component
                    .installed_size
                    .map(format_size)
                    .unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
            muted(p, "   Installed  "),
            Span::raw(shown(component.installed_at.as_deref()).to_string()),
        ]),
        Line::from(vec![
            muted(p, "Location    "),
            Span::raw(
                component
                    .location
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| UNAVAILABLE.to_string()),
            ),
        ]),
        Line::from(vec![
            muted(p, "Capabilities  "),
            Span::raw(capability_list(component)),
        ]),
        Line::from(vec![
            muted(p, "Depends on  "),
            Span::raw(summarise(&component.depends_on)),
        ]),
        Line::from(vec![
            muted(p, "Required by "),
            Span::raw(summarise(&component.required_by)),
        ]),
        Line::from(vec![
            muted(p, "Detected by "),
            muted(p, component.detection.label()),
        ]),
    ];

    if let Some(description) = &component.description {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::raw(description.clone())));
    }
    for field in &component.extra {
        lines.push(Line::from(vec![
            muted(p, format!("{:<13}", field.label)),
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
    format!(
        "{} … (+{} more)",
        items[..LIMIT].join(", "),
        items.len() - LIMIT
    )
}

fn draw_details_pane(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let block = Block::default().borders(Borders::ALL).title(" SELECTED ");
    let Some(component) = state.selected_component() else {
        frame.render_widget(Paragraph::new("Nothing selected").block(block), area);
        return;
    };
    let lines = detail_lines(p, component);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Extra facts contributed by secondary sources (AUR metadata, for example), with
/// their pending and empty states stated plainly.
fn enrichment_lines(state: &UiState, component: &Component) -> Vec<Line<'static>> {
    let p = &state.theme.palette;
    match &state.enrichment {
        Some((id, fields)) if id == &component.id => {
            if fields.is_empty() {
                return Vec::new();
            }
            let mut lines = vec![Line::from(""), Line::from(accent(p, "REMOTE METADATA"))];
            for field in fields {
                lines.push(Line::from(vec![
                    muted(p, format!("{:<18}", field.label)),
                    Span::raw(field.value.clone()),
                ]));
            }
            lines
        }
        _ => vec![
            Line::from(""),
            Line::from(muted(p, "REMOTE METADATA")),
            Line::from(muted(p, "  looking it up…")),
        ],
    }
}

fn draw_details_screen(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let Some(component) = state.selected_component() else {
        frame.render_widget(
            Paragraph::new("Nothing selected").block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };
    let mut lines = detail_lines(p, component);
    lines.extend(enrichment_lines(state, component));

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(p.accent))
                    .title(" DETAILS "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_plan(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let Some(pending) = &state.pending else {
        frame.render_widget(
            Paragraph::new("No operation is being prepared.")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };
    let plan = &pending.plan;
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            bold(plan.target_name.clone()),
            muted(p, "  "),
            accent(p, shown(plan.target_version.as_deref()).to_string()),
            muted(p, "   "),
            Span::raw(plan.adapter.clone()),
        ]),
        Line::from(vec![
            muted(p, "Action      "),
            semantic(p.warning, plan.kind.label().to_uppercase()),
            muted(p, "   identity  "),
            Span::raw(plan.target.to_string()),
        ]),
        Line::from(""),
        Line::from(accent(p, "WHAT WILL RUN")),
    ];

    if plan.commands.is_empty() {
        lines.push(Line::from(semantic(
            p.error,
            "the adapter proposed no command: nothing to run",
        )));
    }
    for command in &plan.commands {
        let mut spans = vec![semantic(p.success, format!("  $ {}", command.display()))];
        if command.requires_root {
            spans.push(muted(p, "  [root]"));
        }
        if command.requires_tty {
            spans.push(muted(p, "  [terminal]"));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(vec![muted(
            p,
            format!("    {}", command.rationale),
        )]));
    }

    if plan.requires_tty() {
        lines.push(Line::from(vec![muted(
            p,
            "  The terminal is handed to this command while it runs, so a privilege prompt \
             works normally. No password is ever read or stored by this program.",
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(accent(p, "WHAT ELSE COULD BE AFFECTED")));
    if plan.affected.is_empty() {
        lines.push(Line::from(muted(
            p,
            "  nothing reported beyond the target itself",
        )));
    } else {
        for affected in &plan.affected {
            lines.push(Line::from(vec![
                semantic(p.warning, "  "),
                Span::raw(affected.clone()),
            ]));
        }
    }

    if !plan.blocking.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(
            p.error,
            "BLOCKED — THIS OPERATION WILL NOT RUN",
        )));
        for reason in &plan.blocking {
            lines.push(Line::from(vec![
                semantic(p.error, "  "),
                Span::raw(reason.clone()),
            ]));
        }
    }

    if !plan.unknown.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(p.warning, "NOT DETERMINED")));
        for fact in &plan.unknown {
            lines.push(Line::from(vec![
                semantic(p.warning, "  "),
                Span::raw(fact.clone()),
            ]));
        }
    }

    if !plan.notes.is_empty() {
        lines.push(Line::from(""));
        for note in &plan.notes {
            lines.push(Line::from(vec![muted(p, "  "), muted(p, note.clone())]));
        }
    }

    lines.push(Line::from(""));
    if pending.is_blocked() {
        lines.push(Line::from(semantic(
            p.error,
            "Nothing can be confirmed while the plan is blocked. Press Esc.",
        )));
    } else {
        lines.push(Line::from(vec![
            bold("Type  "),
            accent(p, pending.confirmation_phrase().to_string()),
            bold("  to confirm: "),
            Span::styled(state.plan_input.clone(), Style::new().fg(p.warning)),
            Span::styled("▏", Style::new().fg(p.accent)),
        ]));
        let satisfied = pending.confirmation_satisfied(&state.plan_input);
        lines.push(Line::from(if satisfied {
            semantic(
                p.success,
                "Press Enter to execute exactly the command above.",
            )
        } else {
            muted(p, "Enter stays disabled until the name matches exactly.")
        }));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(p.warning))
                    .title(" OPERATION PLAN "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_result(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let Some(record) = &state.last_record else {
        frame.render_widget(
            Paragraph::new("No operation has run in this session.")
                .block(Block::default().borders(Borders::ALL)),
            area,
        );
        return;
    };

    let banner = if record.success {
        semantic(p.success, format!("{} succeeded", record.kind))
    } else {
        semantic(p.error, format!("{} failed", record.kind))
    };

    let mut lines = vec![
        Line::from(banner),
        Line::from(""),
        Line::from(vec![
            muted(p, "Component   "),
            bold(record.component_name.clone()),
            muted(p, format!("  ({})", record.component)),
        ]),
        Line::from(vec![
            muted(p, "Exit code   "),
            Span::raw(
                record
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "terminated by a signal".to_string()),
            ),
        ]),
        Line::from(vec![
            muted(p, "Started     "),
            Span::raw(record.started_at()),
        ]),
        Line::from(vec![
            muted(p, "Command     "),
            Span::raw(record.command.clone()),
        ]),
        Line::from(vec![
            muted(p, "Confirmed   "),
            Span::raw(record.confirmed_by.clone()),
        ]),
        Line::from(vec![
            muted(p, "Summary     "),
            Span::raw(record.summary.clone()),
        ]),
    ];

    if !record.error_tail.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(semantic(p.error, "ERROR OUTPUT")));
        for line in record.error_tail.lines().take(12) {
            lines.push(Line::from(Span::raw(line.to_string())));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(muted(
        p,
        "The exit code and the error output are kept verbatim: this screen never reports a \
         failure as success.",
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(if record.success { p.success } else { p.error }))
                    .title(" OPERATION RESULT "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_cleanup(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let leftovers = state.reclaimable();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(p.warning))
        .title(format!(
            " CLEANUP — {} leftover(s) {} · selected {} ",
            leftovers.len(),
            format_size(state.reclaimable_total()),
            format_size(state.selected_reclaim_size())
        ));

    if leftovers.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "Nothing to clean up.\n\nThis screen lists only leftovers that no manager \
                 owns: plugin removal backups and Hyprland backup copies. System packages are \
                 never listed here — those are removed through the package manager, one at a \
                 time.",
            )
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    for (index, component) in leftovers.iter().enumerate() {
        let selected = state.cleanup_selected.contains(&component.id);
        let marker = if selected {
            accent(p, "[x]")
        } else {
            muted(p, "[ ]")
        };
        let size = format_size(state.size_of(component));
        let path = component
            .location
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| UNAVAILABLE.to_string());
        let style = if index == state.cleanup_index {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        items.push(
            ListItem::new(vec![
                Line::from(vec![
                    marker,
                    muted(p, " "),
                    bold(component.name.clone()),
                    muted(p, format!("  {} · {}", component.id.adapter, size)),
                ]),
                Line::from(vec![muted(p, "     "), muted(p, path)]),
            ])
            .style(style),
        );
    }

    let list = List::new(items).block(block);
    let mut list_state = ListState::default();
    list_state.select(Some(
        state.cleanup_index.min(leftovers.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_cleanup_confirm(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let Some(pending) = &state.pending_cleanup else {
        let boxed = modal_area(area, 3);
        frame.render_widget(Clear, boxed);
        frame.render_widget(
            Paragraph::new("No cleanup is being prepared.")
                .block(Block::default().borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            boxed,
        );
        return;
    };

    let quarantine = pending.action == crate::app::CleanupAction::Quarantine;
    let (title, headline, explain, accent_colour) = if quarantine {
        (
            " CONFIRM QUARANTINE ",
            format!(
                "{} item(s) will move, {} in total",
                pending.plans.len(),
                format_size(pending.total)
            ),
            "They are moved into the quarantine area and can be restored from there. \
             Nothing is deleted yet.",
            p.warning,
        )
    } else {
        (
            " CONFIRM PURGE ",
            format!("{} will be deleted for good", format_size(pending.total)),
            "This removes the quarantined copy permanently. There is no way back after \
             this step.",
            p.error,
        )
    };

    let mut paths: Vec<Line> = Vec::new();
    for plan in &pending.plans {
        for target in &plan.plan.reclaim {
            paths.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(
                    target
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| target.path.display().to_string()),
                ),
                muted(p, format!("   {}", format_size(target.size))),
            ]));
        }
    }
    for entry in &pending.entries {
        paths.push(Line::from(vec![
            Span::raw("  "),
            Span::raw(
                entry
                    .original
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.original.display().to_string()),
            ),
            muted(p, format!("   {}", format_size(entry.size))),
        ]));
    }

    // The window must fit; say how many paths are hidden instead of quietly cutting.
    let available = area.height.saturating_sub(12).max(2) as usize;
    let hidden = paths.len().saturating_sub(available);
    paths.truncate(available);
    if hidden > 0 {
        paths.push(Line::from(muted(p, format!("  … and {hidden} more"))));
    }

    let matches = pending.phrase == state.cleanup_input.trim();
    let mut lines = vec![
        Line::from(semantic(accent_colour, headline)),
        Line::from(muted(p, explain)),
        Line::from(""),
    ];
    lines.extend(paths);
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        bold("Type  "),
        accent(p, pending.phrase.clone()),
        bold("  and press Enter: "),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            state.cleanup_input.clone(),
            Style::new().fg(accent_colour).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▏", Style::new().fg(p.accent)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(if matches {
        semantic(
            p.success,
            "Enter now confirms. Esc cancels and changes nothing.",
        )
    } else {
        muted(
            p,
            "Enter stays disabled until the phrase matches exactly. Esc cancels.",
        )
    }));

    let boxed = modal_area(area, lines.len());
    frame.render_widget(Clear, boxed);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(accent_colour))
                    .title(Span::styled(
                        title,
                        Style::new().fg(accent_colour).add_modifier(Modifier::BOLD),
                    )),
            )
            .wrap(Wrap { trim: false }),
        boxed,
    );
}

fn draw_cleanup_result(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let Some(report) = &state.cleanup_report else {
        return;
    };
    let colour = if report.success { p.success } else { p.warning };
    let mut lines = vec![Line::from(semantic(
        colour,
        format!(
            "{}{}",
            if report.success { "✓ " } else { "! " },
            report.headline
        ),
    ))];
    lines.push(Line::from(""));
    for line in &report.lines {
        lines.push(Line::from(Span::raw(line.clone())));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(muted(p, "Enter or Esc closes this window.")));

    let boxed = modal_area(area, lines.len());
    frame.render_widget(Clear, boxed);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(colour))
                    .title(" RESULT "),
            )
            .wrap(Wrap { trim: true }),
        boxed,
    );
}

/// One row of colour swatches, so a theme can be judged without applying it.
fn swatches(palette: &theme::Palette) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled("   ", Style::new().bg(palette.accent)),
        Span::styled("   ", Style::new().bg(palette.success)),
        Span::styled("   ", Style::new().bg(palette.warning)),
        Span::styled("   ", Style::new().bg(palette.error)),
        Span::styled("   ", Style::new().bg(palette.muted)),
    ];
    if let Some(background) = palette.background {
        spans.push(Span::styled("   ", Style::new().bg(background)));
    }
    spans
}

fn draw_themes(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let themes = &state.themes.themes;

    let mut items: Vec<ListItem> = Vec::new();
    for (index, theme) in themes.iter().enumerate() {
        let marker = if index == state.theme_index {
            accent(p, "› ")
        } else {
            muted(p, "  ")
        };
        let origin = if theme.user_defined {
            muted(p, "  yours")
        } else {
            muted(p, "")
        };
        let style = if index == state.theme_index {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let mut header = vec![marker, bold(theme.name.clone())];
        if let Some(background) = theme.palette.background {
            header.push(Span::styled("  ", Style::new().bg(background)));
        }
        header.extend(swatches(&theme.palette));
        header.push(origin);
        items.push(ListItem::new(vec![Line::from(header)]).style(style));
    }

    let selected = themes.get(state.theme_index);
    let mut footer: Vec<Line> = vec![Line::from("")];
    if let Some(theme) = selected {
        if let Some(description) = &theme.description {
            footer.push(Line::from(muted(p, description.clone())));
        }
        if let Some(author) = &theme.author {
            footer.push(Line::from(muted(p, format!("by {author}"))));
        }
        footer.push(Line::from(muted(
            p,
            format!(
                "id '{}' — write it as theme = \"{}\" in config.toml to pin it",
                theme.id, theme.id
            ),
        )));
    }
    if state.themes.problems.is_empty() {
        footer.push(Line::from(""));
        footer.push(Line::from(muted(
            p,
            format!(
                "Press c to start your own from this palette (writes into {}).",
                state
                    .theme_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "your theme directory".to_string())
            ),
        )));
    } else {
        footer.push(Line::from(""));
        for problem in &state.themes.problems {
            footer.push(Line::from(semantic(
                p.error,
                format!("theme file rejected: {problem}"),
            )));
        }
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(p.accent))
        .title(bold(" THEMES "));
    let inner = block.inner(area);
    let rows = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(footer.len() as u16 + 1),
    ])
    .split(inner);
    frame.render_widget(block, area);

    let list = List::new(items);
    let mut list_state = ListState::default();
    list_state.select(Some(state.theme_index.min(themes.len().saturating_sub(1))));
    frame.render_stateful_widget(list, rows[0], &mut list_state);
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), rows[1]);
}

fn draw_quarantine(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" QUARANTINE — {} item(s) ", state.quarantine.len()));

    if state.quarantine.is_empty() {
        frame.render_widget(
            Paragraph::new(
                "The quarantine area is empty.\n\nAnything moved from the cleanup screen lands \
                 here with its original path recorded, so it can be restored. Purging is the only \
                 step that frees the space, and it deletes only what this list records.",
            )
            .block(block)
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();
    for (index, entry) in state.quarantine.iter().enumerate() {
        let style = if index == state.quarantine_index {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        items.push(
            ListItem::new(vec![
                Line::from(vec![
                    bold(entry.original.display().to_string()),
                    muted(p, format!("  {}", format_size(entry.size))),
                ]),
                Line::from(vec![
                    muted(p, "     quarantined at "),
                    muted(p, entry.location.display().to_string()),
                ]),
            ])
            .style(style),
        );
    }

    let list = List::new(items).block(block);
    let mut list_state = ListState::default();
    list_state.select(Some(
        state
            .quarantine_index
            .min(state.quarantine.len().saturating_sub(1)),
    ));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_history(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let block = Block::default().borders(Borders::ALL).title(format!(
        " OPERATION HISTORY ({}) ",
        state.history_entries.len()
    ));

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
            semantic(p.success, "ok  ")
        } else {
            semantic(p.error, "fail")
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
                    muted(p, format!(" {} ", record.started_at())),
                    bold(record.component_name.clone()),
                    muted(p, format!("  {} · {}", record.kind, record.adapter)),
                    muted(p, format!("  exit {:?}", record.exit_code)),
                ]),
                Line::from(vec![muted(p, format!("     $ {}", record.command))]),
            ])
            .style(style),
        );
    }

    let list = List::new(items).block(block).highlight_style(Style::new());
    let mut list_state = ListState::default();
    list_state.select(Some(
        state.history_selected.min(state.history_entries.len() - 1),
    ));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_adapters(frame: &mut Frame, area: Rect, state: &UiState) {
    let p = &state.theme.palette;
    let header = Row::new(vec!["ADAPTER", "STATE", "TOOL", "DETAIL"])
        .style(Style::new().fg(p.muted).add_modifier(Modifier::BOLD));

    let rows = state.snapshot.statuses.iter().map(|status| {
        let (state_text, color) = match &status.availability {
            AdapterAvailability::Available => ("available", p.success),
            AdapterAvailability::Unavailable { .. } => ("unavailable", p.warning),
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
                    semantic(p.error, format!("{} ", failure.adapter)),
                    muted(p, format!("{}: ", failure.stage)),
                    Span::raw(failure.message.clone()),
                ])
            })
            .chain(state.snapshot.duplicates.iter().map(|duplicate| {
                Line::from(vec![
                    semantic(p.warning, "duplicate identity "),
                    Span::raw(duplicate.id.to_string()),
                    muted(
                        p,
                        format!(
                            " kept by {}, rejected from {}",
                            duplicate.kept, duplicate.rejected
                        ),
                    ),
                ])
            }))
            .collect();

        let height = (lines.len() as u16 + 2)
            .min(area.height.saturating_sub(2))
            .max(3);
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
                        .border_style(Style::new().fg(p.error))
                        .title(" ADAPTER ISSUES "),
                )
                .wrap(Wrap { trim: true }),
            failure_area,
        );
    }
}

fn draw_help(frame: &mut Frame, area: Rect, palette: &theme::Palette) {
    let p = palette;
    let rows = [
        ("Navigation", ""),
        ("  ↑ / ↓, k / j", "move inside the focused pane"),
        ("  Tab", "switch between the category sidebar and the list"),
        ("  ← / →, h / l", "same as Tab"),
        ("  Enter", "open the details of the selected component"),
        ("  Esc", "go back, then quit from the inventory"),
        ("  /", "search by name, identifier, description or source"),
        (
            "  d / u",
            "preview a removal or an update for the selected component",
        ),
        ("  H", "operation history: what ran, when, and how it ended"),
        ("  t", "themes: preview and apply a colour scheme"),
        ("  i / a / ?", "inventory, adapter status, this help"),
        ("  q, Q", "quit (never interrupts a running operation)"),
        ("", ""),
        ("Reading the list", ""),
        (
            "  STATE",
            "installed or orphan; orphans are informative only",
        ),
        (
            "  SOURCE",
            "the manager that owns the component: pacman, AUR/foreign, flatpak",
        ),
        ("", ""),
        ("Honest facts", ""),
        ("  No disponible", "the source does not provide this value"),
        (
            "  unavailable",
            "the tool is not installed; the reason is shown and nothing else breaks",
        ),
        ("  Detection", "the exact command that produced the fact"),
        ("", ""),
        ("Safety", ""),
        (
            "  read-only",
            "start with --read-only to hide every destructive action",
        ),
        (
            "  confirmation",
            "destructive actions always show a plan and require the exact name",
        ),
    ];

    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, description)| {
            if key.is_empty() {
                Line::from("")
            } else if description.is_empty() {
                Line::from(accent(p, *key))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CleanupAction, CleanupReport, PendingCleanup, UiState};
    use crate::application::InventorySnapshot;
    use crate::domain::{
        ComponentCapability, ComponentCategory, ComponentId, ComponentSource, ComponentStatus,
        DetectionConfidence, DetectionMethod, Quarantined,
    };
    use crate::infrastructure::history::History;
    use crate::infrastructure::system::SystemInfo;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Renders one frame and returns the screen as text, so a test can assert what a
    /// human would actually see. This is the check that "there is no feedback" fails on.
    fn render(state: &UiState, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..height {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn state() -> UiState {
        let system = SystemInfo {
            os_name: Some("Omarchy".to_string()),
            os_id: Some("omarchy".to_string()),
            version_id: Some("4.5.0".to_string()),
            kernel: None,
            is_arch_like: true,
            is_omarchy: true,
            euid: Some(1000),
        };
        let history = History::new(std::env::temp_dir().join("occ-ui-test/history.jsonl"));
        UiState::new(system, false, Vec::new(), history)
    }

    fn leftover() -> Component {
        Component {
            id: ComponentId::new("omarchy", ".denon.audio.bak.1786584854"),
            name: "denon.audio".to_string(),
            version: None,
            description: Some("leftover of an earlier plugin removal".to_string()),
            category: ComponentCategory::Plugin,
            source: ComponentSource::OmarchyPlugin { first_party: false },
            status: ComponentStatus::Orphan,
            confidence: DetectionConfidence::Low,
            capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Reclaim],
            detection: DetectionMethod::scan("/tmp/plugins"),
            installed_size: None,
            location: Some(std::path::PathBuf::from(
                "/tmp/plugins/.denon.audio.bak.1786584854",
            )),
            installed_at: None,
            depends_on: Vec::new(),
            required_by: Vec::new(),
            extra: Vec::new(),
        }
    }

    fn state_with_leftover() -> UiState {
        let mut state = state();
        state.snapshot = InventorySnapshot {
            components: vec![leftover()],
            statuses: Vec::new(),
            failures: Vec::new(),
            duplicates: Vec::new(),
        };
        state
    }

    // ---------- documentation screenshots ----------
    //
    // There is no terminal in CI, so the images in the README are produced from the real
    // renderer: a fixture inventory is drawn into a ratatui `TestBackend` and the resulting
    // buffer is written as SVG. Regenerate with:
    //
    //     cargo test --bin omarchy-control-center -- --ignored --nocapture
    //
    // This is why the screenshot cannot drift from the code: it *is* the code.

    fn hex_of(color: Color, fallback: &str) -> String {
        match color {
            Color::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
            Color::Black => "#000000".to_string(),
            Color::Red => "#cd3131".to_string(),
            Color::Green => "#0dbc79".to_string(),
            Color::Yellow => "#e5e510".to_string(),
            Color::Blue => "#2472c8".to_string(),
            Color::Magenta => "#bc3fbc".to_string(),
            Color::Cyan => "#11a8cd".to_string(),
            Color::Gray => "#e5e5e5".to_string(),
            Color::DarkGray => "#666666".to_string(),
            Color::White => "#ffffff".to_string(),
            Color::LightRed => "#f14c4c".to_string(),
            Color::LightGreen => "#23d18b".to_string(),
            Color::LightYellow => "#f5f543".to_string(),
            Color::LightBlue => "#3b8eea".to_string(),
            Color::LightMagenta => "#d670d6".to_string(),
            Color::LightCyan => "#29b8db".to_string(),
            Color::Indexed(_) | Color::Reset => fallback.to_string(),
        }
    }

    fn svg_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    /// A cell's colours, with `REVERSED` applied: the interface marks a highlighted row with
    /// reverse video, and an image that ignored it would not show what the user sees.
    fn cell_colors(cell: &ratatui::buffer::Cell, fallback: &str) -> (String, String) {
        let mut foreground = hex_of(cell.fg, fallback);
        let mut background = hex_of(cell.bg, fallback);
        if cell.modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut foreground, &mut background);
        }
        (foreground, background)
    }

    /// One SVG per buffer: background rectangles, then runs of text sharing a colour.
    fn render_svg(state: &UiState, width: u16, height: u16) -> String {
        const CELL_WIDTH: u16 = 8;
        const CELL_HEIGHT: u16 = 17;
        const PADDING: u16 = 12;

        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer();
        let background = hex_of(
            state.theme.palette.background.unwrap_or(Color::Reset),
            "#101216",
        );
        let default_foreground = hex_of(
            state.theme.palette.foreground.unwrap_or(Color::Reset),
            "#d8dee9",
        );

        let mut svg = String::new();
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" \
             viewBox=\"0 0 {} {}\">\n",
            u32::from(width) * u32::from(CELL_WIDTH) + u32::from(PADDING) * 2,
            u32::from(height) * u32::from(CELL_HEIGHT) + u32::from(PADDING) * 2,
            u32::from(width) * u32::from(CELL_WIDTH) + u32::from(PADDING) * 2,
            u32::from(height) * u32::from(CELL_HEIGHT) + u32::from(PADDING) * 2,
        ));
        svg.push_str(&format!(
            "  <rect width=\"100%\" height=\"100%\" fill=\"{background}\"/>\n"
        ));

        // Cell backgrounds first, so text sits on top of them.
        for y in 0..height {
            for x in 0..width {
                let cell = &buffer[(x, y)];
                let (_, cell_background) = cell_colors(cell, &background);
                if cell_background != background {
                    svg.push_str(&format!(
                        "  <rect x=\"{}\" y=\"{}\" width=\"{CELL_WIDTH}\" height=\"{CELL_HEIGHT}\" fill=\"{cell_background}\"/>\n",
                        u32::from(x) * u32::from(CELL_WIDTH) + u32::from(PADDING),
                        u32::from(y) * u32::from(CELL_HEIGHT) + u32::from(PADDING),
                    ));
                }
            }
        }

        // Then text, one run per maximal span of equal foreground colour.
        for y in 0..height {
            let mut x = 0;
            while x < width {
                let start = x;
                let colour = cell_colors(&buffer[(x, y)], &default_foreground).0;
                let mut text = String::new();
                while x < width {
                    let cell = &buffer[(x, y)];
                    if cell_colors(cell, &default_foreground).0 != colour {
                        break;
                    }
                    text.push_str(cell.symbol());
                    x += 1;
                }
                let trimmed = text.trim_end();
                if !trimmed.is_empty() {
                    svg.push_str(&format!(
                        "  <text x=\"{}\" y=\"{}\" fill=\"{colour}\" \
                         font-family=\"'JetBrains Mono','Fira Code',monospace\" \
                         font-size=\"13\" xml:space=\"preserve\">{}</text>\n",
                        u32::from(start) * u32::from(CELL_WIDTH) + u32::from(PADDING),
                        u32::from(y) * u32::from(CELL_HEIGHT) + u32::from(PADDING) + 13,
                        svg_escape(trimmed),
                    ));
                }
            }
        }
        svg.push_str("</svg>\n");
        svg
    }

    fn screenshot_state(theme_id: &str, screen: Screen) -> UiState {
        let mut state = state();
        state.themes = crate::ui::theme::catalogue(std::path::Path::new("/nonexistent"));
        state.theme = state
            .themes
            .find(theme_id)
            .cloned()
            .unwrap_or_else(crate::ui::theme::default_theme);
        state.loading = false;
        state.snapshot = fixture_inventory();
        state.measure_leftovers();
        state
            .cleanup_selected
            .insert(state.snapshot.components[0].id.clone());
        state.screen = screen;
        state.theme_index = state.active_theme_index();
        state
    }

    /// A believable inventory for the images: several ecosystems, familiar names.
    fn fixture_inventory() -> InventorySnapshot {
        /// The fields of one fixture row, named at the call site.
        /// One realistic row for the images. Eight positional arguments read better here
        /// than a struct whose fields are each used exactly once.
        #[allow(clippy::too_many_arguments)]
        fn component(
            adapter: &str,
            name: &str,
            version: &str,
            source: ComponentSource,
            status: ComponentStatus,
            category: ComponentCategory,
            size: u64,
            description: &str,
        ) -> Component {
            Component {
                id: ComponentId::new(adapter, name),
                name: name.to_string(),
                version: Some(version.to_string()),
                description: Some(description.to_string()),
                category,
                source,
                status,
                confidence: DetectionConfidence::Authoritative,
                capabilities: vec![ComponentCapability::Inspect, ComponentCapability::Remove],
                detection: DetectionMethod::command("pacman", vec!["-Q".to_string()]),
                installed_size: Some(size),
                location: Some(std::path::PathBuf::from("/usr/bin").join(name)),
                installed_at: None,
                depends_on: vec!["glibc".to_string()],
                required_by: Vec::new(),
                extra: Vec::new(),
            }
        }

        let official = || ComponentSource::Pacman {
            origin: crate::domain::PacmanOrigin::Official {
                repo: Some("extra".to_string()),
            },
        };
        let foreign = || ComponentSource::Pacman {
            origin: crate::domain::PacmanOrigin::Foreign,
        };

        let mut components = vec![
            component(
                "pacman",
                "firefox",
                "145.0-1",
                official(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                251_658_240,
                "Standalone web browser from Mozilla",
            ),
            component(
                "pacman",
                "neovim",
                "0.12.6-1",
                official(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                31_457_280,
                "Fork of Vim aiming to improve user experience",
            ),
            component(
                "pacman",
                "kitty",
                "0.44.1-1",
                official(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                12_582_912,
                "A modern, hackable, featureful, OpenGL based terminal",
            ),
            component(
                "pacman",
                "hyprland",
                "0.56.2-4",
                official(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                74_514_432,
                "A dynamic tiling Wayland compositor",
            ),
            component(
                "pacman",
                "ripgrep",
                "14.1.0-1",
                official(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                5_242_880,
                "A search tool that combines the usability of ag with the raw speed of grep",
            ),
            component(
                "pacman",
                "bruno-bin",
                "4.2.1-1",
                foreign(),
                ComponentStatus::Installed,
                ComponentCategory::Package,
                188_743_680,
                "Offline-first API client",
            ),
            component(
                "pacman",
                "libfoo",
                "1.0.0-2",
                foreign(),
                ComponentStatus::Orphan,
                ComponentCategory::Package,
                1_048_576,
                "Left behind by a package that no longer needs it",
            ),
            component(
                "omarchy",
                "Clock",
                "1.0.0",
                ComponentSource::OmarchyPlugin { first_party: true },
                ComponentStatus::Enabled,
                ComponentCategory::Plugin,
                0,
                "A quiet clock in the bar",
            ),
            component(
                "omarchy",
                "Houra",
                "1.0.0",
                ComponentSource::OmarchyPlugin { first_party: false },
                ComponentStatus::Enabled,
                ComponentCategory::Plugin,
                0,
                "A calm focus timer with ambient views",
            ),
            component(
                "omarchy",
                "denon.audio",
                "3.2.0",
                ComponentSource::OmarchyPlugin { first_party: false },
                ComponentStatus::Orphan,
                ComponentCategory::Plugin,
                204_712,
                "leftover of an earlier plugin removal",
            ),
            component(
                "lazy",
                "blink.cmp",
                "78336bc",
                ComponentSource::LazyNvim { downloaded: true },
                ComponentStatus::Installed,
                ComponentCategory::Plugin,
                0,
                "Completion plugin for Neovim",
            ),
            component(
                "lazy",
                "sidekick.nvim",
                "1f2c9de",
                ComponentSource::LazyNvim { downloaded: false },
                ComponentStatus::Missing,
                ComponentCategory::Plugin,
                0,
                "Configured in lazy-lock.json but never downloaded",
            ),
            component(
                "zsh",
                "zsh-autosuggestions",
                "-",
                ComponentSource::Zsh {
                    managed_by: crate::domain::ZshManager::OhMyZsh,
                },
                ComponentStatus::Installed,
                ComponentCategory::Plugin,
                0,
                "Fish-like autosuggestions for zsh",
            ),
            component(
                "systemd",
                "bluetooth.service",
                "-",
                ComponentSource::Systemd {
                    scope: crate::domain::InstallScope::System,
                },
                ComponentStatus::Active,
                ComponentCategory::Service,
                0,
                "Bluetooth service",
            ),
            component(
                "systemd",
                "pipewire-pulse.service",
                "-",
                ComponentSource::Systemd {
                    scope: crate::domain::InstallScope::User,
                },
                ComponentStatus::Active,
                ComponentCategory::Service,
                0,
                "PipeWire PulseAudio server",
            ),
            component(
                "cargo",
                "ripgrep",
                "14.1.0",
                ComponentSource::Cargo,
                ComponentStatus::Installed,
                ComponentCategory::Tool,
                0,
                "Installed with cargo install",
            ),
            component(
                "hyprland",
                "fix-ultrawide-mode",
                "-",
                ComponentSource::Hyprland {
                    reference_kind: "launch target".to_string(),
                },
                ComponentStatus::Installed,
                ComponentCategory::Configuration,
                0,
                "started by the configuration",
            ),
            component(
                "resources",
                "JetBrainsMono Nerd Font",
                "-",
                ComponentSource::Resource {
                    resource_kind: crate::domain::ResourceKind::Font,
                },
                ComponentStatus::Installed,
                ComponentCategory::Font,
                0,
                "Monospaced font with icons",
            ),
            component(
                "resources",
                "cyberpunk",
                "-",
                ComponentSource::Resource {
                    resource_kind: crate::domain::ResourceKind::Theme,
                },
                ComponentStatus::Installed,
                ComponentCategory::Theme,
                0,
                "Omarchy theme",
            ),
            component(
                "flatpak",
                "com.spotify.Client",
                "1.2.52",
                ComponentSource::Flatpak {
                    scope: crate::domain::InstallScope::User,
                },
                ComponentStatus::Installed,
                ComponentCategory::Application,
                0,
                "Music for everyone",
            ),
        ];
        components.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        let mut statuses = Vec::new();
        for (id, name, available) in [
            ("pacman", "Pacman", true),
            ("omarchy", "Omarchy plugins", true),
            ("aur", "AUR helper", true),
            ("flatpak", "Flatpak", true),
            ("lazy", "Neovim plugins", true),
            ("systemd", "Services", true),
            ("zsh", "Zsh plugins", true),
            ("hyprland", "Hyprland configuration", true),
            ("cargo", "Cargo tools", true),
            ("python", "Python tools", false),
            ("resources", "Fonts, themes & icons", true),
        ] {
            statuses.push(crate::domain::AdapterStatus {
                id: id.to_string(),
                display_name: name.to_string(),
                description: String::new(),
                availability: if available {
                    crate::domain::AdapterAvailability::Available
                } else {
                    crate::domain::AdapterAvailability::Unavailable {
                        reason: "neither uv nor pipx was found".to_string(),
                    }
                },
                tool: None,
                tool_version: None,
                capabilities: Vec::new(),
                commands: Vec::new(),
                notes: Vec::new(),
            });
        }

        InventorySnapshot {
            components,
            statuses,
            failures: Vec::new(),
            duplicates: Vec::new(),
        }
    }

    #[test]
    #[ignore = "regenerates the images in docs/screenshots; run it on purpose"]
    fn regenerate_documentation_screenshots() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
        std::fs::create_dir_all(&root).expect("screenshots dir");

        let shots: [(&str, &str, Screen, u16, u16); 3] = [
            (
                "inventory-cyberpunk.svg",
                "cyberpunk",
                Screen::Inventory,
                108,
                30,
            ),
            ("themes-monokai.svg", "monokai", Screen::Themes, 108, 30),
            ("cleanup-matrix.svg", "matrix", Screen::Cleanup, 108, 30),
        ];
        for (file, theme, screen, width, height) in shots {
            let state = screenshot_state(theme, screen);
            let svg = render_svg(&state, width, height);
            std::fs::write(root.join(file), svg).expect("write svg");
            println!("wrote {file}");
        }
    }

    #[test]
    fn the_cleanup_screen_lists_the_leftovers_it_would_move() {
        let mut state = state_with_leftover();
        state.screen = Screen::Cleanup;

        let screen = render(&state, 100, 24);
        assert!(screen.contains("CLEANUP"), "no cleanup heading:\n{screen}");
        assert!(
            screen.contains("denon.audio"),
            "the leftover is not shown:\n{screen}"
        );
        assert!(
            screen.contains("[ ]"),
            "the selection marker is missing:\n{screen}"
        );
    }

    #[test]
    fn the_purge_confirmation_is_a_floating_window_that_asks_for_the_phrase() {
        let mut state = state_with_leftover();
        state.screen = Screen::Cleanup;
        state.previous_screen = Screen::Cleanup;
        let entry = Quarantined {
            original: std::path::PathBuf::from("/tmp/plugins/.denon.audio.bak.1786584854"),
            location: std::path::PathBuf::from("/tmp/state/quarantine/7/.denon.audio.bak"),
            root: std::path::PathBuf::from("/tmp/plugins"),
            size: 204_712,
            stamp: 7,
            moved_at_unix: 1_700_000_000,
        };
        state.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Purge,
            plans: Vec::new(),
            entries: vec![entry],
            total: 204_712,
            phrase: "purge /tmp/plugins/.denon.audio.bak.1786584854".to_string(),
        });
        state.screen = Screen::CleanupConfirm;

        let screen = render(&state, 110, 30);
        assert!(
            screen.contains("CONFIRM PURGE"),
            "no confirmation window:\n{screen}"
        );
        assert!(
            screen.contains("deleted for good"),
            "the consequence is not stated:\n{screen}"
        );
        assert!(
            screen.contains(".denon.audio.bak.1786584854"),
            "the path being purged is not shown:\n{screen}"
        );
        assert!(
            screen.contains("purge /tmp/plugins"),
            "the phrase to type is not shown:\n{screen}"
        );
        assert!(
            screen.contains("Enter stays disabled"),
            "the confirmation state is not explained:\n{screen}"
        );
        // The list behind it stays visible: it is a floating window, not a new page.
        assert!(
            screen.contains("CLEANUP"),
            "the background screen disappeared:\n{screen}"
        );
    }

    #[test]
    fn the_quarantine_confirmation_says_that_nothing_is_deleted_yet() {
        let mut state = state_with_leftover();
        state.previous_screen = Screen::Cleanup;
        state.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Quarantine,
            plans: Vec::new(),
            entries: Vec::new(),
            total: 0,
            phrase: "quarantine 0".to_string(),
        });
        state.screen = Screen::CleanupConfirm;

        let screen = render(&state, 110, 30);
        assert!(
            screen.contains("CONFIRM QUARANTINE"),
            "no confirmation window:\n{screen}"
        );
        assert!(
            screen.contains("can be restored"),
            "reversibility is not stated:\n{screen}"
        );
        assert!(
            screen.contains("quarantine 0"),
            "the phrase is missing:\n{screen}"
        );
    }

    #[test]
    fn typing_the_phrase_flips_the_window_to_reassure_the_user() {
        let mut state = state_with_leftover();
        state.previous_screen = Screen::Cleanup;
        state.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Quarantine,
            plans: Vec::new(),
            entries: Vec::new(),
            total: 0,
            phrase: "quarantine 3".to_string(),
        });
        state.cleanup_input = "quarantine 3".to_string();
        state.screen = Screen::CleanupConfirm;

        let screen = render(&state, 110, 30);
        assert!(
            screen.contains("Enter now confirms"),
            "no confirmation feedback:\n{screen}"
        );
        assert!(!screen.contains("Enter stays disabled"));
    }

    #[test]
    fn the_result_window_reports_what_happened_with_the_numbers() {
        let mut state = state_with_leftover();
        state.cleanup_report = Some(CleanupReport {
            success: true,
            headline: "Moved into quarantine".to_string(),
            lines: vec![
                "3 item(s) · 1.4 MiB".to_string(),
                "They are in the quarantine area with their original paths recorded".to_string(),
            ],
        });
        state.screen = Screen::CleanupResult;

        let screen = render(&state, 110, 30);
        assert!(screen.contains("RESULT"), "no result window:\n{screen}");
        assert!(
            screen.contains("Moved into quarantine"),
            "no headline:\n{screen}"
        );
        assert!(screen.contains("3 item(s)"), "no numbers:\n{screen}");
        assert!(
            screen.contains("Enter or Esc closes"),
            "no way out is explained:\n{screen}"
        );
    }

    #[test]
    fn the_list_shows_the_measured_size_of_each_leftover() {
        let dir = std::env::temp_dir().join(format!("occ-ui-size-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("data.bin"), vec![b'x'; 2048]).expect("file");

        let mut state = state();
        let mut component = leftover();
        component.id = ComponentId::new("omarchy", ".measured.bak.1");
        component.name = "measured".to_string();
        component.location = Some(dir.clone());
        state.snapshot = InventorySnapshot {
            components: vec![component],
            statuses: Vec::new(),
            failures: Vec::new(),
            duplicates: Vec::new(),
        };
        // The sizes are measured once, when the view opens: the draw loop reads them.
        state.measure_leftovers();
        state
            .cleanup_selected
            .insert(state.snapshot.components[0].id.clone());
        state.screen = Screen::Cleanup;

        let screen = render(&state, 100, 20);
        assert!(
            screen.contains("2.0 KiB"),
            "the size is not shown:\n{screen}"
        );
        assert!(
            screen.contains("selected 2.0 KiB"),
            "the selected total is not shown:\n{screen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_theme_selector_lists_every_theme_with_its_swatches() {
        let mut state = state();
        state.themes = crate::ui::theme::catalogue(std::path::Path::new("/nonexistent"));
        state.theme_index = 5;
        state.theme = state.themes.themes[5].clone();
        state.screen = Screen::Themes;

        let screen = render(&state, 120, 30);
        for expected in [
            "THEMES",
            "Terminal default",
            "The Legend of Zelda",
            "Cyberpunk",
            "Rei Ayanami",
            "Monokai",
            "Matrix",
            "C. Viper",
        ] {
            assert!(
                screen.contains(expected),
                "{expected} is missing:\n{screen}"
            );
        }
        // The description of the highlighted theme, and its id for config.toml.
        assert!(screen.contains("Green phosphor on black"), "{screen}");
        assert!(screen.contains("theme = \"matrix\""), "{screen}");
        assert!(
            screen.contains("Press c to start your own"),
            "creating a theme is not discoverable:\n{screen}"
        );
    }

    #[test]
    fn the_selector_reports_a_rejected_theme_file() {
        let dir = std::env::temp_dir().join(format!("occ-ui-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("themes")).expect("dir");
        std::fs::write(
            dir.join("themes/broken.toml"),
            "name = \"Broken\"\n[colors]\nmuted = \"#333\"\naccent = \"nope\"\n\
             success = \"#0f0\"\nwarning = \"#ff0\"\nerror = \"#f00\"\n",
        )
        .expect("write");

        let mut state = state();
        state.themes = crate::ui::theme::catalogue(&dir);
        state.screen = Screen::Themes;

        let screen = render(&state, 120, 30);
        assert!(screen.contains("theme file rejected"), "{screen}");
        assert!(
            screen.contains("accent"),
            "the bad field is not named:\n{screen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_themed_palette_paints_the_frame_background() {
        let mut state = state();
        let matrix = crate::ui::theme::builtin_themes()
            .into_iter()
            .find(|theme| theme.id == "matrix")
            .expect("matrix");
        state.theme = matrix;
        state.screen = Screen::Inventory;

        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| draw(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        // The default theme leaves the terminal alone; this one fills the frame.
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0x00, 0x00, 0x00));
    }

    #[test]
    fn a_small_terminal_still_shows_the_phrase_and_the_warning() {
        let mut state = state_with_leftover();
        state.previous_screen = Screen::Cleanup;
        state.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Quarantine,
            plans: Vec::new(),
            entries: Vec::new(),
            total: 0,
            phrase: "quarantine 2".to_string(),
        });
        state.screen = Screen::CleanupConfirm;

        let screen = render(&state, 60, 16);
        assert!(
            screen.contains("quarantine 2"),
            "the phrase vanished on a small screen:\n{screen}"
        );
    }
}
