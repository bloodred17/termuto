use super::preview::{self, Preview, Renderer};
use super::view::{ListView, RowKeys, SortKey};
use crate::catalog::AnimeKind;
use crate::live::LiveEpisode;
use crate::mode::Mode;
use crate::playback::{Playback, StreamRequest};
use crate::source::model::EMPTY;
use crate::source::{AnimeDetail, AnimeSummary, Origin, SeasonRef, Source};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use image::DynamicImage;
use std::collections::HashMap;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// How many rows a listing screen requests. Deliberately modest: every live
/// screen is one or two API pages.
const LISTING_LIMIT: usize = 50;

/// The episode list is paged like any other, and a long-running title has more
/// episodes than anyone scrolls. Whatever the fetch does not reach still gets a
/// numbered, playable row from the title's own episode count.
const EPISODE_LIMIT: usize = 200;

/// How many stills to hold before dropping the ones not in use. A decoded still
/// and its encoded form cost a megabyte or so each, and a long-running title has
/// hundreds of episodes to scroll past.
const PREVIEW_CACHE: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Screen {
    Home,
    /// Any list of titles. The heading and rows live on the app.
    Listing,
    /// The year/season index from `/seasons`.
    SeasonPicker,
    Search,
    /// A title loaded from the API.
    LiveDetail,
    /// A catalog series and its episodes.
    Episodes,
    /// The episodes of a live series. The API gives a count rather than a list,
    /// so the numbers are counted out rather than fetched.
    LiveEpisodes,
    /// A catalog movie.
    MovieDetail,
    /// Raised once the player has been handed a resolved stream.
    Playing,
    QuitConfirm,
    /// Raised over whatever was on screen when a request failed.
    Error,
}

/// Whether typing edits the query or moves through the results below it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchFocus {
    Query,
    Results,
}

/// Work that needs the network. Key handling records one, the event loop draws
/// a loading frame, and only then runs it — so the UI never freezes silently.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Top,
    CurrentSeason,
    SeasonIndex,
    Season(SeasonRef),
    Recommendations,
    Latest,
    Ongoing,
    Search(String),
    Detail(Origin),
    /// The episode list of a live title, fetched when its picker is opened.
    Episodes(u32),
    /// Resolving a stream can take as long as a listing, so it queues like one.
    Play(StreamRequest),
}

impl Action {
    fn loading_label(&self) -> String {
        match self {
            Self::Top => "Loading top anime…".to_string(),
            Self::CurrentSeason => "Loading this season…".to_string(),
            Self::SeasonIndex => "Loading seasons…".to_string(),
            Self::Season(season) => format!("Loading {}…", season.label()),
            Self::Recommendations => "Loading recommendations…".to_string(),
            Self::Latest => "Loading latest releases…".to_string(),
            Self::Ongoing => "Loading airing titles…".to_string(),
            Self::Search(query) => format!("Searching for \"{query}\"…"),
            Self::Detail(_) => "Loading details…".to_string(),
            Self::Episodes(_) => "Loading episodes…".to_string(),
            Self::Play(request) => format!("Resolving {}…", request.label()),
        }
    }
}

/// One entry in the home menu.
struct HomeEntry {
    label: &'static str,
    choice: HomeChoice,
}

enum HomeChoice {
    /// Needs the network, so it goes through the pending-action path.
    Load(Action),
    Search,
    Quit,
}

pub(crate) struct App {
    source: Source,
    playback: Playback,
    pub(crate) screen: Screen,
    /// Screens to return to, innermost last.
    history: Vec<Screen>,
    pub(crate) home_index: usize,
    pub(crate) list_index: usize,
    pub(crate) episode_index: usize,
    pub(crate) season_index: usize,
    pub(crate) listing_title: String,
    pub(crate) listing: Vec<AnimeSummary>,
    pub(crate) seasons: Vec<SeasonRef>,
    /// How each list is sorted and filtered. One per kind of list rather than
    /// one per screen: the listing and its search results are the same rows,
    /// and the two episode pickers never show at once.
    pub(crate) listing_view: ListView,
    pub(crate) episodes_view: ListView,
    pub(crate) seasons_view: ListView,
    pub(crate) search_input: String,
    pub(crate) search_focus: SearchFocus,
    pub(crate) search_submitted: Option<String>,
    pub(crate) detail: Option<AnimeDetail>,
    pub(crate) detail_scroll: u16,
    /// The episode list of the live title on screen, when the API had one.
    pub(crate) episodes: Vec<LiveEpisode>,
    /// Whether the still and the synopsis panes are shown; `v` and `s` toggle
    /// them, and both start on.
    pub(crate) show_preview: bool,
    pub(crate) show_synopsis: bool,
    /// Stills by URL, so scrolling back over an episode redraws immediately.
    previews: HashMap<String, Preview>,
    preview_tx: UnboundedSender<(String, Option<Box<DynamicImage>>)>,
    preview_rx: UnboundedReceiver<(String, Option<Box<DynamicImage>>)>,
    /// How stills get drawn, as worked out from the terminal.
    renderer: Renderer,
    pub(crate) error: Option<String>,
    /// What the last successful play handed to the player, shown on
    /// [`Screen::Playing`].
    pub(crate) now_playing: Option<Vec<String>>,
    pending: Option<Action>,
    pub(crate) loading: Option<String>,
}

impl App {
    pub(crate) fn new(source: Source, playback: Playback, renderer: Renderer) -> Self {
        let (preview_tx, preview_rx) = unbounded_channel();
        Self {
            source,
            playback,
            screen: Screen::Home,
            history: Vec::new(),
            home_index: 0,
            list_index: 0,
            episode_index: 0,
            season_index: 0,
            listing_title: String::new(),
            listing: Vec::new(),
            seasons: Vec::new(),
            listing_view: ListView::default(),
            episodes_view: ListView::default(),
            seasons_view: ListView::default(),
            search_input: String::new(),
            search_focus: SearchFocus::Query,
            search_submitted: None,
            detail: None,
            detail_scroll: 0,
            episodes: Vec::new(),
            show_preview: true,
            show_synopsis: true,
            previews: HashMap::new(),
            preview_tx,
            preview_rx,
            renderer,
            error: None,
            now_playing: None,
            pending: None,
            loading: None,
        }
    }

    pub(crate) fn mode(&self) -> Mode {
        self.source.mode()
    }

    pub(crate) fn player_name(&self) -> &str {
        self.playback.player_name()
    }

    /// The host `p` currently has selected, shown in the header so the choice
    /// is visible rather than something to remember.
    pub(crate) fn provider_name(&self) -> &str {
        self.playback.provider().unwrap_or("none")
    }

    /// Whether a host with nothing to offer falls through to the next one.
    /// Shown in the header as `auto`, since it changes which host a title can
    /// come from and that is otherwise invisible until something fails.
    pub(crate) fn autoswitch(&self) -> bool {
        self.playback.autoswitch()
    }

    /// The screen whose contents are on show. The quit prompt, the now-playing
    /// panel, and errors are overlays, so the screen underneath keeps drawing.
    pub(crate) fn display_screen(&self) -> Screen {
        match self.screen {
            Screen::QuitConfirm | Screen::Error | Screen::Playing => self
                .history
                .last()
                .copied()
                .filter(|screen| {
                    !matches!(
                        screen,
                        Screen::QuitConfirm | Screen::Error | Screen::Playing
                    )
                })
                .unwrap_or(Screen::Home),
            screen => screen,
        }
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// The menu depends on the mode: the API-only screens are hidden when the
    /// API is not in play.
    fn home_entries(&self) -> Vec<HomeEntry> {
        let mut entries = Vec::new();
        if self.source.supports_live_listings() {
            entries.push(HomeEntry {
                label: "Top anime",
                choice: HomeChoice::Load(Action::Top),
            });
            entries.push(HomeEntry {
                label: "This season",
                choice: HomeChoice::Load(Action::CurrentSeason),
            });
            entries.push(HomeEntry {
                label: "Browse seasons",
                choice: HomeChoice::Load(Action::SeasonIndex),
            });
            entries.push(HomeEntry {
                label: "Recommendations",
                choice: HomeChoice::Load(Action::Recommendations),
            });
        } else {
            entries.push(HomeEntry {
                label: "Latest releases",
                choice: HomeChoice::Load(Action::Latest),
            });
        }
        entries.push(HomeEntry {
            label: "Airing now",
            choice: HomeChoice::Load(Action::Ongoing),
        });
        entries.push(HomeEntry {
            label: "Search",
            choice: HomeChoice::Search,
        });
        entries.push(HomeEntry {
            label: "Quit",
            choice: HomeChoice::Quit,
        });
        entries
    }

    pub(crate) fn home_labels(&self) -> Vec<&'static str> {
        self.home_entries()
            .into_iter()
            .map(|entry| entry.label)
            .collect()
    }

    /// Hands the queued action to the caller so the event loop can await it
    /// after a frame has been drawn.
    pub(crate) fn take_pending(&mut self) -> Option<Action> {
        self.pending.take()
    }

    fn queue(&mut self, action: Action) {
        self.loading = Some(action.loading_label());
        self.pending = Some(action);
    }

    /// Performs one queued action. Failures become a dismissible overlay rather
    /// than ending the session.
    pub(crate) async fn run_action(&mut self, action: Action) -> Result<()> {
        let outcome = self.perform(&action).await;
        self.loading = None;
        if let Err(error) = outcome {
            self.error = Some(format!("{error:#}"));
            self.enter(Screen::Error);
        }
        Ok(())
    }

    async fn perform(&mut self, action: &Action) -> Result<()> {
        match action {
            Action::Top => {
                let rows = self.source.top(LISTING_LIMIT).await?;
                self.show_listing("Top anime", rows);
            }
            Action::CurrentSeason => {
                let rows = self.source.current_season(LISTING_LIMIT).await?;
                self.show_listing("This season", rows);
            }
            Action::SeasonIndex => {
                self.seasons = self.source.seasons_index().await?;
                self.season_index = 0;
                self.seasons_view = ListView::default();
                self.enter(Screen::SeasonPicker);
            }
            Action::Season(season) => {
                let rows = self.source.season(season, LISTING_LIMIT).await?;
                self.show_listing(&season.label(), rows);
            }
            Action::Recommendations => {
                let rows = self.source.recommendations(LISTING_LIMIT).await?;
                self.show_listing("Recommendations", rows);
            }
            Action::Latest => {
                let rows = self.source.latest(LISTING_LIMIT).await?;
                self.show_listing("Latest releases", rows);
            }
            Action::Ongoing => {
                let rows = self.source.ongoing(LISTING_LIMIT).await?;
                self.show_listing("Airing now", rows);
            }
            Action::Search(query) => {
                self.listing = self.source.search(query, LISTING_LIMIT).await?;
                self.search_submitted = Some(query.clone());
                self.list_index = 0;
                self.listing_view = ListView::default();
                self.search_focus = if self.listing.is_empty() {
                    SearchFocus::Query
                } else {
                    SearchFocus::Results
                };
            }
            Action::Detail(origin) => {
                let detail = self.source.detail(origin).await?;
                self.detail_scroll = 0;
                self.episode_index = 0;
                self.episodes_view = ListView::default();
                let screen = match &detail {
                    AnimeDetail::Live(_) => Screen::LiveDetail,
                    AnimeDetail::Cached(anime) => match anime.kind {
                        AnimeKind::Series => Screen::Episodes,
                        AnimeKind::Movie => Screen::MovieDetail,
                    },
                };
                self.detail = Some(detail);
                self.enter(screen);
            }
            // A failed episode list is not worth an error overlay: the title
            // already reports how many episodes it has, and the picker falls
            // back to numbering them out, which is what it did before.
            Action::Episodes(id) => {
                self.episodes = self
                    .source
                    .live_episodes(*id, EPISODE_LIMIT)
                    .await
                    .unwrap_or_default();
                self.episode_index = 0;
                self.episodes_view = ListView::default();
                self.previews.clear();
                self.enter(Screen::LiveEpisodes);
                self.request_preview();
            }
            Action::Play(request) => {
                let label = request.label();
                let stream = self.playback.play(request.clone()).await?;
                self.now_playing = Some(vec![
                    label,
                    format!("via {stream}"),
                    format!("in {}", self.playback.player_name()),
                    stream.url,
                    format!("Player output: {}", self.playback.log_path().display()),
                ]);
                self.enter(Screen::Playing);
            }
        }
        Ok(())
    }

    /// A new list arrives in the order its source chose, with nothing hidden:
    /// a filter typed against the last screen has no meaning on this one.
    fn show_listing(&mut self, title: &str, rows: Vec<AnimeSummary>) {
        self.listing_title = title.to_string();
        self.listing = rows;
        self.list_index = 0;
        self.listing_view = ListView::default();
        self.enter(Screen::Listing);
    }

    /// Moves to `screen`, remembering the current one for `Esc`.
    fn enter(&mut self, screen: Screen) {
        if self.screen != screen {
            self.history.push(self.screen);
            self.screen = screen;
        }
    }

    fn go_back(&mut self) {
        self.screen = self.history.pop().unwrap_or(Screen::Home);
        if self.screen == Screen::Home {
            self.history.clear();
        }
    }

    /// Returns `false` when the session should end.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return !matches!(key.code, KeyCode::Char('c'));
        }

        match self.screen {
            // These overlays swallow every other binding until answered.
            Screen::QuitConfirm => return self.handle_quit_confirm_key(key),
            Screen::Error => {
                self.error = None;
                self.go_back();
                return true;
            }
            Screen::Playing => {
                self.now_playing = None;
                self.go_back();
                return true;
            }
            _ => {}
        }

        // A filter being typed owns the keyboard, so `q` and the rest stay
        // ordinary letters until it is accepted or abandoned.
        if self.filtering() {
            return self.handle_filter_key(key);
        }
        if self.screen == Screen::Search {
            return self.handle_search_key(key);
        }

        match key.code {
            // `q` is reserved for the query text while Search has focus, but
            // quits from every other screen.
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Esc => self.go_back(),
            KeyCode::Char('/') => self.enter_search(),
            // Both take effect on the next play; nothing already handed to the
            // player is affected.
            KeyCode::Char('p') => self.playback.cycle_provider(),
            KeyCode::Char('a') => {
                self.playback.toggle_autoswitch();
            }
            // Only the episode picker has panes to hide, so elsewhere these are
            // ordinary letters with nothing to do.
            KeyCode::Char('v') if self.screen == Screen::LiveEpisodes => {
                self.show_preview = !self.show_preview;
                self.request_preview();
            }
            KeyCode::Char('s') if self.screen == Screen::LiveEpisodes => {
                self.show_synopsis = !self.show_synopsis;
            }
            // No-ops on a screen without a list, like the toggles above.
            KeyCode::Char('f') => self.begin_filter(),
            KeyCode::Char('d') => self.sort_by(SortKey::Date),
            KeyCode::Char('n') => self.sort_by(SortKey::Name),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Enter => self.select_current(),
            _ => {}
        }
        true
    }

    fn handle_quit_confirm_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => false,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.go_back();
                true
            }
            _ => true,
        }
    }

    fn request_quit(&mut self) {
        self.enter(Screen::QuitConfirm);
    }

    /// Live searches cost a request, so the query is submitted with `Enter`
    /// rather than on every keystroke. Focus then drops into the results.
    fn handle_search_key(&mut self, key: KeyEvent) -> bool {
        match (self.search_focus, key.code) {
            (_, KeyCode::Esc) if self.search_focus == SearchFocus::Results => {
                self.search_focus = SearchFocus::Query;
            }
            (_, KeyCode::Esc) => {
                self.search_input.clear();
                self.search_submitted = None;
                self.listing.clear();
                self.list_index = 0;
                self.go_back();
            }
            (SearchFocus::Query, KeyCode::Enter) => {
                let query = self.search_input.trim().to_string();
                if !query.is_empty() {
                    self.queue(Action::Search(query));
                }
            }
            (SearchFocus::Query, KeyCode::Backspace) => {
                self.search_input.pop();
            }
            (SearchFocus::Query, KeyCode::Char(character)) => {
                self.search_input.push(character);
            }
            (SearchFocus::Query, KeyCode::Down) if !self.listing.is_empty() => {
                self.search_focus = SearchFocus::Results;
            }
            (SearchFocus::Results, KeyCode::Enter) => self.select_current(),
            (SearchFocus::Results, KeyCode::Char('/')) => {
                self.search_focus = SearchFocus::Query;
            }
            // The query is what the API was asked; these narrow and reorder
            // what came back, without spending another request.
            (SearchFocus::Results, KeyCode::Char('f')) => self.begin_filter(),
            (SearchFocus::Results, KeyCode::Char('d')) => self.sort_by(SortKey::Date),
            (SearchFocus::Results, KeyCode::Char('n')) => self.sort_by(SortKey::Name),
            (SearchFocus::Results, KeyCode::Up | KeyCode::Char('k')) => {
                if self.list_index == 0 {
                    self.search_focus = SearchFocus::Query;
                } else {
                    self.move_selection(-1);
                }
            }
            (SearchFocus::Results, KeyCode::Down | KeyCode::Char('j')) => self.move_selection(1),
            (SearchFocus::Results, KeyCode::PageUp) => self.move_selection(-10),
            (SearchFocus::Results, KeyCode::PageDown) => self.move_selection(10),
            _ => {}
        }
        true
    }

    fn enter_search(&mut self) {
        self.search_input.clear();
        self.search_submitted = None;
        self.listing.clear();
        self.list_index = 0;
        self.listing_view = ListView::default();
        self.search_focus = SearchFocus::Query;
        self.enter(Screen::Search);
    }

    /// Whether a filter is being typed, which changes both what the keys mean
    /// and what the foot of the screen offers.
    pub(crate) fn filtering(&self) -> bool {
        self.active_view().is_some_and(ListView::editing)
    }

    /// Typing narrows the list on every keystroke: the rows are already in
    /// hand, so there is nothing to wait for and no reason to make it a
    /// submitted query like the API search above.
    fn handle_filter_key(&mut self, key: KeyEvent) -> bool {
        let selected = self.selected_row();
        match key.code {
            // Esc abandons the filter; Enter keeps it and returns to the list.
            KeyCode::Esc => self.with_view(|view| view.cancel_filter()),
            KeyCode::Enter => {
                self.with_view(|view| view.accept_filter());
                return true;
            }
            KeyCode::Backspace => self.with_view(|view| view.pop_filter()),
            KeyCode::Char(character) => self.with_view(|view| view.push_filter(character)),
            // The list stays live under the filter bar, so it can be walked
            // without accepting the filter first.
            KeyCode::Up => self.move_selection(-1),
            KeyCode::Down => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            _ => {}
        }
        // Whatever the filter now leaves may not hold the selected row, or may
        // hold it somewhere else.
        self.follow_selection(selected);
        true
    }

    fn begin_filter(&mut self) {
        self.with_view(|view| view.begin_filter());
    }

    fn sort_by(&mut self, key: SortKey) {
        let selected = self.selected_row();
        self.with_view(|view| view.sort_by(key));
        self.follow_selection(selected);
    }

    fn with_view(&mut self, edit: impl FnOnce(&mut ListView)) {
        if let Some(view) = self.active_view_mut() {
            edit(view);
        }
    }

    /// The list the sort and filter keys act on, for the screens that have one.
    /// The search screen only counts once its results have focus — until then
    /// every letter belongs to the query.
    fn active_view(&self) -> Option<&ListView> {
        match self.screen {
            Screen::Listing => Some(&self.listing_view),
            Screen::Search if self.search_focus == SearchFocus::Results => Some(&self.listing_view),
            Screen::Episodes | Screen::LiveEpisodes => Some(&self.episodes_view),
            Screen::SeasonPicker => Some(&self.seasons_view),
            _ => None,
        }
    }

    fn active_view_mut(&mut self) -> Option<&mut ListView> {
        match self.screen {
            Screen::Listing => Some(&mut self.listing_view),
            Screen::Search if self.search_focus == SearchFocus::Results => {
                Some(&mut self.listing_view)
            }
            Screen::Episodes | Screen::LiveEpisodes => Some(&mut self.episodes_view),
            Screen::SeasonPicker => Some(&mut self.seasons_view),
            _ => None,
        }
    }

    /// The rows the screen shows, as positions into the data behind it.
    fn visible(&self) -> Option<Vec<usize>> {
        match self.screen {
            Screen::Listing | Screen::Search => Some(self.visible_listing()),
            Screen::Episodes | Screen::LiveEpisodes => Some(self.visible_episodes()),
            Screen::SeasonPicker => Some(self.visible_seasons()),
            _ => None,
        }
    }

    fn selection_mut(&mut self) -> Option<&mut usize> {
        match self.screen {
            Screen::Listing | Screen::Search => Some(&mut self.list_index),
            Screen::Episodes | Screen::LiveEpisodes => Some(&mut self.episode_index),
            Screen::SeasonPicker => Some(&mut self.season_index),
            _ => None,
        }
    }

    /// Which row of the underlying data the highlight is on, as opposed to
    /// which line of the screen it sits on.
    fn selected_row(&self) -> Option<usize> {
        let index = match self.screen {
            Screen::Listing | Screen::Search => self.list_index,
            Screen::Episodes | Screen::LiveEpisodes => self.episode_index,
            Screen::SeasonPicker => self.season_index,
            _ => return None,
        };
        self.visible()?.get(index).copied()
    }

    /// Puts the highlight back on `row` wherever the new order moved it. A
    /// sort should reorder the list under the selection, not send it elsewhere;
    /// a filter that hides the selected row falls back to the top.
    fn follow_selection(&mut self, row: Option<usize>) {
        let Some(order) = self.visible() else {
            return;
        };
        let position = row
            .and_then(|row| order.iter().position(|index| *index == row))
            .unwrap_or(0)
            .min(order.len().saturating_sub(1));
        if let Some(selection) = self.selection_mut() {
            *selection = position;
        }
        if self.screen == Screen::LiveEpisodes {
            self.request_preview();
        }
    }

    pub(crate) fn visible_listing(&self) -> Vec<usize> {
        self.listing_view.order(&self.listing_keys())
    }

    pub(crate) fn visible_episodes(&self) -> Vec<usize> {
        self.episodes_view.order(&self.episode_keys())
    }

    pub(crate) fn visible_seasons(&self) -> Vec<usize> {
        self.seasons_view.order(&self.season_keys())
    }

    fn listing_keys(&self) -> Vec<RowKeys> {
        self.listing
            .iter()
            .map(|row| RowKeys::new(row.title.clone(), sortable_date(&row.released)))
            .collect()
    }

    /// Which picker is on screen follows from which kind of title was loaded,
    /// so the keys are built from the detail rather than from the screen — the
    /// overlays draw the picker underneath themselves.
    fn episode_keys(&self) -> Vec<RowKeys> {
        if let Some(anime) = self.cached_detail() {
            return anime
                .episodes
                .iter()
                .map(|episode| {
                    RowKeys::new(
                        episode.title.clone(),
                        episode
                            .released_at
                            .map(|date| date.format("%Y-%m-%d").to_string()),
                    )
                })
                .collect();
        }
        (0..self.live_episode_count())
            .map(|index| match self.episode(index) {
                Some(episode) => RowKeys::new(episode.display_title(), episode.aired_label()),
                // The rows the fetched list did not reach know only their number.
                None => RowKeys::new(format!("Episode {}", index + 1), None),
            })
            .collect()
    }

    fn season_keys(&self) -> Vec<RowKeys> {
        self.seasons
            .iter()
            .map(|season| {
                RowKeys::new(
                    season.label(),
                    Some(format!("{:04}-{:02}", season.year, season_month(season))),
                )
            })
            .collect()
    }

    fn move_selection(&mut self, direction: isize) {
        match self.screen {
            Screen::Home => {
                let count = self.home_entries().len();
                move_index(&mut self.home_index, count, direction);
            }
            // Movement is through what the list shows, so a filtered list steps
            // between its matches rather than over the rows in between.
            Screen::Listing | Screen::Search => {
                let count = self.visible_listing().len();
                move_index(&mut self.list_index, count, direction);
            }
            Screen::SeasonPicker => {
                let count = self.visible_seasons().len();
                move_index(&mut self.season_index, count, direction);
            }
            Screen::Episodes => {
                let count = self.visible_episodes().len();
                move_index(&mut self.episode_index, count, direction);
            }
            Screen::LiveEpisodes => {
                let count = self.visible_episodes().len();
                move_index(&mut self.episode_index, count, direction);
                self.request_preview();
            }
            Screen::LiveDetail => {
                self.detail_scroll = if direction < 0 {
                    self.detail_scroll
                        .saturating_sub(direction.unsigned_abs() as u16)
                } else {
                    self.detail_scroll.saturating_add(direction as u16)
                };
            }
            _ => {}
        }
    }

    fn select_current(&mut self) {
        match self.screen {
            Screen::Home => {
                let mut entries = self.home_entries();
                if self.home_index >= entries.len() {
                    return;
                }
                match entries.remove(self.home_index).choice {
                    HomeChoice::Load(action) => self.queue(action),
                    HomeChoice::Search => self.enter_search(),
                    HomeChoice::Quit => self.request_quit(),
                }
            }
            // Every screen here opens the row the highlight is on, which after
            // a sort or a filter is not the row at that position in the data.
            Screen::Listing | Screen::Search => {
                let origin = self
                    .selected_row()
                    .and_then(|row| self.listing.get(row))
                    .map(|row| row.origin.clone());
                if let Some(origin) = origin {
                    self.queue(Action::Detail(origin));
                }
            }
            Screen::SeasonPicker => {
                let season = self
                    .selected_row()
                    .and_then(|row| self.seasons.get(row))
                    .cloned();
                if let Some(season) = season {
                    self.queue(Action::Season(season));
                }
            }
            // A catalog series plays the highlighted episode; a movie has none.
            Screen::Episodes => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                let Some(anime) = self.cached_detail() else {
                    return;
                };
                let Some(episode) = anime.episodes.get(row) else {
                    return;
                };
                let request = self.playback.request(
                    Origin::Cached(anime.id.clone()),
                    anime.title.clone(),
                    Some(episode.number),
                );
                self.queue(Action::Play(request));
            }
            Screen::MovieDetail => {
                let Some(anime) = self.cached_detail() else {
                    return;
                };
                let request = self.playback.request(
                    Origin::Cached(anime.id.clone()),
                    anime.title.clone(),
                    None,
                );
                self.queue(Action::Play(request));
            }
            // A live movie plays straight away; a series needs an episode first.
            Screen::LiveDetail => {
                let Some(anime) = self.live_detail() else {
                    return;
                };
                if anime.is_movie() || self.live_episode_count() <= 1 {
                    let request = self.playback.request(
                        Origin::Live(anime.mal_id),
                        anime.display_title().to_string(),
                        (!anime.is_movie()).then_some(1),
                    );
                    self.queue(Action::Play(request));
                } else {
                    self.queue(Action::Episodes(anime.mal_id));
                }
            }
            Screen::LiveEpisodes => {
                let Some(row) = self.selected_row() else {
                    return;
                };
                let Some(anime) = self.live_detail() else {
                    return;
                };
                let number = self.episode_number(row);
                let request = self.playback.request(
                    Origin::Live(anime.mal_id),
                    anime.display_title().to_string(),
                    Some(number),
                );
                self.queue(Action::Play(request));
            }
            _ => {}
        }
    }

    /// How many rows the picker shows. `/anime/{id}/episodes` is the better
    /// answer, but it is paged and can trail a still-airing title, so the count
    /// the title itself reports still sets the floor — every episode stays
    /// playable even when the list does not reach it.
    pub(crate) fn live_episode_count(&self) -> usize {
        let reported = self
            .live_detail()
            .map_or(0, |anime| anime.episodes.unwrap_or(1).max(1) as usize);
        reported.max(self.episodes.len())
    }

    /// The episode at `index`, for the rows the fetched list covers.
    pub(crate) fn episode(&self, index: usize) -> Option<&LiveEpisode> {
        self.episodes.get(index)
    }

    /// The episode the highlight is on, looked up through the view so the still
    /// and the synopsis beside a sorted list belong to the row they sit next to.
    pub(crate) fn selected_episode(&self) -> Option<&LiveEpisode> {
        self.episode(self.visible_episodes().get(self.episode_index).copied()?)
    }

    /// What to ask the provider for. The API numbers episodes itself, and a
    /// title whose list starts at 0 or skips a special would otherwise play the
    /// wrong part; rows past the fetched list fall back to their position.
    fn episode_number(&self, index: usize) -> u32 {
        self.episode(index)
            .map(|episode| episode.mal_id)
            .unwrap_or(index as u32 + 1)
    }

    /// How stills are being drawn, named in the preview pane so a fallback to
    /// half-blocks is visible rather than just looking bad.
    pub(crate) fn preview_protocol(&self) -> String {
        self.renderer.label()
    }

    /// The still for the selected episode, once it has arrived.
    pub(crate) fn selected_preview(&self) -> Option<&Preview> {
        self.previews.get(self.preview_url()?.as_str())
    }

    /// Drawing advances the protocol's own state — it resizes and re-encodes
    /// when the pane it is given changes — so the renderer needs it by value.
    pub(crate) fn selected_preview_mut(&mut self) -> Option<&mut Preview> {
        let url = self.preview_url()?;
        self.previews.get_mut(url.as_str())
    }

    /// The still to show: the episode's own, or the title's poster for the
    /// episodes — and the trailing numbered rows — the API has no still for.
    fn preview_url(&self) -> Option<String> {
        let episode = self.selected_episode().and_then(LiveEpisode::image_url);
        episode
            .or_else(|| self.live_detail().and_then(|anime| anime.image_url()))
            .map(str::to_string)
    }

    /// Starts fetching the selected still if it is not already in hand. The
    /// work runs on its own task so moving through the list never blocks on it.
    fn request_preview(&mut self) {
        if !self.show_preview {
            return;
        }
        let Some(url) = self.preview_url() else {
            return;
        };
        if self.previews.contains_key(&url) {
            return;
        }
        let Some(client) = self.source.live().cloned() else {
            return;
        };

        self.forget_unused_previews();
        self.previews.insert(url.clone(), Preview::Pending);
        let sender = self.preview_tx.clone();
        tokio::spawn(async move {
            let decoded = preview::fetch(&client, &url).await.map(Box::new);
            let _ = sender.send((url, decoded));
        });
    }

    /// Drops everything but the still on screen once the cache has grown past
    /// what scrolling back is likely to want.
    fn forget_unused_previews(&mut self) {
        if self.previews.len() < PREVIEW_CACHE {
            return;
        }
        let showing = self.preview_url();
        self.previews
            .retain(|url, _| Some(url.as_str()) == showing.as_deref());
    }

    /// Takes whatever stills finished since the last frame. Called by the event
    /// loop, which redraws on its own timer, so a late arrival simply appears.
    ///
    /// The protocol is built here rather than on the fetching task: it holds
    /// terminal state, and only this thread draws.
    pub(crate) fn collect_previews(&mut self) {
        while let Ok((url, decoded)) = self.preview_rx.try_recv() {
            let entry = match decoded {
                Some(image) => Preview::Ready(Box::new(self.renderer.new_protocol(*image))),
                None => Preview::Missing,
            };
            self.previews.insert(url, entry);
        }
    }

    pub(crate) fn cached_detail(&self) -> Option<&crate::catalog::Anime> {
        match self.detail.as_ref()? {
            AnimeDetail::Cached(anime) => Some(anime),
            AnimeDetail::Live(_) => None,
        }
    }

    pub(crate) fn live_detail(&self) -> Option<&crate::live::LiveAnime> {
        match self.detail.as_ref()? {
            AnimeDetail::Live(anime) => Some(anime),
            AnimeDetail::Cached(_) => None,
        }
    }

    /// Keeps the detail scroll inside the rendered content; the renderer knows
    /// the wrapped height, the key handler does not.
    pub(crate) fn clamp_detail_scroll(&mut self, max: u16) {
        self.detail_scroll = self.detail_scroll.min(max);
    }
}

/// The released column as something the date sort can compare. Sources write
/// the dash when they have nothing, and a row with nothing sorts last rather
/// than under a literal `—`.
fn sortable_date(released: &str) -> Option<String> {
    let released = released.trim();
    (!released.is_empty() && released != EMPTY).then(|| released.to_string())
}

/// Where a season falls in its year. `/seasons` names them, and names put fall
/// before winter, which is not what sorting by date is being asked for.
fn season_month(season: &SeasonRef) -> u32 {
    match season.season.trim().to_lowercase().as_str() {
        "winter" => 1,
        "spring" => 4,
        "summer" => 7,
        "fall" | "autumn" => 10,
        _ => 0,
    }
}

fn move_index(index: &mut usize, count: usize, direction: isize) {
    if count == 0 {
        *index = 0;
    } else if direction < 0 {
        *index = index.saturating_sub(direction.unsigned_abs());
    } else {
        *index = (*index + direction as usize).min(count - 1);
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, App, Origin, Renderer, Screen, move_index};
    use crate::mode::Mode;
    use crate::playback::{Playback, TrackPrefs};
    use crate::source::{AnimeSummary, Source};
    use crossterm::event::{KeyCode, KeyEvent};

    /// Built from the repo's own catalog in cached mode, so nothing here needs
    /// the network.
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
        App::new(source, playback, Renderer::halfblocks())
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    #[tokio::test]
    async fn p_cycles_the_host_streams_are_resolved_from() {
        let mut app = app().await;
        assert_eq!(app.provider_name(), "zokoanime");
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.provider_name(), "megavid");
        // And wraps, so the key alone gets back to where it started.
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(app.provider_name(), "zokoanime");
    }

    #[tokio::test]
    async fn a_toggles_autoswitch_which_starts_on() {
        let mut app = app().await;
        assert!(app.autoswitch());
        press(&mut app, KeyCode::Char('a'));
        assert!(!app.autoswitch());
        press(&mut app, KeyCode::Char('a'));
        assert!(app.autoswitch());
    }

    /// `p` and `a` are ordinary letters, so they have to reach the query rather
    /// than silently changing how playback resolves while a search is typed.
    #[tokio::test]
    async fn provider_keys_type_into_a_search_instead_of_taking_effect() {
        let mut app = app().await;
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.search_input, "pa");
        assert_eq!(app.provider_name(), "zokoanime");
        assert!(app.autoswitch());
    }

    fn summary(mal_id: u32, title: &str, released: &str) -> AnimeSummary {
        AnimeSummary {
            origin: Origin::Live(mal_id),
            title: title.to_string(),
            kind: "TV".into(),
            status: "Finished".into(),
            score: None,
            episodes: None,
            released: released.to_string(),
            note: None,
        }
    }

    /// A listing as a source hands one over: unsorted, and with one row the
    /// source has no date for.
    fn listing(app: &mut App) {
        app.listing = vec![
            summary(1, "Cowboy Bebop", "1998-04-03"),
            summary(2, "attack on titan", "2013-04-07"),
            summary(3, "Bocchi the Rock!", "—"),
        ];
        app.list_index = 0;
        app.screen = Screen::Listing;
    }

    fn titles(app: &App) -> Vec<String> {
        app.visible_listing()
            .iter()
            .map(|index| app.listing[*index].title.clone())
            .collect()
    }

    fn selected_title(app: &App) -> String {
        titles(app)[app.list_index].clone()
    }

    #[tokio::test]
    async fn n_orders_a_listing_by_name_and_d_by_date() {
        let mut app = app().await;
        listing(&mut app);
        assert_eq!(titles(&app)[0], "Cowboy Bebop");

        press(&mut app, KeyCode::Char('n'));
        assert_eq!(
            titles(&app),
            ["attack on titan", "Bocchi the Rock!", "Cowboy Bebop"]
        );

        press(&mut app, KeyCode::Char('d'));
        assert_eq!(
            titles(&app),
            ["attack on titan", "Cowboy Bebop", "Bocchi the Rock!"]
        );
        // The same key again is the only way to ask for oldest first.
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(
            titles(&app),
            ["Cowboy Bebop", "attack on titan", "Bocchi the Rock!"]
        );
    }

    /// Sorting rearranges the list under the highlight rather than moving the
    /// highlight, so Enter still opens what it was pointing at.
    #[tokio::test]
    async fn the_highlighted_title_survives_a_sort_and_is_the_one_opened() {
        let mut app = app().await;
        listing(&mut app);
        assert_eq!(selected_title(&app), "Cowboy Bebop");

        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.list_index, 2);
        assert_eq!(selected_title(&app), "Cowboy Bebop");

        press(&mut app, KeyCode::Enter);
        match app.take_pending() {
            Some(Action::Detail(Origin::Live(mal_id))) => assert_eq!(mal_id, 1),
            other => panic!("expected the Cowboy Bebop detail, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn f_narrows_the_listing_as_it_is_typed_and_esc_puts_it_back() {
        let mut app = app().await;
        listing(&mut app);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Char('B'));
        // Case folds both ways, and the match is anywhere in the title.
        assert_eq!(titles(&app), ["Cowboy Bebop", "Bocchi the Rock!"]);

        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(titles(&app), ["Bocchi the Rock!"]);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(titles(&app).len(), 2);

        // The first Esc abandons the filter; only the second leaves the screen.
        press(&mut app, KeyCode::Esc);
        assert_eq!(titles(&app).len(), 3);
        assert_eq!(app.screen, Screen::Listing);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.screen, Screen::Home);
    }

    /// The filter is a text field, so the letters bound to actions elsewhere
    /// have to reach it instead of quitting or changing how playback resolves.
    #[tokio::test]
    async fn a_filter_being_typed_takes_the_keys_bound_to_actions() {
        let mut app = app().await;
        listing(&mut app);
        press(&mut app, KeyCode::Char('f'));
        for key in ['a', 'q', 'p', 'n', 'd'] {
            assert!(app.handle_key(KeyEvent::from(KeyCode::Char(key))));
        }
        assert_eq!(app.screen, Screen::Listing);
        assert!(app.autoswitch());
        assert_eq!(app.provider_name(), "zokoanime");
        assert!(titles(&app).is_empty());

        // Enter keeps the filter and hands the keys back.
        press(&mut app, KeyCode::Enter);
        assert!(!app.filtering());
        assert!(titles(&app).is_empty());
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.screen, Screen::QuitConfirm);
    }

    /// A filter that hides everything must not leave Enter pointing at a row.
    #[tokio::test]
    async fn nothing_opens_while_the_filter_matches_nothing() {
        let mut app = app().await;
        listing(&mut app);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(app.take_pending().is_none());
    }

    /// A listing filtered down to one title has no bearing on the next one.
    #[tokio::test]
    async fn a_new_listing_arrives_unfiltered() {
        let mut app = app().await;
        listing(&mut app);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('n'));

        app.show_listing("Airing now", vec![summary(4, "Frieren", "2023-09-29")]);
        assert!(!app.filtering());
        assert_eq!(titles(&app), ["Frieren"]);
    }

    #[test]
    fn selection_stays_in_bounds() {
        let mut index = 0;
        move_index(&mut index, 2, -1);
        assert_eq!(index, 0);
        move_index(&mut index, 2, 1);
        move_index(&mut index, 2, 1);
        assert_eq!(index, 1);
    }

    #[test]
    fn paging_clamps_to_the_ends() {
        let mut index = 0;
        move_index(&mut index, 5, 10);
        assert_eq!(index, 4);
        move_index(&mut index, 5, -10);
        assert_eq!(index, 0);
        move_index(&mut index, 0, 1);
        assert_eq!(index, 0);
    }
}
