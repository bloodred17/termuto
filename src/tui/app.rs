use crate::catalog::{Anime, AnimeKind, CatalogRepository};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Screen {
    Home,
    Latest,
    Ongoing,
    Search,
    Episodes,
    MovieDetail,
    PlaybackNotice,
    QuitConfirm,
}

pub(crate) struct App {
    repository: CatalogRepository,
    pub(crate) screen: Screen,
    pub(crate) previous_screen: Screen,
    pub(crate) detail_origin: Screen,
    pub(crate) quit_origin: Screen,
    pub(crate) home_index: usize,
    pub(crate) list_index: usize,
    pub(crate) episode_index: usize,
    pub(crate) latest: Vec<Anime>,
    pub(crate) ongoing: Vec<Anime>,
    pub(crate) search_results: Vec<Anime>,
    pub(crate) search_input: String,
    pub(crate) selected_anime: Option<Anime>,
}

impl App {
    pub(crate) fn new(repository: CatalogRepository) -> Self {
        Self {
            repository,
            screen: Screen::Home,
            previous_screen: Screen::Home,
            detail_origin: Screen::Home,
            quit_origin: Screen::Home,
            home_index: 0,
            list_index: 0,
            episode_index: 0,
            latest: Vec::new(),
            ongoing: Vec::new(),
            search_results: Vec::new(),
            search_input: String::new(),
            selected_anime: None,
        }
    }

    /// The screen whose contents are on show. The quit prompt is an overlay, so
    /// the screen it was raised from keeps rendering underneath it.
    pub(crate) fn display_screen(&self) -> Screen {
        match self.screen {
            Screen::QuitConfirm => self.quit_origin,
            screen => screen,
        }
    }

    pub(crate) fn current_items(&self) -> &[Anime] {
        match self.display_screen() {
            Screen::Latest => &self.latest,
            Screen::Ongoing => &self.ongoing,
            Screen::Search => &self.search_results,
            _ => &[],
        }
    }

    pub(crate) async fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }

        // The quit prompt swallows every other binding until it is answered.
        if self.screen == Screen::QuitConfirm {
            return Ok(self.handle_quit_confirm_key(key));
        }

        if self.screen == Screen::Search {
            return self.handle_search_key(key).await;
        }

        match key.code {
            // `q` is reserved for the query text while Search has focus, but quits
            // from every other screen so users are never forced through a menu path.
            KeyCode::Char('q') => {
                self.request_quit();
                Ok(true)
            }
            KeyCode::Esc => {
                self.go_back();
                Ok(true)
            }
            KeyCode::Char('/') => {
                self.enter_search();
                Ok(true)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Ok(true)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Ok(true)
            }
            KeyCode::Char('l') if self.screen == Screen::Home => {
                self.open_latest().await?;
                Ok(true)
            }
            KeyCode::Char('o') if self.screen == Screen::Home => {
                self.open_ongoing().await?;
                Ok(true)
            }
            KeyCode::Enter => self.select_current().await,
            _ => Ok(true),
        }
    }

    /// Returns `false` once the user confirms the quit; every other key either
    /// dismisses the prompt or is ignored.
    fn handle_quit_confirm_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => false,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.screen = self.quit_origin;
                true
            }
            _ => true,
        }
    }

    fn request_quit(&mut self) {
        self.quit_origin = self.screen;
        self.screen = Screen::QuitConfirm;
    }

    async fn handle_search_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.search_input.clear();
                self.search_results.clear();
                self.list_index = 0;
                self.screen = self.previous_screen;
                Ok(true)
            }
            KeyCode::Backspace => {
                self.search_input.pop();
                self.refresh_search().await?;
                Ok(true)
            }
            KeyCode::Up => {
                self.move_selection(-1);
                Ok(true)
            }
            KeyCode::Down => {
                self.move_selection(1);
                Ok(true)
            }
            KeyCode::Enter => self.select_current().await,
            KeyCode::Char(character) => {
                self.search_input.push(character);
                self.refresh_search().await?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    async fn open_latest(&mut self) -> Result<()> {
        self.latest = self.repository.latest(usize::MAX).await?;
        self.list_index = 0;
        self.screen = Screen::Latest;
        Ok(())
    }

    async fn open_ongoing(&mut self) -> Result<()> {
        self.ongoing = self.repository.ongoing().await?;
        self.list_index = 0;
        self.screen = Screen::Ongoing;
        Ok(())
    }

    fn enter_search(&mut self) {
        self.previous_screen = self.screen;
        self.search_input.clear();
        self.search_results.clear();
        self.list_index = 0;
        self.screen = Screen::Search;
    }

    async fn refresh_search(&mut self) -> Result<()> {
        self.search_results = self.repository.search(&self.search_input).await?;
        self.list_index = 0;
        Ok(())
    }

    fn move_selection(&mut self, direction: isize) {
        match self.screen {
            Screen::Home => move_index(&mut self.home_index, 4, direction),
            Screen::Latest | Screen::Ongoing | Screen::Search => {
                let count = self.current_items().len();
                move_index(&mut self.list_index, count, direction)
            }
            Screen::Episodes => {
                let count = self
                    .selected_anime
                    .as_ref()
                    .map_or(0, |anime| anime.episodes.len());
                move_index(&mut self.episode_index, count, direction);
            }
            _ => {}
        }
    }

    async fn select_current(&mut self) -> Result<bool> {
        match self.screen {
            Screen::Home => match self.home_index {
                0 => self.open_latest().await?,
                1 => self.open_ongoing().await?,
                2 => self.enter_search(),
                3 => self.request_quit(),
                _ => unreachable!("home selection is bounded"),
            },
            Screen::Latest | Screen::Ongoing | Screen::Search => {
                if let Some(anime) = self.current_items().get(self.list_index).cloned() {
                    self.detail_origin = self.screen;
                    self.selected_anime = Some(anime.clone());
                    self.episode_index = 0;
                    self.screen = match anime.kind {
                        AnimeKind::Series => Screen::Episodes,
                        AnimeKind::Movie => Screen::MovieDetail,
                    };
                }
            }
            Screen::Episodes => {
                self.previous_screen = Screen::Episodes;
                self.screen = Screen::PlaybackNotice;
            }
            Screen::MovieDetail => {
                self.previous_screen = Screen::MovieDetail;
                self.screen = Screen::PlaybackNotice;
            }
            Screen::PlaybackNotice => self.screen = self.previous_screen,
            // Answered by `handle_quit_confirm_key`, which runs before this.
            Screen::QuitConfirm => {}
        }
        Ok(true)
    }

    fn go_back(&mut self) {
        match self.screen {
            Screen::Home => {}
            Screen::Latest | Screen::Ongoing => self.screen = Screen::Home,
            Screen::Search => self.screen = self.previous_screen,
            Screen::Episodes | Screen::MovieDetail => self.screen = self.detail_origin,
            Screen::PlaybackNotice => self.screen = self.previous_screen,
            Screen::QuitConfirm => self.screen = self.quit_origin,
        }
    }
}

fn move_index(index: &mut usize, count: usize, direction: isize) {
    if count == 0 {
        *index = 0;
    } else if direction < 0 {
        *index = index.saturating_sub(1);
    } else {
        *index = (*index + 1).min(count - 1);
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
}
