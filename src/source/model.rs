//! View models shared by the CLI and the terminal UI, independent of whether a
//! row came from the local catalog or the Tenrai API.

use crate::catalog::Anime;
use crate::live::LiveAnime;
use crate::live::model::Recommendation;
use serde::{Deserialize, Serialize};

/// Identifies a title and, with it, how its detail view is loaded.
///
/// Serialized as `{"cached": "id"}` or `{"live": 123}` so the library can
/// reopen a starred row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    /// A record in the local Deeb catalog, keyed by its catalog id.
    Cached(String),
    /// A MyAnimeList id served by the Tenrai API.
    Live(u32),
}

impl Origin {
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live(_))
    }
}

/// One row in any listing screen. A starred row is kept whole in the library,
/// so the favourites screen draws the same columns without going back to the
/// source for them — which is why this is serializable.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AnimeSummary {
    pub origin: Origin,
    pub title: String,
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub episodes: Option<u32>,
    pub released: String,
    /// Extra context for the row, such as why a title was recommended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl AnimeSummary {
    pub fn from_cached(anime: &Anime) -> Self {
        Self {
            origin: Origin::Cached(anime.id.clone()),
            title: anime.title.clone(),
            kind: anime.kind.to_string(),
            status: anime.status.to_string(),
            score: None,
            episodes: (!anime.episodes.is_empty()).then_some(anime.episodes.len() as u32),
            released: anime
                .latest_release_at
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| EMPTY.to_string()),
            note: None,
        }
    }

    pub fn from_live(anime: &LiveAnime) -> Self {
        Self {
            origin: Origin::Live(anime.mal_id),
            title: anime.display_title().to_string(),
            kind: anime
                .media_type
                .clone()
                .unwrap_or_else(|| EMPTY.to_string()),
            status: short_status(anime.status.as_deref()),
            score: anime.score,
            episodes: anime.episodes,
            released: live_release(anime),
            note: None,
        }
    }

    /// Builds a row for the recommended title, carrying the title it was
    /// recommended from as the row's note.
    pub fn from_recommendation(recommendation: &Recommendation) -> Option<Self> {
        let suggestion = recommendation.suggestion()?;
        let note = recommendation
            .source()
            .filter(|source| source.mal_id != suggestion.mal_id)
            .map(|source| format!("because you watched {}", source.title));
        Some(Self {
            origin: Origin::Live(suggestion.mal_id),
            title: suggestion.title.clone(),
            kind: EMPTY.to_string(),
            status: EMPTY.to_string(),
            score: None,
            episodes: None,
            released: EMPTY.to_string(),
            note,
        })
    }

    /// Used to keep the same title from appearing twice when hybrid mode merges
    /// catalog rows with API rows.
    pub fn dedupe_key(&self) -> String {
        self.title.trim().to_lowercase()
    }

    /// True when the source gave nothing but a title — `/recommendations`
    /// returns only ids and titles, so those rows skip the empty columns.
    pub fn is_bare(&self) -> bool {
        self.score.is_none()
            && self.episodes.is_none()
            && self.kind == EMPTY
            && self.status == EMPTY
            && self.released == EMPTY
    }

    /// The listing row rendered by both frontends, aligned to [`Self::header`].
    pub fn row(&self) -> String {
        self.row_marked(false)
    }

    /// The same row, with a star after the title when it is one of the user's
    /// favourites.
    pub fn row_marked(&self, favourite: bool) -> String {
        // A title-only row has no columns to line up with, so the star simply
        // follows the title rather than eating into it.
        if self.is_bare() {
            return match favourite {
                true => format!("{} ★", self.title),
                false => self.title.clone(),
            };
        }
        format!(
            "{:<42.42}  {:<8.8}  {:<9.9}  {:>5}  {:>4}  {}",
            self.title_cell(favourite),
            self.kind,
            self.status,
            self.score
                .map(|score| format!("{score:.2}"))
                .unwrap_or_else(|| EMPTY.to_string()),
            self.episodes
                .map(|count| count.to_string())
                .unwrap_or_else(|| EMPTY.to_string()),
            self.released
        )
    }

    /// The title column. A starred title gives up two of its own characters to
    /// the star rather than letting the column grow, so the rows around it still
    /// line up under [`Self::header`].
    fn title_cell(&self, favourite: bool) -> String {
        if !favourite {
            return self.title.clone();
        }
        let mut title: String = self.title.chars().take(TITLE_WIDTH - 2).collect();
        title.push_str(" ★");
        title
    }

    pub fn header() -> String {
        format!(
            "{:<42}  {:<8}  {:<9}  {:>5}  {:>4}  {}",
            "TITLE", "TYPE", "STATUS", "SCORE", "EPS", "RELEASED"
        )
    }
}

/// The detail behind a selected row.
#[derive(Clone, Debug)]
pub enum AnimeDetail {
    Cached(Box<Anime>),
    Live(Box<LiveAnime>),
}

/// A year and season pair from `/seasons`, e.g. `2023` / `fall`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeasonRef {
    pub year: u32,
    pub season: String,
}

impl SeasonRef {
    pub fn label(&self) -> String {
        let mut season = self.season.clone();
        if let Some(first) = season.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        format!("{season} {}", self.year)
    }
}

/// The dash shown wherever a source has nothing for a column.
pub const EMPTY: &str = "—";

/// The width of the title column, which the star has to fit inside.
const TITLE_WIDTH: usize = 42;

/// The API's status wording is long; these are the column-width equivalents.
fn short_status(status: Option<&str>) -> String {
    match status.map(str::trim) {
        Some("Currently Airing") => "Airing".to_string(),
        Some("Finished Airing") => "Finished".to_string(),
        Some("Not yet aired") => "Upcoming".to_string(),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => EMPTY.to_string(),
    }
}

/// Prefers the exact air date, then the broadcast season, then nothing.
fn live_release(anime: &LiveAnime) -> String {
    anime
        .aired
        .as_ref()
        .and_then(|aired| aired.from)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .or_else(|| anime.season_label())
        .unwrap_or_else(|| EMPTY.to_string())
}

#[cfg(test)]
mod tests {
    use super::{AnimeSummary, Origin, SeasonRef, short_status};
    use crate::live::LiveAnime;

    fn live(mal_id: u32, title: &str) -> LiveAnime {
        LiveAnime {
            mal_id,
            title: title.to_string(),
            status: Some("Currently Airing".into()),
            score: Some(8.5),
            episodes: Some(12),
            season: Some("summer".into()),
            year: Some(2026),
            ..LiveAnime::default()
        }
    }

    #[test]
    fn live_rows_fall_back_to_the_season_when_no_air_date_exists() {
        let summary = AnimeSummary::from_live(&live(1, "Example"));
        assert_eq!(summary.origin, Origin::Live(1));
        assert_eq!(summary.released, "summer 2026");
        assert_eq!(summary.status, "Airing");
        assert!(summary.row().contains("8.50"));
    }

    #[test]
    fn missing_values_render_as_a_dash() {
        let summary = AnimeSummary::from_live(&LiveAnime {
            mal_id: 2,
            title: "Bare".into(),
            ..LiveAnime::default()
        });
        assert_eq!(summary.released, "—");
        assert_eq!(short_status(None), "—");
    }

    #[test]
    fn season_labels_are_capitalised() {
        let season = SeasonRef {
            year: 2023,
            season: "fall".into(),
        };
        assert_eq!(season.label(), "Fall 2023");
    }
}
