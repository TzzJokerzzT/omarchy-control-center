//! The interface process: state, events and the draw loop.
//!
//! Loading happens on background threads and arrives as messages. The loop never
//! waits for a command, so the interface stays responsive while adapters work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::application::{AdapterRegistry, InventorySnapshot, search};
use crate::domain::{Component, SourceGroup};
use crate::infrastructure::system::SystemInfo;
use crate::term::TerminalSession;
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Inventory,
    Details,
    Adapters,
    Help,
}

/// Which pane owns the arrow keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    List,
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
    /// `None` means "all groups".
    pub group_filter: Option<SourceGroup>,
    /// Position inside the sidebar list, index 0 being "All".
    pub sidebar_index: usize,
    /// Position inside the filtered component list.
    pub selected: usize,
    /// Applied search text. Empty means "no search".
    pub query: String,
    /// True while the search field is capturing keystrokes.
    pub searching: bool,
    pub quit: bool,
    pub status_line: String,
}

impl UiState {
    pub fn new(system: SystemInfo, read_only: bool, notices: Vec<String>) -> Self {
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
            group_filter: None,
            sidebar_index: 0,
            selected: 0,
            query: String::new(),
            searching: false,
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
    /// A search reorders by score; without a search the inventory order stands.
    pub fn visible(&self) -> Vec<&Component> {
        let query = self.query.trim();
        if query.is_empty() {
            return self
                .snapshot
                .components
                .iter()
                .filter(|component| {
                    self.group_filter.is_none_or(|group| component.source.group() == group)
                })
                .collect();
        }

        let mut matched: Vec<(i32, &Component)> = self
            .snapshot
            .components
            .iter()
            .filter(|component| {
                self.group_filter.is_none_or(|group| component.source.group() == group)
            })
            .filter_map(|component| {
                search::score_component(query, component).map(|(score, _)| (score, component))
            })
            .collect();
        matched.sort_by(|left, right| {
            right.0.cmp(&left.0).then_with(|| left.1.sort_key().cmp(&right.1.sort_key()))
        });
        matched.into_iter().map(|(_, component)| component).collect()
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
            Message::Failed(message) => {
                self.loading = false;
                self.status_line = message;
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
        self.group_filter = self.sidebar_groups().get(self.sidebar_index).copied().flatten();
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

    fn on_key(&mut self, key: KeyEvent) {
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
            KeyCode::Char('i') => self.screen = Screen::Inventory,
            KeyCode::Char('a') => self.open(Screen::Adapters),
            KeyCode::Tab | KeyCode::BackTab => {
                if self.screen == Screen::Inventory {
                    self.toggle_focus();
                }
            }
            KeyCode::Enter => {
                if self.screen == Screen::Inventory {
                    match self.focus {
                        Focus::Sidebar => self.focus = Focus::List,
                        Focus::List if self.selected_component().is_some() => {
                            self.open(Screen::Details)
                        }
                        Focus::List => {}
                    }
                } else if self.screen == Screen::Details {
                    self.back();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => match (self.screen, self.focus) {
                (Screen::Inventory, Focus::Sidebar) => self.move_sidebar(-1),
                (Screen::Inventory, Focus::List) => self.move_selection(-1),
                _ => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match (self.screen, self.focus) {
                (Screen::Inventory, Focus::Sidebar) => self.move_sidebar(1),
                (Screen::Inventory, Focus::List) => self.move_selection(1),
                _ => {}
            },
            KeyCode::Right | KeyCode::Char('l') => {
                if self.screen == Screen::Inventory && self.focus == Focus::Sidebar {
                    self.focus = Focus::List;
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if self.screen == Screen::Inventory && self.focus == Focus::List {
                    self.focus = Focus::Sidebar;
                }
            }
            _ => {}
        }
    }
}

enum Message {
    Loaded(Box<InventorySnapshot>),
    Failed(String),
}

pub struct Bootstrap {
    pub registry: Arc<AdapterRegistry>,
    pub system: SystemInfo,
    pub read_only: bool,
    pub notices: Vec<String>,
}

pub fn run(bootstrap: Bootstrap) -> anyhow::Result<()> {
    let mut session = TerminalSession::enter().context("cannot take control of the terminal")?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal =
        Terminal::new(backend).context("cannot initialise the terminal renderer")?;
    terminal.clear().ok();

    let cancel = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = mpsc::channel();
    spawn_loader(Arc::clone(&bootstrap.registry), sender, Arc::clone(&cancel));

    let mut state = UiState::new(bootstrap.system, bootstrap.read_only, bootstrap.notices);
    let outcome = event_loop(&mut terminal, &mut state, &receiver);
    cancel.store(true, Ordering::Relaxed);

    drop(terminal);
    session.suspend().ok();
    outcome
}

fn spawn_loader(
    registry: Arc<AdapterRegistry>,
    sender: mpsc::Sender<Message>,
    _cancel: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        let snapshot = registry.load_all();
        let _ = sender.send(Message::Loaded(Box::new(snapshot)));
    });
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut UiState,
    receiver: &Receiver<Message>,
) -> anyhow::Result<()> {
    loop {
        loop {
            match receiver.try_recv() {
                Ok(message) => state.apply(message),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }

        terminal.draw(|frame| ui::draw(frame, state))?;

        if state.quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(120)).context("cannot poll terminal events")? {
            match event::read().context("cannot read a terminal event")? {
                TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => state.on_key(key),
                TerminalEvent::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}
