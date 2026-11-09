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
use crate::domain::Quarantined;
use crate::domain::{Component, ComponentId, DetailField, OperationKind, SourceGroup};
use crate::infrastructure::history::History;
use crate::infrastructure::reclaim::Reclaimer;
use crate::infrastructure::system::SystemInfo;
use crate::term::TerminalSession;
use crate::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Inventory,
    Details,
    Plan,
    Cleanup,
    CleanupConfirm,
    CleanupResult,
    Quarantine,
    Result,
    History,
    Themes,
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
    pub reclaimer: &'a dyn Reclaimer,
}

/// What a cleanup confirmation would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupAction {
    /// Move leftovers into quarantine.
    Quarantine,
    /// Delete quarantine entries for good.
    Purge,
}

/// What a finished cleanup did. Rendered as a modal with the numbers, because a
/// one-line status is easy to miss and this action moves real files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    pub success: bool,
    pub headline: String,
    pub lines: Vec<String>,
}

/// A cleanup waiting for its phrase.
pub struct PendingCleanup {
    pub action: CleanupAction,
    pub plans: Vec<PendingOperation>,
    pub entries: Vec<Quarantined>,
    pub total: u64,
    pub phrase: String,
}

pub struct UiState {
    pub system: SystemInfo,
    pub read_only: bool,
    pub notices: Vec<String>,
    /// The active colour scheme. Starts from the configuration and changes when the theme
    /// selector applies one.
    pub theme: crate::ui::theme::Theme,
    pub themes: crate::ui::theme::ThemeCatalogue,
    pub theme_index: usize,
    pub preferences_path: Option<std::path::PathBuf>,
    /// Where a user drops their own theme files; rescanned when the selector opens.
    pub theme_dir: Option<std::path::PathBuf>,
    /// The theme that was active when the selector opened, so Esc can put it back.
    pub theme_before: Option<crate::ui::theme::Theme>,
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
    /// Selected leftovers in the cleanup view.
    pub cleanup_selected: std::collections::BTreeSet<crate::domain::ComponentId>,
    pub cleanup_index: usize,
    pub quarantine: Vec<Quarantined>,
    pub quarantine_index: usize,
    pub pending_cleanup: Option<PendingCleanup>,
    pub cleanup_input: String,
    /// Bytes per leftover, measured when the view opens. The draw loop never walks the
    /// filesystem: it reads this map.
    pub reclaim_sizes: std::collections::BTreeMap<crate::domain::ComponentId, u64>,
    /// What the last cleanup did, shown as a modal so it cannot be missed.
    pub cleanup_report: Option<CleanupReport>,
    pub plan_input: String,
    pub last_record: Option<OperationRecord>,
    /// Extra facts for the open detail view, with the component they belong to.
    pub enrichment: Option<(ComponentId, Vec<DetailField>)>,
    /// A lookup the loop must run off the draw path.
    pub enrich_requested: Option<Component>,
    pub history: History,
    pub history_entries: Vec<OperationRecord>,
    pub history_selected: usize,
    /// One line about a newer release, when the optional check found one.
    pub update_hint: Option<String>,
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
            theme: crate::ui::theme::default_theme(),
            themes: crate::ui::theme::ThemeCatalogue::default(),
            theme_index: 0,
            preferences_path: None,
            theme_dir: None,
            theme_before: None,
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
            cleanup_selected: std::collections::BTreeSet::new(),
            cleanup_index: 0,
            quarantine: Vec::new(),
            quarantine_index: 0,
            pending_cleanup: None,
            cleanup_input: String::new(),
            reclaim_sizes: std::collections::BTreeMap::new(),
            cleanup_report: None,
            plan_input: String::new(),
            last_record: None,
            enrichment: None,
            enrich_requested: None,
            history,
            history_entries: Vec::new(),
            history_selected: 0,
            update_hint: None,
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
                self.measure_leftovers();
                if self.snapshot.components.is_empty() && self.snapshot.failure_count() > 0 {
                    self.status_line =
                        "no component could be loaded: open the adapters view (a)".to_string();
                }
            }
            Message::UpdateAvailable(hint) => self.update_hint = Some(hint),
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

    /// Leftovers this session could move into quarantine.
    pub fn reclaimable(&self) -> Vec<&Component> {
        self.snapshot
            .components
            .iter()
            .filter(|component| component.supports(crate::domain::ComponentCapability::Reclaim))
            .collect()
    }

    pub fn size_of(&self, component: &Component) -> u64 {
        self.reclaim_sizes.get(&component.id).copied().unwrap_or(0)
    }

    /// Measures every leftover once. Called when the cleanup view opens and after a
    /// reload, never from the render path.
    pub fn measure_leftovers(&mut self) {
        self.reclaim_sizes.clear();
        for component in self.snapshot.components.iter() {
            if !component.supports(crate::domain::ComponentCapability::Reclaim) {
                continue;
            }
            let size = component
                .location
                .as_ref()
                .map(|path| crate::domain::path_size(path))
                .unwrap_or(0);
            self.reclaim_sizes.insert(component.id.clone(), size);
        }
    }

    pub fn reclaimable_total(&self) -> u64 {
        self.reclaimable()
            .iter()
            .map(|component| self.size_of(component))
            .sum()
    }

    pub fn selected_reclaim_size(&self) -> u64 {
        self.reclaimable()
            .iter()
            .filter(|component| self.cleanup_selected.contains(&component.id))
            .map(|component| self.size_of(component))
            .sum()
    }

    fn refresh_quarantine(&mut self, reclaimer: &dyn Reclaimer) {
        match reclaimer.entries() {
            Ok(entries) => {
                self.quarantine = entries;
                self.quarantine_index = self
                    .quarantine_index
                    .min(self.quarantine.len().saturating_sub(1));
            }
            Err(error) => {
                self.quarantine.clear();
                self.status_line = format!("cannot read the quarantine area: {error}");
            }
        }
    }

    /// Builds the confirmation for the selected leftovers.
    fn prepare_cleanup(&mut self, registry: &AdapterRegistry) {
        let mut plans = Vec::new();
        let mut problems = Vec::new();
        for component in self.reclaimable() {
            if !self.cleanup_selected.contains(&component.id) {
                continue;
            }
            match operations::prepare(registry, component, OperationKind::Reclaim, self.read_only) {
                Ok(pending) => plans.push(pending),
                Err(error) => problems.push(format!("{}: {error}", component.name)),
            }
        }

        if plans.is_empty() {
            self.status_line = if problems.is_empty() {
                "select at least one leftover to move".to_string()
            } else {
                problems.join("; ")
            };
            return;
        }
        if !problems.is_empty() {
            // A partial batch is never executed silently: the refusal is shown and the
            // batch is not built.
            self.status_line = format!("nothing was prepared: {}", problems.join("; "));
            return;
        }

        let total: u64 = plans.iter().map(|plan| plan.plan.reclaim_size()).sum();
        self.cleanup_input.clear();
        self.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Quarantine,
            phrase: format!("quarantine {}", plans.len()),
            total,
            plans,
            entries: Vec::new(),
        });
        self.open(Screen::CleanupConfirm);
    }

    fn prepare_purge(&mut self) {
        let Some(entry) = self.quarantine.get(self.quarantine_index).cloned() else {
            self.status_line = "nothing in quarantine to purge".to_string();
            return;
        };
        self.cleanup_input.clear();
        self.pending_cleanup = Some(PendingCleanup {
            action: CleanupAction::Purge,
            phrase: format!(
                "purge {}",
                entry
                    .original
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.original.display().to_string())
            ),
            total: entry.size,
            plans: Vec::new(),
            entries: vec![entry],
        });
        self.open(Screen::CleanupConfirm);
    }

    /// Runs the confirmed cleanup. Every plan is re-verified immediately before it runs.
    fn run_cleanup(&mut self, actions: &mut Actions<'_>) {
        let Some(pending) = self.pending_cleanup.take() else {
            return;
        };
        if pending.phrase != self.cleanup_input.trim() {
            self.status_line = format!(
                "type exactly '{}' to confirm, or Esc to cancel",
                pending.phrase
            );
            self.pending_cleanup = Some(pending);
            return;
        }

        let mut done = 0usize;
        let mut bytes = 0u64;
        let mut failure: Option<String> = None;

        match pending.action {
            CleanupAction::Quarantine => {
                for plan in &pending.plans {
                    match operations::execute_verified(actions.registry, plan, self.read_only) {
                        Ok(outcome) if outcome.success => {
                            done += 1;
                            bytes += plan.plan.reclaim_size();
                        }
                        Ok(outcome) => {
                            failure = Some(outcome.summary);
                            break;
                        }
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
            }
            CleanupAction::Purge => {
                for entry in &pending.entries {
                    match actions.reclaimer.purge(entry) {
                        Ok(()) => {
                            done += 1;
                            bytes += entry.size;
                        }
                        Err(error) => {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
            }
        }

        self.status_line = match failure {
            None => format!(
                "{} {} item(s) ({})",
                if pending.action == CleanupAction::Quarantine {
                    "moved into quarantine:"
                } else {
                    "purged:"
                },
                done,
                crate::domain::format_size(bytes)
            ),
            Some(error) => format!(
                "stopped after {done} item(s): {error}{}",
                if pending.action == CleanupAction::Quarantine {
                    " — already moved items stay recoverable in quarantine"
                } else {
                    ""
                }
            ),
        };

        self.cleanup_selected.clear();
        self.cleanup_input.clear();
        self.pending_cleanup = None;
        self.refresh_quarantine(actions.reclaimer);
        self.screen = Screen::Inventory;
        self.reload_requested = true;
    }

    fn restore_quarantine(&mut self, actions: &Actions<'_>) {
        let Some(entry) = self.quarantine.get(self.quarantine_index).cloned() else {
            self.status_line = "nothing in quarantine to restore".to_string();
            return;
        };
        match actions.reclaimer.restore(&entry) {
            Ok(()) => {
                self.status_line = format!("restored {}", entry.original.display());
                self.refresh_quarantine(actions.reclaimer);
            }
            Err(error) => self.status_line = error.to_string(),
        }
    }

    pub fn active_theme_index(&self) -> usize {
        self.themes
            .themes
            .iter()
            .position(|theme| theme.id == self.theme.id)
            .unwrap_or(0)
    }

    /// Opens the selector, rescanning the user's directory so a theme dropped in while the
    /// program runs appears without a restart.
    fn open_themes(&mut self) {
        if let Some(directory) = self.theme_dir.clone() {
            let parent = directory.parent().map(|path| path.to_path_buf());
            if let Some(parent) = parent {
                self.themes = crate::ui::theme::catalogue(&parent);
            }
        }
        self.theme_before = Some(self.theme.clone());
        self.theme_index = self.active_theme_index();
        self.open(Screen::Themes);
    }

    /// Moving through the list previews the theme: the list is the preview.
    fn preview_theme(&mut self, delta: isize) {
        if self.themes.themes.is_empty() {
            return;
        }
        let count = self.themes.themes.len() as isize;
        let next = (self.theme_index as isize + delta).rem_euclid(count);
        self.theme_index = next as usize;
        if let Some(theme) = self.themes.themes.get(self.theme_index) {
            self.theme = theme.clone();
        }
    }

    fn confirm_theme(&mut self) {
        let Some(theme) = self.themes.themes.get(self.theme_index).cloned() else {
            return;
        };
        self.theme = theme.clone();
        self.theme_before = None;
        self.screen = Screen::Inventory;

        // The choice is remembered in the interface's own file, never by rewriting the
        // user's config.toml (which carries their comments).
        let Some(path) = self.preferences_path.clone() else {
            self.status_line = format!(
                "theme '{}' applied for this session; it could not be saved",
                theme.name
            );
            return;
        };
        let preferences = crate::infrastructure::preferences::Preferences {
            theme: Some(theme.id.clone()),
        };
        self.status_line = match preferences.save(&path) {
            Ok(()) => format!("theme '{}' applied and remembered", theme.name),
            Err(error) => format!(
                "theme '{}' applied, but it could not be saved: {error}",
                theme.name
            ),
        };
    }

    /// Seeds a new theme file from the highlighted palette, so "create your own" starts from
    /// something that already renders. The file is never overwritten and appears in the list
    /// immediately, because the catalogue is rescanned.
    fn create_theme(&mut self) {
        let Some(base) = self.themes.themes.get(self.theme_index).cloned() else {
            self.status_line = "no theme selected".to_string();
            return;
        };
        let Some(directory) = self.theme_dir.clone() else {
            self.status_line = "the theme directory is not known in this session".to_string();
            return;
        };

        match crate::ui::theme::starter_file(&directory, &base) {
            Ok(path) => {
                if let Some(parent) = directory.parent() {
                    self.themes = crate::ui::theme::catalogue(parent);
                }
                let id = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.theme_index = self
                    .themes
                    .themes
                    .iter()
                    .position(|theme| theme.id == id)
                    .unwrap_or(self.theme_index);
                if let Some(theme) = self.themes.themes.get(self.theme_index) {
                    self.theme = theme.clone();
                }
                self.status_line = format!(
                    "created {} — edit it, save, and press t again",
                    path.display()
                );
            }
            Err(error) => {
                self.status_line = format!("could not create a theme file: {error}");
            }
        }
    }

    fn cancel_themes(&mut self) {
        if let Some(previous) = self.theme_before.take() {
            self.theme = previous;
        }
        self.back();
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
            OperationKind::Reclaim => crate::domain::ComponentCapability::Reclaim,
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

        if self.screen == Screen::CleanupResult {
            match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char(' ') => {
                    self.cleanup_report = None;
                    self.screen = Screen::Inventory;
                }
                _ => {}
            }
            return;
        }

        // A cleanup confirmation owns every printable key, for the same reason.
        if self.screen == Screen::CleanupConfirm {
            match key.code {
                KeyCode::Esc => {
                    self.pending_cleanup = None;
                    self.cleanup_input.clear();
                    self.status_line = "cancelled: nothing was moved or purged".to_string();
                    self.back();
                }
                KeyCode::Enter => self.run_cleanup(actions),
                KeyCode::Backspace => {
                    self.cleanup_input.pop();
                }
                KeyCode::Char(character) if !character.is_control() => {
                    self.cleanup_input.push(character);
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
                if self.screen == Screen::Themes {
                    self.cancel_themes();
                } else if self.screen == Screen::Inventory {
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
            KeyCode::Char('C') => {
                if self.screen != Screen::Cleanup {
                    self.cleanup_selected.clear();
                    self.cleanup_index = 0;
                    self.measure_leftovers();
                    self.open(Screen::Cleanup);
                }
            }
            KeyCode::Char('t') if self.screen != Screen::Themes => self.open_themes(),
            KeyCode::Char('V') => {
                self.refresh_quarantine(actions.reclaimer);
                self.open(Screen::Quarantine);
            }
            KeyCode::Char(' ') if self.screen == Screen::Cleanup => {
                if let Some(component) = self.reclaimable().get(self.cleanup_index) {
                    let id = component.id.clone();
                    if !self.cleanup_selected.insert(id.clone()) {
                        self.cleanup_selected.remove(&id);
                    }
                }
            }
            KeyCode::Char('A') if self.screen == Screen::Cleanup => {
                self.cleanup_selected = self
                    .reclaimable()
                    .iter()
                    .map(|component| component.id.clone())
                    .collect();
            }
            KeyCode::Char('N') if self.screen == Screen::Cleanup => {
                self.cleanup_selected.clear();
            }
            KeyCode::Char('r') if self.screen == Screen::Quarantine => {
                self.restore_quarantine(actions);
            }
            KeyCode::Char('p') if self.screen == Screen::Quarantine => self.prepare_purge(),
            KeyCode::Char('c') if self.screen == Screen::Themes => self.create_theme(),
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
                Screen::Cleanup if !self.cleanup_selected.is_empty() => {
                    self.prepare_cleanup(actions.registry);
                }
                Screen::Cleanup => {
                    self.status_line = "select a leftover with space first".to_string();
                }
                Screen::Themes => self.confirm_theme(),
                Screen::Result | Screen::Details | Screen::History | Screen::Quarantine => {
                    self.back();
                }
                _ => {}
            },
            KeyCode::Up | KeyCode::Char('k') => match (self.screen, self.focus) {
                (Screen::Themes, _) => self.preview_theme(-1),
                (Screen::Cleanup, _) => {
                    self.cleanup_index = self.cleanup_index.saturating_sub(1);
                }
                (Screen::Quarantine, _) => {
                    self.quarantine_index = self.quarantine_index.saturating_sub(1);
                }
                (Screen::Inventory, Focus::Sidebar) => self.move_sidebar(-1),
                (Screen::Inventory, Focus::List) => self.move_selection(-1),
                (Screen::History, _) => {
                    self.history_selected = self.history_selected.saturating_sub(1);
                }
                _ => {}
            },
            KeyCode::Down | KeyCode::Char('j') => match (self.screen, self.focus) {
                (Screen::Themes, _) => self.preview_theme(1),
                (Screen::Cleanup, _) => {
                    if self.cleanup_index + 1 < self.reclaimable().len() {
                        self.cleanup_index += 1;
                    }
                }
                (Screen::Quarantine, _) => {
                    if self.quarantine_index + 1 < self.quarantine.len() {
                        self.quarantine_index += 1;
                    }
                }
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
    UpdateAvailable(String),
    Enriched {
        id: ComponentId,
        fields: Vec<DetailField>,
    },
}

pub struct Bootstrap {
    pub registry: Arc<AdapterRegistry>,
    /// Used for the optional update check, which is the only thing in this program that
    /// touches the network.
    pub runner: Arc<dyn crate::infrastructure::process::CommandRunner>,
    pub state_dir: std::path::PathBuf,
    /// Off unless the configuration asks for it.
    pub check_for_updates: bool,
    pub theme: crate::ui::theme::Theme,
    pub themes: crate::ui::theme::ThemeCatalogue,
    pub theme_dir: std::path::PathBuf,
    pub preferences_path: std::path::PathBuf,
    pub reclaimer: Arc<dyn crate::infrastructure::reclaim::Reclaimer>,
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
    if bootstrap.check_for_updates {
        spawn_update_check(
            Arc::clone(&bootstrap.runner),
            bootstrap.state_dir.clone(),
            sender.clone(),
        );
    }

    let mut state = UiState::new(
        bootstrap.system,
        bootstrap.read_only,
        bootstrap.notices,
        bootstrap.history,
    );
    state.theme = bootstrap.theme;
    state.themes = bootstrap.themes;
    state.theme_dir = Some(bootstrap.theme_dir);
    state.preferences_path = Some(bootstrap.preferences_path);
    state.theme_index = state.active_theme_index();
    let registry = Arc::clone(&bootstrap.registry);
    let reclaimer = Arc::clone(&bootstrap.reclaimer);
    let outcome = event_loop(
        &mut terminal,
        &mut state,
        &receiver,
        &registry,
        &mut session,
        reclaimer.as_ref(),
        &sender,
    );
    drop(terminal);
    session.suspend().ok();
    outcome
}

/// The only network access in the program, and it is opt-in: one read of the release API,
/// in the background, which never blocks the interface and never installs anything.
fn spawn_update_check(
    runner: Arc<dyn crate::infrastructure::process::CommandRunner>,
    state_dir: std::path::PathBuf,
    sender: mpsc::Sender<Message>,
) {
    std::thread::spawn(move || {
        let Some(current) =
            crate::infrastructure::update::Version::parse(env!("CARGO_PKG_VERSION"))
        else {
            return;
        };
        let updater =
            crate::infrastructure::update::Updater::new(runner.as_ref(), state_dir.join("update"));
        match updater.check(&current) {
            Ok(outcome) if outcome.is_newer() => {
                let _ = sender.send(Message::UpdateAvailable(outcome.summary()));
            }
            Ok(_) => {}
            Err(error) => {
                // A failed check is not the user's problem: it must not become noise in the
                // interface. It goes to the log and nowhere else.
                tracing::info!(%error, "update check failed");
            }
        }
    });
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
    reclaimer: &dyn Reclaimer,
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
                        reclaimer,
                    };
                    state.on_key(key, &mut actions);
                }
                TerminalEvent::Resize(_, _) => state.needs_full_redraw = true,
                _ => {}
            }
        }
    }
}
