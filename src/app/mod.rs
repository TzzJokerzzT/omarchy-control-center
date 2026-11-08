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

use crate::application::{AdapterRegistry, InventorySnapshot};
use crate::infrastructure::system::SystemInfo;
use crate::term::TerminalSession;
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Inventory,
    Adapters,
    Help,
}

pub struct UiState {
    pub system: SystemInfo,
    pub read_only: bool,
    pub notices: Vec<String>,
    pub snapshot: InventorySnapshot,
    pub screen: Screen,
    pub previous_screen: Screen,
    pub loading: bool,
    pub selected: usize,
    pub quit: bool,
    pub status_line: String,
}

impl UiState {
    pub fn new(system: SystemInfo, read_only: bool, notices: Vec<String>) -> Self {
        let mut status_line = String::new();
        if read_only {
            status_line = "read-only mode: destructive actions are disabled".to_string();
        }
        Self {
            system,
            read_only,
            notices,
            snapshot: InventorySnapshot::default(),
            screen: Screen::Inventory,
            previous_screen: Screen::Inventory,
            loading: true,
            selected: 0,
            quit: false,
            status_line,
        }
    }

    fn apply(&mut self, message: Message) {
        match message {
            Message::Loaded(snapshot) => {
                self.loading = false;
                self.snapshot = *snapshot;
                self.clamp_selection();
                if self.snapshot.components.is_empty() && self.snapshot.failure_count() > 0 {
                    self.status_line =
                        "no component could be loaded: see the adapters view".to_string();
                }
            }
            Message::Failed(message) => {
                self.loading = false;
                self.status_line = message;
            }
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.snapshot.components.len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    pub fn selected_component(&self) -> Option<&crate::domain::Component> {
        self.snapshot.components.get(self.selected)
    }

    fn open_help(&mut self) {
        if self.screen != Screen::Help {
            self.previous_screen = self.screen;
            self.screen = Screen::Help;
        }
    }

    fn close_help(&mut self) {
        if self.screen == Screen::Help {
            self.screen = self.previous_screen;
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => self.quit = true,
            KeyCode::Char('?') => {
                if self.screen == Screen::Help {
                    self.close_help();
                } else {
                    self.open_help();
                }
            }
            KeyCode::Esc => {
                if self.screen == Screen::Help {
                    self.close_help();
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('i') => self.screen = Screen::Inventory,
            KeyCode::Char('a') => self.screen = Screen::Adapters,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let count = self.snapshot.components.len();
                if count > 0 && self.selected + 1 < count {
                    self.selected += 1;
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
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
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
