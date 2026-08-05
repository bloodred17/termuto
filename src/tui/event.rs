use super::{app::App, ui};
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io::Stdout, time::Duration};

pub(crate) async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        // Stills are fetched in the background, so each frame picks up whichever
        // ones landed while the last one was on screen.
        app.collect_previews();
        terminal.draw(|frame| ui::render(frame, app))?;

        // A queued action runs only after the loading frame above is on screen,
        // so a slow API call shows progress instead of a frozen UI.
        if let Some(action) = app.take_pending() {
            app.run_action(action).await?;
            continue;
        }

        if event::poll(Duration::from_millis(250)).context("Could not poll terminal events")?
            && let Event::Key(key) = event::read().context("Could not read a terminal event")?
            && key.kind == KeyEventKind::Press
            && !app.handle_key(key)
        {
            return Ok(());
        }
    }
}
