//! Terminal lifecycle.
//!
//! Raw mode and the alternate screen are owned by a single value. It restores the
//! terminal on drop, on an explicit `suspend` (privilege handoff) and from a panic
//! hook, so a crash never leaves the shell unusable.

use std::io::{self, stdout};
use std::sync::Once;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub struct TerminalSession {
    active: bool,
}

impl TerminalSession {
    pub fn enter() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide, EnableBracketedPaste)?;
        Ok(Self { active: true })
    }

    /// Gives the terminal back to the shell so an interactive command (a sudo
    /// password prompt) can own it.
    pub fn suspend(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let _ = disable_raw_mode();
        execute!(stdout(), Show, LeaveAlternateScreen, DisableBracketedPaste)?;
        self.active = false;
        Ok(())
    }

    pub fn resume(&mut self) -> io::Result<()> {
        if self.active {
            return Ok(());
        }
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, Hide, EnableBracketedPaste)?;
        self.active = true;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.suspend();
    }
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), Show, LeaveAlternateScreen, DisableBracketedPaste);
            previous(info);
        }));
    });
}
