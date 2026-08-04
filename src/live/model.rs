//! Typed subsets of the Tenrai (MyAnimeList-shaped) API payloads.
//!
//! Only the fields the CLI and terminal UI render are modelled; every optional
//! field is tolerated so an API addition or a sparsely populated title cannot
//! fail a whole response.

use chrono::{DateTime, TimeZone, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};

/// A list response: `{ "pagination": {...}, "data": [...] }`.
#[derive(Clone, Debug, Deserialize)]
pub struct Page<T> {
    #[serde(default)]
    pub pagination: Option<Pagination>,
    #[serde(
        default = "Vec::new",
        bound(deserialize = "T: DeserializeOwned"),
        deserialize_with = "lenient_items"
    )]
    pub data: Vec<T>,
}

/// A single-object response: `{ "data": {...} }`.
#[derive(Clone, Debug, Deserialize)]
pub struct Envelope<T> {
    pub data: T,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Pagination {
    #[serde(default)]
    pub has_next_page: bool,
    #[serde(default)]
    pub current_page: Option<u32>,
    #[serde(default)]
    pub last_visible_page: Option<u32>,
}

/// One title. The list endpoints and `/anime/{id}/full` share this shape; the
/// extra `full` fields simply arrive as `None` on list responses.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LiveAnime {
    pub mal_id: u32,
    #[serde(default)]
    pub url: Option<String>,
    pub title: String,
    #[serde(default)]
    pub title_english: Option<String>,
    #[serde(default)]
    pub title_japanese: Option<String>,
    #[serde(default)]
    pub title_synonyms: Vec<String>,
    #[serde(rename = "type", default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub episodes: Option<u32>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub airing: bool,
    #[serde(default)]
    pub aired: Option<Aired>,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub score: Option<f64>,
    #[serde(default)]
    pub scored_by: Option<u64>,
    #[serde(default)]
    pub rank: Option<u32>,
    #[serde(default)]
    pub popularity: Option<u32>,
    #[serde(default)]
    pub members: Option<u64>,
    #[serde(default)]
    pub favorites: Option<u64>,
    #[serde(default)]
    pub synopsis: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub season: Option<String>,
    #[serde(default)]
    pub year: Option<u32>,
    #[serde(default)]
    pub broadcast: Option<Broadcast>,
    #[serde(default)]
    pub studios: Vec<Named>,
    #[serde(default)]
    pub producers: Vec<Named>,
    #[serde(default)]
    pub licensors: Vec<Named>,
    #[serde(default)]
    pub genres: Vec<Named>,
    #[serde(default)]
    pub themes: Vec<Named>,
    #[serde(default)]
    pub demographics: Vec<Named>,
    #[serde(rename = "theme", default)]
    pub songs: Option<Songs>,
    #[serde(default)]
    pub streaming: Vec<Link>,
    #[serde(default)]
    pub external: Vec<Link>,
}

impl LiveAnime {
    /// The English title when the API has one, otherwise the romanised default.
    pub fn display_title(&self) -> &str {
        self.title_english
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&self.title)
    }

    pub fn is_movie(&self) -> bool {
        self.media_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("movie"))
    }

    /// `fall 2023`, falling back to the aired year when the season is unset.
    pub fn season_label(&self) -> Option<String> {
        match (self.season.as_deref(), self.year) {
            (Some(season), Some(year)) => Some(format!("{season} {year}")),
            (Some(season), None) => Some(season.to_string()),
            (None, Some(year)) => Some(year.to_string()),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Aired {
    #[serde(default, deserialize_with = "lenient_date")]
    pub from: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "lenient_date")]
    pub to: Option<DateTime<Utc>>,
    /// The API's own human-readable range, e.g. `Sep 29, 2023 to Mar 22, 2024`.
    #[serde(default)]
    pub string: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Broadcast {
    #[serde(default)]
    pub string: Option<String>,
}

/// A producer, studio, genre, theme, or demographic reference.
#[derive(Clone, Debug, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Songs {
    #[serde(default)]
    pub openings: Vec<String>,
    #[serde(default)]
    pub endings: Vec<String>,
}

/// One user recommendation: `entry[0]` is the title watched, `entry[1]` the
/// suggestion made from it.
#[derive(Clone, Debug, Deserialize)]
pub struct Recommendation {
    #[serde(default)]
    pub entry: Vec<RecommendationEntry>,
    #[serde(default)]
    pub content: Option<String>,
}

impl Recommendation {
    pub fn source(&self) -> Option<&RecommendationEntry> {
        self.entry.first()
    }

    pub fn suggestion(&self) -> Option<&RecommendationEntry> {
        self.entry.get(1).or_else(|| self.entry.first())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RecommendationEntry {
    pub mal_id: u32,
    #[serde(default)]
    pub title: String,
}

/// One row of `/seasons`: the seasons the API holds titles for in a given year.
#[derive(Clone, Debug, Deserialize)]
pub struct SeasonYear {
    pub year: u32,
    #[serde(default)]
    pub seasons: Vec<String>,
}

/// Keeps one unusable entry from discarding a whole page. The API is a moving
/// target, and a single odd record should cost that record, not the screen.
fn lenient_items<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect())
}

/// Air dates are not always complete: an unannounced premiere arrives as
/// `2026-10`, or even `2026`, which no RFC 3339 parser accepts.
fn lenient_date<'de, D>(deserializer: D) -> Result<Option<DateTime<Utc>>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(parse_partial_date))
}

fn parse_partial_date(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }

    // Fall back to the leading `year[-month[-day]]`, defaulting to the first of
    // the period, which is how the API's own date strings read it.
    let date = value.split('T').next().unwrap_or(value);
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next().and_then(|part| part.parse().ok()).unwrap_or(1);
    let day = parts.next().and_then(|part| part.parse().ok()).unwrap_or(1);
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).single()
}

/// The API's error envelope, used to surface a useful message on non-2xx.
#[derive(Clone, Debug, Deserialize)]
pub struct ApiError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{LiveAnime, Page, parse_partial_date};

    #[test]
    fn list_payloads_tolerate_missing_optional_fields() {
        let page: Page<LiveAnime> = serde_json::from_str(
            r#"{"pagination":{"has_next_page":true},"data":[{"mal_id":1,"title":"Cowboy Bebop"}]}"#,
        )
        .expect("minimal payload deserializes");
        assert_eq!(page.data[0].display_title(), "Cowboy Bebop");
        assert!(page.pagination.expect("pagination").has_next_page);
    }

    #[test]
    fn english_title_is_preferred_when_present() {
        let anime: LiveAnime = serde_json::from_str(
            r#"{"mal_id":1,"title":"Sousou no Frieren","title_english":"Frieren","season":"fall","year":2023}"#,
        )
        .expect("payload deserializes");
        assert_eq!(anime.display_title(), "Frieren");
        assert_eq!(anime.season_label().as_deref(), Some("fall 2023"));
    }

    #[test]
    fn unannounced_premieres_keep_their_partial_air_date() {
        // A title whose premiere is only known to the month, as `/seasons`
        // returns it. An RFC 3339 parser alone rejects this outright.
        let anime: LiveAnime = serde_json::from_str(
            r#"{"mal_id":1,"title":"Upcoming","aired":{"from":"2026-10","to":null,"string":"Oct 2026 to ?"}}"#,
        )
        .expect("partial dates are accepted");
        let from = anime.aired.expect("aired").from.expect("from");
        assert_eq!(from.format("%Y-%m-%d").to_string(), "2026-10-01");
        assert_eq!(parse_partial_date("2011"), parse_partial_date("2011-01-01"));
        assert!(parse_partial_date("not a date").is_none());
    }

    #[test]
    fn one_unusable_entry_does_not_discard_the_page() {
        let page: Page<LiveAnime> = serde_json::from_str(
            r#"{"data":[{"mal_id":1,"title":"Good"},{"title":"No id"},{"mal_id":3,"title":"Also good"}]}"#,
        )
        .expect("page deserializes");
        assert_eq!(page.data.len(), 2);
        assert_eq!(page.data[1].title, "Also good");
    }
}
