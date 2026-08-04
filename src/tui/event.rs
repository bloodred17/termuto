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
        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(250)).context("Could not poll terminal events")?
            && let Event::Key(key) = event::read().context("Could not read a terminal event")?
            && key.kind == KeyEventKind::Press
            && !app.handle_key(key).await?
        {
            return Ok(());
        }
    }
}
