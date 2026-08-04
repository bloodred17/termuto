use crate::catalog::AnimeKind;
use crate::mode::Mode;
use crate::playback::{Playback, StreamRequest};
use crate::source::{AnimeDetail, AnimeSummary, Origin, SeasonRef, Source};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// How many rows a listing screen requests. Deliberately modest: every live
/// screen is one or two API pages.
const LISTING_LIMIT: usize = 50;

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
    pub(crate) search_input: String,
    pub(crate) search_focus: SearchFocus,
    pub(crate) search_submitted: Option<String>,
    pub(crate) detail: Option<AnimeDetail>,
    pub(crate) detail_scroll: u16,
    pub(crate) error: Option<String>,
    /// What the last successful play handed to the player, shown on
    /// [`Screen::Playing`].
    pub(crate) now_playing: Option<Vec<String>>,
    pending: Option<Action>,
    pub(crate) loading: Option<String>,
}

impl App {
    pub(crate) fn new(source: Source, playback: Playback) -> Self {
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
            search_input: String::new(),
            search_focus: SearchFocus::Query,
            search_submitted: None,
            detail: None,
            detail_scroll: 0,
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

    /// The screen whose contents are on show. The quit prompt, the now-playing
    /// panel, and errors are overlays, so the screen underneath keeps drawing.
    pub(crate) fn display_screen(&self) -> Screen {
        match self.screen {
            Screen::QuitConfirm | Screen::Error | Screen::Playing => self
                .history
                .last()
                .copied()
                .filter(|screen| {
                    !matches!(screen, Screen::QuitConfirm | Screen::Error | Screen::Playing)
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

    fn show_listing(&mut self, title: &str, rows: Vec<AnimeSummary>) {
        self.listing_title = title.to_string();
        self.listing = rows;
        self.list_index = 0;
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
            Screen::Search => return self.handle_search_key(key),
            _ => {}
        }

        match key.code {
            // `q` is reserved for the query text while Search has focus, but
            // quits from every other screen.
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Esc => self.go_back(),
            KeyCode::Char('/') => self.enter_search(),
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
        self.search_focus = SearchFocus::Query;
        self.enter(Screen::Search);
    }

    fn move_selection(&mut self, direction: isize) {
        match self.screen {
            Screen::Home => {
                let count = self.home_entries().len();
                move_index(&mut self.home_index, count, direction);
            }
            Screen::Listing | Screen::Search => {
                let count = self.listing.len();
                move_index(&mut self.list_index, count, direction);
            }
            Screen::SeasonPicker => {
                let count = self.seasons.len();
                move_index(&mut self.season_index, count, direction);
            }
            Screen::Episodes => {
                let count = self.cached_detail().map_or(0, |anime| anime.episodes.len());
                move_index(&mut self.episode_index, count, direction);
            }
            Screen::LiveEpisodes => {
                let count = self.live_episode_count();
                move_index(&mut self.episode_index, count, direction);
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
            Screen::Listing | Screen::Search => {
                if let Some(row) = self.listing.get(self.list_index) {
                    self.queue(Action::Detail(row.origin.clone()));
                }
            }
            Screen::SeasonPicker => {
                if let Some(season) = self.seasons.get(self.season_index).cloned() {
                    self.queue(Action::Season(season));
                }
            }
            // A catalog series plays the highlighted episode; a movie has none.
            Screen::Episodes => {
                let Some(anime) = self.cached_detail() else {
                    return;
                };
                let Some(episode) = anime.episodes.get(self.episode_index) else {
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
                let request =
                    self.playback
                        .request(Origin::Cached(anime.id.clone()), anime.title.clone(), None);
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
                    self.episode_index = 0;
                    self.enter(Screen::LiveEpisodes);
                }
            }
            Screen::LiveEpisodes => {
                let Some(anime) = self.live_detail() else {
                    return;
                };
                let request = self.playback.request(
                    Origin::Live(anime.mal_id),
                    anime.display_title().to_string(),
                    Some(self.episode_index as u32 + 1),
                );
                self.queue(Action::Play(request));
            }
            _ => {}
        }
    }

    /// The API reports how many episodes a title has rather than listing them,
    /// so the picker counts them out. An unknown count means one playable part.
    pub(crate) fn live_episode_count(&self) -> usize {
        self.live_detail()
            .map_or(0, |anime| anime.episodes.unwrap_or(1).max(1) as usize)
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
    use super::move_index;

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
