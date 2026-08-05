use super::app::{App, Screen, SearchFocus};
use super::preview::Preview;
use crate::catalog::Anime;
use crate::live::model::Named;
use crate::live::{LiveAnime, LiveEpisode};
use crate::source::AnimeSummary;
use crate::source::model::EMPTY;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use ratatui_image::{FilterType, Resize, StatefulImage};

pub(crate) fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(header_line(app))
            .block(Block::default().borders(Borders::ALL).title(" termuto "))
            .alignment(Alignment::Center),
        chunks[0],
    );

    match app.display_screen() {
        Screen::Home => render_home(frame, app, chunks[1]),
        Screen::Listing => render_listing(frame, app, chunks[1]),
        Screen::SeasonPicker => render_season_picker(frame, app, chunks[1]),
        Screen::Search => render_search(frame, app, chunks[1]),
        Screen::LiveDetail => render_live_detail(frame, app, chunks[1]),
        Screen::Episodes => render_episodes(frame, app, chunks[1]),
        Screen::LiveEpisodes => render_live_episodes(frame, app, chunks[1]),
        Screen::MovieDetail => render_movie(frame, app, chunks[1]),
        // Overlay screens never reach `display_screen`.
        Screen::Playing | Screen::QuitConfirm | Screen::Error => {}
    }

    match app.screen {
        Screen::Playing => render_now_playing(frame, app, chunks[1]),
        Screen::QuitConfirm => render_quit_confirm(frame, chunks[1]),
        Screen::Error => render_error(frame, app, chunks[1]),
        _ => {}
    }

    // Drawn last so it covers whatever request is in flight.
    if let Some(label) = app.loading.clone() {
        render_loading(frame, &label, chunks[1]);
    }

    frame.render_widget(
        Paragraph::new(help_text(app)).alignment(Alignment::Center),
        chunks[2],
    );
}

/// The header, carrying the settings that change what a play actually does.
/// `auto` is highlighted rather than written into the sentence: it is a state
/// that toggles under the user, not a label, and the colour is what makes a
/// glance enough to read it.
fn header_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::raw(format!(
        "termuto — anime from the terminal · mode: {} · player: {} · provider: {}",
        app.mode(),
        app.player_name(),
        app.provider_name()
    ))];
    if app.autoswitch() {
        spans.push(Span::raw(" · "));
        spans.push(Span::styled(
            "auto",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn render_home(frame: &mut Frame, app: &App, area: Rect) {
    let items = app
        .home_labels()
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    render_list(frame, area, "Home", items, app.home_index);
}

fn render_listing(frame: &mut Frame, app: &App, area: Rect) {
    let title = listing_title(&app.listing_title, &app.listing);
    render_list(
        frame,
        area,
        &title,
        summary_items(&app.listing),
        app.list_index,
    );
}

/// The column header rides along in the block title, so it stays put while the
/// rows scroll. Title-only listings have no columns to describe.
fn listing_title(heading: &str, rows: &[AnimeSummary]) -> String {
    if rows.is_empty() || rows.iter().all(AnimeSummary::is_bare) {
        heading.to_string()
    } else {
        format!("{heading} — {}", AnimeSummary::header())
    }
}

fn render_season_picker(frame: &mut Frame, app: &App, area: Rect) {
    let items = app
        .seasons
        .iter()
        .map(|season| ListItem::new(season.label()))
        .collect::<Vec<_>>();
    render_list(frame, area, "Seasons", items, app.season_index);
}

fn render_search(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let editing = app.search_focus == SearchFocus::Query;
    let cursor = if editing { "▏" } else { "" };
    frame.render_widget(
        Paragraph::new(format!("{}{cursor}", app.search_input)).block(
            Block::default()
                .borders(Borders::ALL)
                .title(if editing {
                    " Search — type a query, then Enter "
                } else {
                    " Search "
                })
                .border_style(if editing {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                }),
        ),
        chunks[0],
    );

    let items = if app.listing.is_empty() {
        let message = match app.search_submitted.as_deref() {
            Some(query) => format!("No titles found for \"{query}\"."),
            None => "Type a query and press Enter.".to_string(),
        };
        vec![ListItem::new(message)]
    } else {
        summary_items(&app.listing)
    };

    let title = listing_title("Results", &app.listing);
    if editing {
        // Nothing is selected while the query has focus.
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} ")),
            ),
            chunks[1],
        );
    } else {
        render_list(frame, chunks[1], &title, items, app.list_index);
    }
}

fn render_episodes(frame: &mut Frame, app: &App, area: Rect) {
    let Some(anime) = app.cached_detail() else {
        return;
    };
    let items = anime
        .episodes
        .iter()
        .map(|episode| {
            let released = episode
                .released_at
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| EMPTY.to_string());
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

/// The episode picker: the list on the left, and the selected episode's still
/// and synopsis stacked in a column beside it.
///
/// Rows come from `/anime/{id}/episodes` where it reached them, and fall back to
/// bare numbering for the rest, so every episode the title reports stays
/// selectable even when the list is short.
fn render_live_episodes(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(title) = app
        .live_detail()
        .map(|anime| format!("{} — episodes", anime.display_title()))
    else {
        return;
    };
    let (list_area, side) = split_episode_area(app, area);

    let width = list_area.width.saturating_sub(4) as usize;
    let items = (0..app.live_episode_count())
        .map(|index| match app.episode(index) {
            Some(episode) => ListItem::new(episode_row(episode, width)),
            None => ListItem::new(format!("{:>3}  Episode {}", index + 1, index + 1)),
        })
        .collect::<Vec<_>>();
    render_list(frame, list_area, &title, items, app.episode_index);

    if let Some(side) = side {
        render_episode_side(frame, app, side);
    }
}

/// The side column appears only when both a pane is wanted and there is room
/// for one; on a narrow terminal the list keeps the whole width rather than
/// both halves being squeezed into uselessness.
fn split_episode_area(app: &App, area: Rect) -> (Rect, Option<Rect>) {
    if !(app.show_preview || app.show_synopsis) || area.width < MIN_SPLIT_WIDTH {
        return (area, None);
    }
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    (columns[0], Some(columns[1]))
}

/// Below this the two columns are both too narrow to read.
const MIN_SPLIT_WIDTH: u16 = 76;

fn render_episode_side(frame: &mut Frame, app: &mut App, area: Rect) {
    let rows = match (app.show_preview, app.show_synopsis) {
        (true, true) => Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Min(4)])
            .split(area)
            .to_vec(),
        _ => vec![area],
    };

    let mut next = rows.iter();
    if app.show_preview
        && let Some(row) = next.next()
    {
        render_episode_preview(frame, app, *row);
    }
    if app.show_synopsis
        && let Some(row) = next.next()
    {
        render_episode_synopsis(frame, app, *row);
    }
}

fn render_episode_preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Preview · {} ", app.preview_protocol()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Read the state before drawing: the still needs the app back mutably, and
    // holding a borrow across the branch would keep it.
    let note = match app.selected_preview() {
        Some(Preview::Ready(_)) => None,
        Some(Preview::Pending) => Some("Loading image…"),
        Some(Preview::Missing) => Some("Image unavailable"),
        None => Some("No image for this episode"),
    };

    match note {
        Some(note) => frame.render_widget(
            Paragraph::new(centered_note(note, inner.height)).alignment(Alignment::Center),
            inner,
        ),
        None => {
            if let Some(Preview::Ready(protocol)) = app.selected_preview_mut() {
                // Lanczos only costs anything when the pane is resized, and it
                // is the difference between a sharp still and a soft one on a
                // terminal that draws real pixels.
                frame.render_stateful_widget(
                    StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3))),
                    inner,
                    protocol.as_mut(),
                );
            }
        }
    }
}

/// A one-line placeholder, sat where the middle of the image would be.
fn centered_note(text: &str, height: u16) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(""); (height as usize).saturating_sub(1) / 2];
    lines.push(Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

fn render_episode_synopsis(frame: &mut Frame, app: &App, area: Rect) {
    let episode = app.selected_episode();
    let lines = match episode.and_then(|episode| non_empty(episode.synopsis.as_deref())) {
        // Split on the synopsis' own newlines rather than handing the whole
        // thing over as one line: `Wrap` rewraps each line it is given, so a
        // single line would run the paragraphs and the source note together.
        Some(synopsis) => synopsis
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect(),
        None => vec![Line::from(Span::styled(
            "No synopsis for this episode.",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    // The runtime rides in the title: it is one short fact, and the pane is for
    // prose.
    let title = match episode.and_then(LiveEpisode::duration_label) {
        Some(duration) => format!(" Synopsis · {duration} "),
        None => " Synopsis ".to_string(),
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// `  1  The Journey's End          2023-09-29  4.29`, with the title taking
/// whatever the fixed columns leave.
fn episode_row(episode: &LiveEpisode, width: usize) -> Line<'static> {
    let aired = episode.aired_label().unwrap_or_else(|| EMPTY.to_string());
    let score = episode.score_label();

    // Number, gaps, date, and score are fixed; the title flexes.
    let fixed = 3 + 2 + 2 + 10 + 2 + 4;
    let title_width = width.saturating_sub(fixed).max(8);
    let title = pad(
        &truncate_to(&episode.display_title(), title_width),
        title_width,
    );

    Line::from(vec![
        Span::styled(
            format!("{:>3}  ", episode.mal_id),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(title),
        Span::styled(
            format!("  {aired:>10}  "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{:>4}", score.clone().unwrap_or_else(|| EMPTY.to_string())),
            Style::default().fg(score_color(episode.score)),
        ),
    ])
}

/// Episode scores are out of 5 and cluster high, so the bands are set where a
/// glance down the column separates the standouts from the rest.
fn score_color(score: Option<f64>) -> Color {
    match score {
        Some(score) if score >= 4.25 => Color::Green,
        Some(score) if score >= 3.75 => Color::Yellow,
        Some(_) => Color::Gray,
        None => Color::DarkGray,
    }
}

fn render_movie(frame: &mut Frame, app: &App, area: Rect) {
    let Some(anime) = app.cached_detail() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(cached_movie_lines(anime))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Movie details "),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn cached_movie_lines(anime: &Anime) -> Vec<Line<'static>> {
    let released = anime
        .latest_release_at
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| EMPTY.to_string());
    vec![
        Line::from(Span::styled(
            anime.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("Status: {}", anime.status)),
        Line::from(format!("Released: {released}")),
        Line::from(""),
        Line::from(anime.description.clone()),
        Line::from(""),
        Line::from("Press Enter to play."),
    ]
}

/// Renders `/anime/{id}/full`. Lines are wrapped up front so the scroll offset
/// can be clamped against the real rendered height.
fn render_live_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(anime) = app.live_detail() else {
        return;
    };
    let title = format!(" {} ", truncate(anime.display_title(), area.width as usize));
    let width = area.width.saturating_sub(2).max(10) as usize;
    let lines = live_detail_lines(anime, width);

    let visible = area.height.saturating_sub(2);
    let max_scroll = (lines.len() as u16).saturating_sub(visible);
    app.clamp_detail_scroll(max_scroll);
    let scroll = app.detail_scroll;

    let indicator = if max_scroll > 0 {
        format!(" {}/{} ", scroll.min(max_scroll) + 1, max_scroll + 1)
    } else {
        String::new()
    };

    frame.render_widget(
        Paragraph::new(lines).scroll((scroll, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_bottom(indicator),
        ),
        area,
    );
}

fn live_detail_lines(anime: &LiveAnime, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(
        anime.display_title().to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))];

    if anime.title_english.is_some() && anime.title != anime.display_title() {
        lines.push(Line::from(anime.title.clone()));
    }
    if let Some(japanese) = &anime.title_japanese {
        lines.push(Line::from(japanese.clone()));
    }
    lines.push(Line::from(""));

    let mut facts: Vec<(&str, String)> = Vec::new();
    push_fact(&mut facts, "Type", anime.media_type.clone());
    push_fact(
        &mut facts,
        "Episodes",
        anime.episodes.map(|count| count.to_string()),
    );
    push_fact(&mut facts, "Status", anime.status.clone());
    push_fact(
        &mut facts,
        "Aired",
        anime.aired.as_ref().and_then(|aired| aired.string.clone()),
    );
    push_fact(&mut facts, "Season", anime.season_label());
    push_fact(
        &mut facts,
        "Broadcast",
        anime
            .broadcast
            .as_ref()
            .and_then(|slot| slot.string.clone()),
    );
    push_fact(&mut facts, "Duration", anime.duration.clone());
    push_fact(&mut facts, "Rating", anime.rating.clone());
    push_fact(&mut facts, "Source", anime.source.clone());
    push_fact(
        &mut facts,
        "Score",
        anime.score.map(|score| match anime.scored_by {
            Some(count) => format!("{score:.2} from {} users", thousands(count)),
            None => format!("{score:.2}"),
        }),
    );
    push_fact(
        &mut facts,
        "Rank",
        anime.rank.map(|rank| format!("#{rank}")),
    );
    push_fact(
        &mut facts,
        "Popularity",
        anime.popularity.map(|rank| format!("#{rank}")),
    );
    push_fact(&mut facts, "Members", anime.members.map(thousands));
    push_fact(&mut facts, "Favorites", anime.favorites.map(thousands));
    push_fact(&mut facts, "Studios", names(&anime.studios));
    push_fact(&mut facts, "Producers", names(&anime.producers));
    push_fact(&mut facts, "Licensors", names(&anime.licensors));
    push_fact(&mut facts, "Genres", names(&anime.genres));
    push_fact(&mut facts, "Themes", names(&anime.themes));
    push_fact(&mut facts, "Demographic", names(&anime.demographics));

    for (label, value) in facts {
        lines.extend(labelled(label, &value, width));
    }

    if let Some(synopsis) = non_empty(anime.synopsis.as_deref()) {
        lines.push(Line::from(""));
        lines.push(heading("Synopsis"));
        lines.extend(wrapped(synopsis, width));
    }
    if let Some(background) = non_empty(anime.background.as_deref()) {
        lines.push(Line::from(""));
        lines.push(heading("Background"));
        lines.extend(wrapped(background, width));
    }
    if let Some(songs) = &anime.songs {
        if !songs.openings.is_empty() {
            lines.push(Line::from(""));
            lines.push(heading("Openings"));
            for song in &songs.openings {
                lines.extend(wrapped(song, width));
            }
        }
        if !songs.endings.is_empty() {
            lines.push(Line::from(""));
            lines.push(heading("Endings"));
            for song in &songs.endings {
                lines.extend(wrapped(song, width));
            }
        }
    }
    if !anime.streaming.is_empty() {
        lines.push(Line::from(""));
        lines.push(heading("Streaming"));
        for link in &anime.streaming {
            lines.extend(wrapped(&format!("{} — {}", link.name, link.url), width));
        }
    }
    if let Some(url) = non_empty(anime.url.as_deref()) {
        lines.push(Line::from(""));
        lines.extend(labelled("MyAnimeList", url, width));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(if anime.is_movie() {
        "Press Enter to play.".to_string()
    } else {
        "Press Enter to choose an episode.".to_string()
    }));
    lines
}

/// The player runs detached, so this reports what was handed over rather than
/// tracking playback — once mpv has the stream, termuto is no longer involved.
fn render_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    // A stream URL is long enough to wrap, so the entries are wrapped here and
    // the popup is sized to the result. A fixed height silently cuts off the
    // last lines, which are the ones worth reading when nothing plays.
    let width = percent_of(area.width, 78).max(20);
    let inner = width.saturating_sub(2).max(10) as usize;

    let mut lines = Vec::new();
    for (index, entry) in app.now_playing.iter().flatten().enumerate() {
        if index == 0 {
            lines.extend(wrap_text(entry, inner).into_iter().map(|chunk| {
                Line::from(Span::styled(
                    chunk,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
            }));
        } else {
            lines.extend(wrapped(entry, inner));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Press any key to return."));

    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = centered(width, height, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Now playing ")
                .border_style(Style::default().fg(Color::Green)),
        ),
        popup,
    );
}

fn percent_of(total: u16, percent: u16) -> u16 {
    (u32::from(total) * u32::from(percent) / 100) as u16
}

/// A popup of an exact size, centered in `area` and never larger than it.
fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
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

fn render_error(frame: &mut Frame, app: &App, area: Rect) {
    let popup = centered_rect(70, 40, area);
    frame.render_widget(Clear, popup);
    let message = app.error.clone().unwrap_or_else(|| "Unknown error".into());
    frame.render_widget(
        Paragraph::new(format!("{message}\n\nPress any key to return."))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Request failed ")
                    .border_style(Style::default().fg(Color::Red)),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn render_loading(frame: &mut Frame, label: &str, area: Rect) {
    let popup = centered_rect(50, 20, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(label.to_string())
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Working ")
                    .border_style(Style::default().fg(Color::Cyan)),
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

fn summary_items(rows: &[AnimeSummary]) -> Vec<ListItem<'static>> {
    rows.iter()
        .map(|row| match &row.note {
            Some(note) => ListItem::new(vec![
                Line::from(row.row()),
                Line::from(Span::styled(
                    format!("    {note}"),
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            None => ListItem::new(row.row()),
        })
        .collect()
}

fn help_text(app: &App) -> &'static str {
    if app.is_busy() {
        return "Loading…";
    }
    match app.screen {
        Screen::Home => "↑/↓ select • Enter open • / Search • p Provider • a Auto • q Quit",
        Screen::Search => "Type a query • Enter search • ↓ results • Esc back • Ctrl-C quit",
        Screen::SeasonPicker => "↑/↓ or j/k select • Enter open • p Provider • a Auto • Esc back",
        Screen::LiveDetail => "↑/↓ or j/k scroll • Enter play • p Provider • a Auto • Esc back",
        Screen::LiveEpisodes => {
            "↑/↓ or j/k select • Enter play • v Image • s Synopsis • p Provider • Esc back"
        }
        Screen::Episodes | Screen::MovieDetail => {
            "↑/↓ or j/k select • Enter play • p Provider • a Auto • Esc back"
        }
        Screen::Playing => "Any key to dismiss",
        Screen::QuitConfirm => "y Quit • n Stay • Esc Stay",
        Screen::Error => "Any key to dismiss",
        Screen::Listing => "↑/↓ select • Enter open • / Search • p Provider • a Auto • Esc back",
    }
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
}

/// `Label: value`, with the continuation of a long value indented under it.
fn labelled(label: &str, value: &str, width: usize) -> Vec<Line<'static>> {
    let prefix = format!("{label}: ");
    let indent = " ".repeat(prefix.len().min(width.saturating_sub(1)));
    wrap_text(value, width.saturating_sub(prefix.len()).max(10))
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            if index == 0 {
                Line::from(vec![
                    Span::styled(prefix.clone(), Style::default().fg(Color::DarkGray)),
                    Span::raw(chunk),
                ])
            } else {
                Line::from(format!("{indent}{chunk}"))
            }
        })
        .collect()
}

fn wrapped(text: &str, width: usize) -> Vec<Line<'static>> {
    wrap_text(text, width).into_iter().map(Line::from).collect()
}

/// A plain greedy word wrap. The renderer needs the resulting line count to
/// clamp scrolling, which ratatui's own wrapping does not expose.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.chars().count()
            } else {
                current.chars().count() + 1 + word.chars().count()
            };
            if candidate > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_fact(facts: &mut Vec<(&'static str, String)>, label: &'static str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        facts.push((label, value));
    }
}

fn names(entries: &[Named]) -> Option<String> {
    let joined = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    (!joined.is_empty()).then_some(joined)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn truncate(text: &str, width: usize) -> String {
    truncate_to(text, width.saturating_sub(4))
}

/// Cuts `text` to `width` cells, marking the cut with an ellipsis.
fn truncate_to(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

/// Pads `text` out to `width` so the columns after it line up.
fn pad(text: &str, width: usize) -> String {
    let length = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(length)))
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

#[cfg(test)]
mod tests {
    use super::{App, Color, Screen, header_line, render, thousands, truncate_to, wrap_text};
    use crate::live::{LiveAnime, LiveEpisode};
    use crate::mode::Mode;
    use crate::playback::{Playback, TrackPrefs};
    use crate::source::{AnimeDetail, Source};
    use crossterm::event::{KeyCode, KeyEvent};
    use ratatui::{Terminal, backend::TestBackend};
    use ratatui_image::picker::Picker;

    async fn app() -> App {
        let source = Source::open(Mode::Cached, "catalog.json")
            .await
            .expect("the catalog opens");
        let playback = Playback::new(
            source.catalog().cloned(),
            TrackPrefs::default(),
            "true".to_string(),
        )
        .expect("playback builds");
        // Nothing here queries a terminal, so the fallback renderer stands in.
        App::new(source, playback, Picker::halfblocks())
    }

    fn header(app: &App) -> String {
        header_line(app)
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect()
    }

    /// The indicator is the only thing that says which way the toggle is set,
    /// so it has to appear and disappear with it.
    #[tokio::test]
    async fn the_auto_indicator_shows_only_while_autoswitch_is_on() {
        let mut app = app().await;
        let line = header_line(&app);
        assert!(header(&app).contains("provider: zokoanime · auto"));
        // Coloured, so it reads as a state at a glance rather than as prose.
        let auto = line
            .spans
            .iter()
            .find(|span| span.content == "auto")
            .expect("the indicator");
        assert_eq!(auto.style.fg, Some(Color::Green));

        app.handle_key(KeyEvent::from(KeyCode::Char('a')));
        let off = header(&app);
        assert!(!off.contains("auto"), "{off}");
        // The host it is pinned to is still named.
        assert!(off.contains("provider: zokoanime"), "{off}");
    }

    #[tokio::test]
    async fn the_header_tracks_the_chosen_host() {
        let mut app = app().await;
        app.handle_key(KeyEvent::from(KeyCode::Char('p')));
        assert!(header(&app).contains("provider: megavid"));
    }

    #[test]
    fn wrapping_breaks_on_words_and_keeps_blank_paragraphs() {
        assert_eq!(wrap_text("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap_text("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn long_words_are_not_lost() {
        assert_eq!(
            wrap_text("supercalifragilistic", 5),
            vec!["supercalifragilistic"]
        );
    }

    #[test]
    fn counts_get_thousands_separators() {
        assert_eq!(thousands(1_492_875), "1,492,875");
        assert_eq!(thousands(42), "42");
    }

    #[test]
    fn a_cut_title_says_so() {
        assert_eq!(truncate_to("The Journey's End", 8), "The Jou…");
        assert_eq!(truncate_to("Short", 8), "Short");
    }

    /// Two episodes as `/anime/{id}/episodes` returns them, trimmed to the
    /// fields the picker draws.
    fn episodes() -> Vec<LiveEpisode> {
        serde_json::from_str(
            r#"[{"mal_id":1,"title":"The Journey's End","duration":1559,
                 "aired":"2023-09-29T00:00:00+00:00","score":4.29,
                 "synopsis":"After defeating the Demon King, Himmel and his crew return.",
                 "images":{"jpg":{"image_url":"https://example.test/1.jpg"}}},
                {"mal_id":2,"title":"It Didn't Have to Be Magic","aired":null,"score":null}]"#,
        )
        .expect("the episode payload deserializes")
    }

    fn frieren(app: &mut App, count: u32) {
        app.detail = Some(AnimeDetail::Live(Box::new(LiveAnime {
            mal_id: 52991,
            title: "Sousou no Frieren".into(),
            title_english: Some("Frieren".into()),
            episodes: Some(count),
            ..LiveAnime::default()
        })));
        app.episodes = episodes();
        app.screen = Screen::LiveEpisodes;
    }

    fn draw(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("the test backend builds");
        terminal
            .draw(|frame| render(frame, app))
            .expect("the frame draws");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[tokio::test]
    async fn an_episode_row_carries_its_title_air_date_and_score() {
        let mut app = app().await;
        frieren(&mut app, 2);
        let screen = draw(&mut app, 120, 20);
        assert!(screen.contains("The Journey's End"), "{screen}");
        assert!(screen.contains("2023-09-29"), "{screen}");
        assert!(screen.contains("4.29"), "{screen}");
        // An episode the API has no date or score for still gets a row.
        assert!(screen.contains("It Didn't Have to Be Magic"), "{screen}");
    }

    /// Both panes start open, and each key closes only its own. The panes are
    /// looked for by their block titles: the help line at the foot of the
    /// screen names both keys, so the bare words are always on screen.
    #[tokio::test]
    async fn v_and_s_toggle_the_preview_and_synopsis_panes() {
        let mut app = app().await;
        frieren(&mut app, 2);
        let both = draw(&mut app, 120, 20);
        assert!(
            both.contains(PREVIEW_PANE) && both.contains(SYNOPSIS_PANE),
            "{both}"
        );
        assert!(both.contains("After defeating the Demon King"), "{both}");
        // The runtime rides in the synopsis title.
        assert!(both.contains("Synopsis · 25m"), "{both}");

        app.handle_key(KeyEvent::from(KeyCode::Char('v')));
        let no_preview = draw(&mut app, 120, 20);
        assert!(!no_preview.contains(PREVIEW_PANE), "{no_preview}");
        assert!(no_preview.contains(SYNOPSIS_PANE), "{no_preview}");

        app.handle_key(KeyEvent::from(KeyCode::Char('s')));
        let neither = draw(&mut app, 120, 20);
        assert!(!neither.contains(SYNOPSIS_PANE), "{neither}");
        // With nothing beside it, the list takes the whole width back.
        assert!(neither.contains("The Journey's End"), "{neither}");
    }

    const PREVIEW_PANE: &str = "┌ Preview";
    const SYNOPSIS_PANE: &str = "┌ Synopsis";

    /// The API pages its episode list, and a still-airing title can report more
    /// episodes than a page returns. The remainder is still selectable.
    #[tokio::test]
    async fn episodes_past_the_fetched_list_are_numbered_out() {
        let mut app = app().await;
        frieren(&mut app, 4);
        let screen = draw(&mut app, 120, 20);
        assert_eq!(app.live_episode_count(), 4);
        assert!(
            screen.contains("Episode 3") && screen.contains("Episode 4"),
            "{screen}"
        );
    }

    /// Neither pane is worth the width it would take from the list here.
    #[tokio::test]
    async fn a_narrow_terminal_keeps_the_list_and_drops_the_side_column() {
        let mut app = app().await;
        frieren(&mut app, 2);
        let screen = draw(&mut app, 60, 20);
        assert!(
            !screen.contains(PREVIEW_PANE) && !screen.contains(SYNOPSIS_PANE),
            "{screen}"
        );
        assert!(screen.contains("The Journey's End"), "{screen}");
    }
}
