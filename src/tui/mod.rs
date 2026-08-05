//! Minimal interactive terminal interface built on the shared data source.

mod app;
mod event;
mod preview;
mod ui;

use crate::playback::Playback;
use crate::source::Source;
use anyhow::{Context, Result};
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, Stdout};

pub use preview::IMAGE_PROTOCOL_ENV;

pub async fn run(source: Source, playback: Playback) -> Result<()> {
    let mut terminal = TerminalGuard::new()?;
    // The terminal is asked what it can draw and how big a cell is. This writes
    // to and reads from the terminal directly, so it has to happen after the
    // alternate screen is up and before the event loop starts reading keys.
    let picker = preview::picker();
    let mut app = app::App::new(source, playback, picker);
    let event_result = event::run(&mut terminal.terminal, &mut app).await;
    let restore_result = terminal.restore();

    event_result?;
    restore_result.context("Could not restore the terminal after leaving the TUI")?;
    Ok(())
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("Could not enable terminal raw mode")?;
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                return Err(error).context("Could not initialize the terminal UI");
            }
        };
        if let Err(error) = execute!(terminal.backend_mut(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error).context("Could not enter the terminal alternate screen");
        }
        if let Err(error) = terminal.hide_cursor() {
            let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            return Err(error).context("Could not hide the terminal cursor");
        }

        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;

        // Attempt every cleanup operation even if a preceding one fails.
        let raw_result = disable_raw_mode();
        let screen_result = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let cursor_result = self.terminal.show_cursor();
        raw_result.and(screen_result).and(cursor_result)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
