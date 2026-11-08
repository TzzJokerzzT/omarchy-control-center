//! The interface process: state, events and the draw loop.
//!
//! Loading happens on background threads and arrives as messages. The loop never
//! waits for a command, so the interface stays responsive while adapters work.
//!
//! The one exception is deliberate: an operation that needs the controlling
//! terminal (a privilege prompt) releases the terminal to that command, waits for
//! it, and takes the terminal back. Nothing else can run during that handoff, which
//! is exactly the guarantee we want for a destructive action.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::application::ConfirmDecision;
use crate::application::operations::{self, OperationRecord, PendingOperation};
use crate::application::{AdapterRegistry, InventorySnapshot, search};
use crate::domain::{Component, ComponentId, DetailField, OperationKind, SourceGroup};
use crate::infrastructure::history::History;
use crate::infrastructure::system::SystemInfo;
use crate::term::TerminalSession;
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Inventory,
    Details,
    Plan,
    Result,
    History,
    Adapters,
    Help,
}

/// Which pane owns the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
}

/// Everything an action needs from outside the state, so the state never owns the
/// system: the registry to route an operation, and the terminal to hand over.
pub struct Actions<'a> {
    pub registry: &'a AdapterRegistry,
    pub session: &'a mut TerminalSession,
}

pub struct UiState {
    pub system: SystemInfo,
    pub read_only: bool,
    pub notices: Vec<String>,
    pub snapshot: InventorySnapshot,
    pub screen: Screen,
    pub previous_screen: Screen,
    pub focus: Focus,
    pub loading: bool,
    pub reload_requested: bool,
    /// Forces a full repaint after the terminal was handed to a command.
    pub needs_full_redraw: bool,
    pub group_filter: Option<SourceGroup>,
    pub sidebar_index: usize,
    pub selected: usize,
    pub query: String,
    pub searching: bool,
    pub pending: Option<PendingOperation>,
    pub plan_input: String,
    pub last_record: Option<OperationRecord>,
    /// Extra facts for the open detail view, with the component they belong to.
    pub enrichment: Option<(ComponentId, Vec<DetailField>)>,
    /// A lookup the loop must run off the draw path.
    pub enrich_requested: Option<Component>,
    pub history: History,
    pub history_entries: Vec<OperationRecord>,
    pub history_selected: usize,
    pub quit: bool,
    pub status_line: String,
}

impl UiState {
    pub fn new(
        system: SystemInfo,
        read_only: bool,
        notices: Vec<String>,
        history: History,
    ) -> Self {
        let status_line = if read_only {
            "read-only mode: destructive actions are disabled".to_string()
        } else {
            String::new()
        };
        Self {
            system,
            read_only,
            notices,
            snapshot: InventorySnapshot::default(),
            screen: Screen::Inventory,
            previous_screen: Screen::Inventory,
            focus: Focus::List,
            loading: true,
            reload_requested: false,
            needs_full_redraw: false,
            group_filter: None,
            sidebar_index: 0,
            selected: 0,
            query: String::new(),
            searching: false,
            pending: None,
            plan_input: String::new(),
            last_record: None,
            enrichment: None,
            enrich_requested: None,
            history,
            history_entries: Vec::new(),
            history_selected: 0,
            quit: false,
            status_line,
        }
    }

    /// Sidebar entries: "All" followed by the groups that actually have components.
    pub fn sidebar_groups(&self) -> Vec<Option<SourceGroup>> {
        let mut groups: Vec<SourceGroup> = self
            .snapshot
            .components
            .iter()
            .map(|component| component.source.group())
            .collect();
        groups.sort();
        groups.dedup();
        let mut entries = vec![None];
        entries.extend(groups.into_iter().map(Some));
        entries
    }

    pub fn sidebar_label(&self, index: usize) -> (&'static str, usize) {
        let groups = self.sidebar_groups();
        let entry = groups.get(index).copied().flatten();
        let count = self
            .snapshot
            .components
            .iter()
            .filter(|component| entry.is_none() || Some(component.source.group()) == entry)
            .count();
        (entry.map(|group| group.label()).unwrap_or("All"), count)
    }

    /// Components matching the active group filter and search, in display order.
    pub fn visible(&self) -> Vec<&Component> {
        let query = self.query.trim();
        if query.is_empty() {
            return self
                .snapshot
                .components
                .iter()
                .filter(|component| {
                    self.group_filter
                        .is_none_or(|group| component.source.group() == group)
                })
                .collect();
        }

        let mut matched: Vec<(i32, &Component)> = self
            .snapshot
            .components
            .iter()
            .filter(|component| {
                self.group_filter
                    .is_none_or(|group| component.source.group() == group)
            })
            .filter_map(|component| {
                search::score_component(query, component).map(|(score, _)| (score, component))
            })
            .collect();
        matched.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.sort_key().cmp(&right.1.sort_key()))
        });
        matched
            .into_iter()
            .map(|(_, component)| component)
            .collect()
    }

    pub fn selected_component(&self) -> Option<&Component> {
        self.visible().get(self.selected).copied()
    }

    fn apply(&mut self, message: Message) {
        match message {
            Message::Loaded(snapshot) => {
                self.loading = false;
                self.snapshot = *snapshot;
                self.clamp_selection();
                if self.snapshot.components.is_empty() && self.snapshot.failure_count() > 0 {
                    self.status_line =
                        "no component could be loaded: open the adapters view (a)".to_string();
                }
            }
            Message::Enriched { id, fields } => {
                self.enrichment = Some((id, fields));
            }
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.visible().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
        let groups = self.sidebar_groups().len();
        if self.sidebar_index >= groups {
            self.sidebar_index = groups.saturating_sub(1);
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::List,
            Focus::List => Focus::Sidebar,
        };
    }

    fn move_sidebar(&mut self, delta: isize) {
        let count = self.sidebar_groups().len();
        if count == 0 {
            return;
        }
        let next = self.sidebar_index as isize + delta;
        self.sidebar_index = next.clamp(0, count as isize - 1) as usize;
        self.group_filter = self
            .sidebar_groups()
            .get(self.sidebar_index)
            .copied()
            .flatten();
        self.selected = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.visible().len();
        if count == 0 {
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, count as isize - 1) as usize;
    }

    fn refresh_history(&mut self) {
        match self.history.recent(50) {
            Ok(entries) => {
                self.history_entries = entries;
                self.history_selected = 0;
            }
            Err(error) => {
                self.history_entries.clear();
                self.status_line = format!("cannot read the operation history: {error}");
            }
        }
    }

    fn open(&mut self, screen: Screen) {
        if self.screen != screen {
            self.previous_screen = self.screen;
            self.screen = screen;
        }
    }

    fn back(&mut self) {
        match self.screen {
            Screen::Inventory => {}
            _ => self.screen = self.previous_screen,
        }
    }

    /// Builds the preview for the selected component. A refusal is shown, never
    /// silently ignored: the human must know why an action is not offered.
    fn request_operation(&mut self, kind: OperationKind, actions: &Actions<'_>) {
        let Some(component) = self.selected_component().cloned() else {
            self.status_line = "select a component first".to_string();
            return;
        };
        if !component.supports(match kind {
            OperationKind::Remove => crate::domain::ComponentCapability::Remove,
            OperationKind::Update => crate::domain::ComponentCapability::Update,
        }) {
            self.status_line = format!(
                "{} is not offered for {} by its adapter",
                kind.label(),
                component.name
            );
            return;
        }

        match operations::prepare(actions.registry, &component, kind, self.read_only) {
            Ok(pending) => {
                if pending.is_blocked() {
                    self.status_line = format!("blocked: {}", pending.plan.blocking.join("; "));
                } else {
                    self.status_line.clear();
                }
                self.plan_input.clear();
                self.pending = Some(pending);
                self.open(Screen::Plan);
            }
            Err(error) => self.status_line = error.to_string(),
        }
    }

    /// Runs the confirmed plan: re-verifies it, hands the terminal over when the
    /// command needs it, and records the real outcome.
    fn confirm_operation(&mut self, actions: &mut Actions<'_>) {
        let Some(pending) = self.pending.clone() else {
            return;
        };

        match crate::application::decide(&pending, &self.plan_input) {
            ConfirmDecision::Execute => {}
            ConfirmDecision::Blocked(reasons) => {
                self.status_line = format!("blocked: {}", reasons.join("; "));
                return;
            }
            ConfirmDecision::PhraseMismatch { expected } => {
                self.status_line =
                    format!("type exactly '{expected}' to confirm, or Esc to cancel");
                return;
            }
        }

        let handover = pending.plan.requires_tty();
        if handover {
            // The command owns the terminal while it runs, so a privilege prompt
            // works normally and we never read a password ourselves.
            actions.session.suspend().ok();
        }
        let started = operations::now_unix();
        let outcome = operations::execute_verified(actions.registry, &pending, self.read_only);
        if handover {
            actions.session.resume().ok();
        }
        self.needs_full_redraw = true;

        match outcome {
            Ok(outcome) => {
                let record = OperationRecord::from_outcome(&pending.plan, &outcome, started);
                self.status_line = if outcome.success {
                    format!(
                        "{} completed: {}",
                        pending.plan.kind.label(),
                        pending.plan.target_name
                    )
                } else {
                    format!(
                        "{} failed with exit code {:?}: {}",
                        pending.plan.kind.label(),
                        outcome.exit_code,
                        outcome.stderr_tail.lines().next().unwrap_or("no message")
                    )
                };
                if let Err(error) = self.history.append(&record) {
                    // The outcome still stands; losing the audit line must be visible.
                    self.status_line = format!(
                        "{}. WARNING: the history could not be written: {error}",
                        self.status_line
                    );
                }
                self.last_record = Some(record);
                self.pending = None;
                self.plan_input.clear();
                self.screen = Screen::Result;
                // The inventory changed (or may have): read it again.
                self.reload_requested = true;
            }
            Err(error) => {
                self.status_line = error.to_string();
                self.pending = None;
                self.plan_input.clear();
                self.screen = Screen::Inventory;
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, actions: &mut Actions<'_>) {
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.query.clear();
                    self.searching = false;
                    self.selected = 0;
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                    self.selected = 0;
                }
                KeyCode::Char(character) if !character.is_control() => {
                    self.query.push(character);
                    self.selected = 0;
                }
                _ => {}
            }
            return;
        }

        // The confirmation field owns every printable key, so a name containing
        // 'q' can be typed without quitting.
        if self.screen == Screen::Plan {
            match key.code {
                KeyCode::Esc => {
                    self.pending = None;
                    self.plan_input.clear();
                    self.status_line = "operation cancelled: nothing was executed".to_string();
                    self.back();
                }
                KeyCode::Enter => self.confirm_operation(actions),
                KeyCode::Backspace => {
                    self.plan_input.pop();
                }
                KeyCode::Char(character) if !character.is_control() => {
                    self.plan_input.push(character);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('?') => {
                if self.screen == Screen::Help {
                    self.back();
                } else {
                    self.open(Screen::Help);
                }
            }
            KeyCode::Esc => {
                if self.screen == Screen::Inventory {
                    self.quit = true;
                } else {
                    self.back();
                }
            }
            KeyCode::Char('/') => {
                if self.screen == Screen::Inventory {
                    self.searching = true;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if self.screen == Screen::Inventory {
                    self.request_operation(OperationKind::Remove, actions);
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if self.screen == Screen::Inventory {
                    self.request_operation(OperationKind::Update, actions);
                }
            }
            KeyCode::Char('i') => self.screen = Screen::Inventory,
            KeyCode::Char('a') => self.open(Screen::Adapters),
            KeyCode::Char('H') => {
                if self.screen != Screen::History {
                    self.refresh_history();
                    self.open(Screen::History);
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.screen == Screen::Inventory {
                    self.toggle_focus();
                }
            }
            KeyCode::Enter => match self.screen {
                Screen::Inventory => match self.focus {
                    Focus::Sidebar => self.focus = Focus::List,
                    Focus::List => {
                        if let Some(component) = self.selected_component().cloned() {
                            // Secondary facts are looked up in the background; the
                            // pane shows they are pending instead of blocking.
                            if self
                                .enrichment
                                .as_ref()
                                .is_none_or(|(id, _)| id != &component.id)
                            {
                                self.enrich_requested = Some(component);
                            }
                            self.open(Screen::Details);
                        }
                    }
                },
                Screen::Result | Screen::Details | Screen::History => self.back(),
                _ => {}
            },
            KeyCode::Up | KeyCode::Char('k') => match (self.screen, self.focus) {
                (Screen::Inventory, Focus::Sidebar) => self.move_sidebar(-1),
                (Screen::Inventory, Focus::List) => self.move_selection(-1),
                (Screen::History, _) => {
                    self.history_selected = self.history_selected.saturating_sub(1);
                }
                _ => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match (self.screen, self.focus) {
                (Screen::Inventory, Focus::Sidebar) => self.move_sidebar(1),
                (Screen::Inventory, Focus::List) => self.move_selection(1),
                (Screen::History, _) if self.history_selected + 1 < self.history_entries.len() => {
                    self.history_selected += 1;
                }
                _ => {}
            },
            KeyCode::Right | KeyCode::Char('l') => {
                if self.screen == Screen::Inventory && self.focus == Focus::Sidebar {
                    self.focus = Focus::List;
                }
            }
            KeyCode::Left | KeyCode::Char('h')
                if self.screen == Screen::Inventory && self.focus == Focus::List =>
            {
                self.focus = Focus::Sidebar;
            }
            _ => {}
        }
    }
}

enum Message {
    Loaded(Box<InventorySnapshot>),
    Enriched {
        id: ComponentId,
        fields: Vec<DetailField>,
    },
}

pub struct Bootstrap {
    pub registry: Arc<AdapterRegistry>,
    pub history: History,
    pub system: SystemInfo,
    pub read_only: bool,
    pub notices: Vec<String>,
}

pub fn run(bootstrap: Bootstrap) -> anyhow::Result<()> {
    let mut session = TerminalSession::enter().context("cannot take control of the terminal")?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("cannot initialise the terminal renderer")?;
    terminal.clear().ok();

    let (sender, receiver) = mpsc::channel();
    spawn_loader(Arc::clone(&bootstrap.registry), sender.clone());

    let mut state = UiState::new(
        bootstrap.system,
        bootstrap.read_only,
        bootstrap.notices,
        bootstrap.history,
    );
    let registry = Arc::clone(&bootstrap.registry);
    let outcome = event_loop(
        &mut terminal,
        &mut state,
        &receiver,
        &registry,
        &mut session,
        &sender,
    );
    drop(terminal);
    session.suspend().ok();
    outcome
}

fn spawn_loader(registry: Arc<AdapterRegistry>, sender: mpsc::Sender<Message>) {
    std::thread::spawn(move || {
        let snapshot = registry.load_all();
        let _ = sender.send(Message::Loaded(Box::new(snapshot)));
    });
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut UiState,
    receiver: &Receiver<Message>,
    registry: &Arc<AdapterRegistry>,
    session: &mut TerminalSession,
    sender: &mpsc::Sender<Message>,
) -> anyhow::Result<()> {
    loop {
        while let Ok(message) = receiver.try_recv() {
            state.apply(message);
        }

        if let Some(component) = state.enrich_requested.take() {
            let registry = Arc::clone(registry);
            let sender = sender.clone();
            std::thread::spawn(move || {
                let fields = registry.enrich(&component);
                let _ = sender.send(Message::Enriched {
                    id: component.id,
                    fields,
                });
            });
        }

        if state.reload_requested {
            state.reload_requested = false;
            state.loading = true;
            spawn_loader(Arc::clone(registry), sender.clone());
        }

        if state.needs_full_redraw {
            state.needs_full_redraw = false;
            terminal.clear().ok();
        }

        terminal.draw(|frame| ui::draw(frame, state))?;

        if state.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(120)).context("cannot poll terminal events")? {
            match event::read().context("cannot read a terminal event")? {
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                    let mut actions = Actions {
                        registry: registry.as_ref(),
                        session,
                    };
                    state.on_key(key, &mut actions);
                }
                TerminalEvent::Resize(_, _) => state.needs_full_redraw = true,
                _ => {}
            }
        }
    }
}
