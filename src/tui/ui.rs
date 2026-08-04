use super::app::{App, Screen};
use crate::catalog::Anime;
use crate::cli::release_date;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

pub(crate) fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("termuto — local anime catalog proof of concept")
            .block(Block::default().borders(Borders::ALL).title(" termuto "))
            .alignment(Alignment::Center),
        chunks[0],
    );

    match app.display_screen() {
        Screen::Home => render_home(frame, app, chunks[1]),
        Screen::Latest => render_anime_list(frame, app, chunks[1], "Latest releases"),
        Screen::Ongoing => render_anime_list(frame, app, chunks[1], "Ongoing"),
        Screen::Search => render_search(frame, app, chunks[1]),
        Screen::Episodes => render_episodes(frame, app, chunks[1]),
        Screen::MovieDetail => render_movie(frame, app, chunks[1]),
        Screen::PlaybackNotice => render_playback_notice(frame, chunks[1]),
        Screen::QuitConfirm => unreachable!("the quit prompt is drawn as an overlay"),
    }

    if app.screen == Screen::QuitConfirm {
        render_quit_confirm(frame, chunks[1]);
    }

    frame.render_widget(
        Paragraph::new(help_text(app.screen)).alignment(Alignment::Center),
        chunks[2],
    );
}

fn render_home(frame: &mut Frame, app: &App, area: Rect) {
    let items = ["Latest releases", "Ongoing", "Search", "Quit"]
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    render_list(frame, area, "Home", items, app.home_index);
}

fn render_anime_list(frame: &mut Frame, app: &App, area: Rect, title: &str) {
    let items = app
        .current_items()
        .iter()
        .map(anime_row)
        .collect::<Vec<_>>();
    render_list(frame, area, title, items, app.list_index);
}

fn render_search(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!("Search: {}", app.search_input)).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Search (typing) "),
        ),
        chunks[0],
    );
    let items = if app.search_results.is_empty() {
        vec![ListItem::new("No matching titles yet.")]
    } else {
        app.search_results.iter().map(anime_row).collect()
    };
    render_list(frame, chunks[1], "Results", items, app.list_index);
}

fn render_episodes(frame: &mut Frame, app: &App, area: Rect) {
    let Some(anime) = app.selected_anime.as_ref() else {
        return;
    };
    let items = anime
        .episodes
        .iter()
        .map(|episode| {
            let released = episode
                .released_at
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "—".to_string());
            ListItem::new(format!(
                "{:>3}  {:<36}  {}",
                episode.number, episode.title, released
            ))
        })
        .collect::<Vec<_>>();
    render_list(
        frame,
        area,
        &format!("{} — episodes", anime.title),
        items,
        app.episode_index,
    );
}

fn render_movie(frame: &mut Frame, app: &App, area: Rect) {
    let Some(anime) = app.selected_anime.as_ref() else {
        return;
    };
    let text = vec![
        Line::from(Span::styled(
            &anime.title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Status: {}", anime.status)),
        Line::from(format!("Released: {}", release_date(anime))),
        Line::from(""),
        Line::from(anime.description.clone()),
        Line::from(""),
        Line::from("Press Enter for the future play action."),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Movie details "),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_playback_notice(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(70, 25, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new("Playback is not implemented in this proof of concept.\n\nPress Enter or Esc to return.")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title(" Playback "))
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_quit_confirm(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 25, area);
    frame.render_widget(Clear, popup);
    let text = vec![
        Line::from(Span::styled(
            "Are you sure you want to quit?",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("y — yes, quit    n — no, stay"),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Quit ")
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_list(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<ListItem<'_>>,
    selected: usize,
) {
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} ")),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn anime_row(anime: &Anime) -> ListItem<'static> {
    ListItem::new(format!(
        "{:<38}  {:<10}  {:<11}  {}",
        anime.title,
        anime.kind,
        anime.status,
        release_date(anime)
    ))
}

fn help_text(screen: Screen) -> &'static str {
    match screen {
        Screen::Home => "l Latest • o Ongoing • / Search • Enter Select • q Quit",
        Screen::Search => "Type to search • ↑/↓ select • Enter open • Esc back • Ctrl-C quit",
        Screen::Episodes | Screen::MovieDetail => {
            "↑/↓ or j/k select • Enter play action • Esc back"
        }
        Screen::PlaybackNotice => "Enter or Esc back",
        Screen::QuitConfirm => "y Quit • n Stay • Esc Stay",
        _ => "↑/↓ or j/k select • Enter open • / Search • Esc back • q Quit",
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
