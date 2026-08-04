//! The single data entry point for both frontends.
//!
//! [`Source`] hides which mode is active: `cached` answers from the local Deeb
//! catalog, `live` from the Tenrai API, and `hybrid` from both — local rows
//! first, API rows appended, and a failed API call degrading to whatever the
//! catalog could answer instead of failing the screen.

pub mod model;

use crate::catalog::CatalogRepository;
use crate::live::{LiveClient, SeasonYear};
use crate::mode::{MODE_ENV, Mode};
use anyhow::{Error, Result, bail};
use std::collections::HashSet;
use std::path::Path;

pub use model::{AnimeDetail, AnimeSummary, Origin, SeasonRef};

#[derive(Clone, Debug)]
pub struct Source {
    mode: Mode,
    catalog: Option<CatalogRepository>,
    live: Option<LiveClient>,
    /// Why the catalog is unavailable in `hybrid` mode, if it is. `cached` mode
    /// fails to open instead of recording this.
    catalog_issue: Option<String>,
}

impl Source {
    pub async fn open(mode: Mode, catalog_path: impl AsRef<Path>) -> Result<Self> {
        let mut catalog_issue = None;
        let catalog = if mode.uses_cache() {
            match CatalogRepository::open(catalog_path.as_ref()).await {
                Ok(repository) => Some(repository),
                Err(error) if mode.requires_catalog() => return Err(error),
                // Hybrid keeps working from the API alone; the reason is kept
                // so a frontend can mention it rather than silently degrading.
                Err(error) => {
                    catalog_issue = Some(first_line(&error));
                    None
                }
            }
        } else {
            None
        };

        let live = mode.uses_live().then(LiveClient::new).transpose()?;

        Ok(Self {
            mode,
            catalog,
            live,
            catalog_issue,
        })
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn catalog_issue(&self) -> Option<&str> {
        self.catalog_issue.as_deref()
    }

    /// Whether the API-only listings (top, seasonal, recommendations) can be served.
    pub fn supports_live_listings(&self) -> bool {
        self.live.is_some()
    }

    /// Highest ranked titles. API only.
    pub async fn top(&self, limit: usize) -> Result<Vec<AnimeSummary>> {
        let live = self.require_live("Top anime")?;
        Ok(summaries(live.top_anime(limit).await?))
    }

    /// The season currently broadcasting. API only.
    pub async fn current_season(&self, limit: usize) -> Result<Vec<AnimeSummary>> {
        let live = self.require_live("Seasonal anime")?;
        Ok(summaries(live.current_season(limit).await?))
    }

    /// One past or future season. API only.
    pub async fn season(&self, season: &SeasonRef, limit: usize) -> Result<Vec<AnimeSummary>> {
        let live = self.require_live("Seasonal anime")?;
        Ok(summaries(
            live.season(season.year, &season.season, limit).await?,
        ))
    }

    /// Every year and season the API holds titles for, newest first.
    pub async fn seasons_index(&self) -> Result<Vec<SeasonRef>> {
        let live = self.require_live("Seasonal anime")?;
        let mut years: Vec<SeasonYear> = live.seasons_index().await?;
        years.sort_by_key(|entry| std::cmp::Reverse(entry.year));
        Ok(years
            .into_iter()
            .flat_map(|entry| {
                let year = entry.year;
                order_seasons(entry.seasons)
                    .into_iter()
                    .map(move |season| SeasonRef { year, season })
            })
            .collect())
    }

    /// Recent user recommendations. API only.
    pub async fn recommendations(&self, limit: usize) -> Result<Vec<AnimeSummary>> {
        let live = self.require_live("Recommendations")?;
        Ok(live
            .recommendations(limit)
            .await?
            .iter()
            .filter_map(AnimeSummary::from_recommendation)
            .collect())
    }

    /// Newest titles: the catalog's newest releases, the API's current season.
    pub async fn latest(&self, limit: usize) -> Result<Vec<AnimeSummary>> {
        let mut rows = Vec::new();
        let mut failure = None;
        if let Some(catalog) = &self.catalog {
            rows.extend(summaries_from_cached(catalog.latest(limit).await?));
        }
        if let Some(live) = &self.live {
            match live.current_season(limit).await {
                Ok(anime) => rows.extend(summaries(anime)),
                Err(error) => failure = Some(error),
            }
        }
        finish(rows, failure, limit)
    }

    /// Still-airing titles: the catalog's ongoing rows, the API's airing ranking.
    pub async fn ongoing(&self, limit: usize) -> Result<Vec<AnimeSummary>> {
        let mut rows = Vec::new();
        let mut failure = None;
        if let Some(catalog) = &self.catalog {
            rows.extend(summaries_from_cached(catalog.ongoing().await?));
        }
        if let Some(live) = &self.live {
            match live.top_airing(limit).await {
                Ok(anime) => rows.extend(summaries(anime)),
                Err(error) => failure = Some(error),
            }
        }
        finish(rows, failure, limit)
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<AnimeSummary>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut rows = Vec::new();
        let mut failure = None;
        if let Some(catalog) = &self.catalog {
            rows.extend(summaries_from_cached(catalog.search(query).await?));
        }
        if let Some(live) = &self.live {
            match live.search(query, limit).await {
                Ok(anime) => rows.extend(summaries(anime)),
                Err(error) => failure = Some(error),
            }
        }
        finish(rows, failure, limit)
    }

    /// Loads the full record behind a selected row from whichever source owns it.
    pub async fn detail(&self, origin: &Origin) -> Result<AnimeDetail> {
        match origin {
            Origin::Cached(id) => {
                let Some(catalog) = &self.catalog else {
                    bail!("The local catalog is not available in {} mode", self.mode);
                };
                match catalog.find_by_id(id).await? {
                    Some(anime) => Ok(AnimeDetail::Cached(Box::new(anime))),
                    None => bail!("The catalog no longer contains \"{id}\""),
                }
            }
            Origin::Live(id) => {
                let live = self.require_live("Anime details")?;
                Ok(AnimeDetail::Live(Box::new(live.anime_full(*id).await?)))
            }
        }
    }

    fn require_live(&self, what: &str) -> Result<&LiveClient> {
        match &self.live {
            Some(live) => Ok(live),
            None => bail!(
                "{what} needs the API. Run with --mode live (or set {MODE_ENV}=live); \
                 the current mode is {}.",
                self.mode
            ),
        }
    }
}

fn summaries(anime: Vec<crate::live::LiveAnime>) -> Vec<AnimeSummary> {
    anime.iter().map(AnimeSummary::from_live).collect()
}

fn summaries_from_cached(anime: Vec<crate::catalog::Anime>) -> Vec<AnimeSummary> {
    anime.iter().map(AnimeSummary::from_cached).collect()
}

/// Drops repeats introduced by merging two sources, applies the limit, and only
/// surfaces an API failure when it left nothing to show.
fn finish(rows: Vec<AnimeSummary>, failure: Option<Error>, limit: usize) -> Result<Vec<AnimeSummary>> {
    let mut seen = HashSet::new();
    let mut deduped: Vec<AnimeSummary> = rows
        .into_iter()
        .filter(|row| seen.insert(row.dedupe_key()))
        .collect();

    if deduped.is_empty() && let Some(error) = failure {
        return Err(error);
    }

    deduped.truncate(limit);
    Ok(deduped)
}

/// `/seasons` lists seasons in calendar order; browsing reads better with the
/// most recent season of each year first.
fn order_seasons(mut seasons: Vec<String>) -> Vec<String> {
    const CALENDAR: [&str; 4] = ["winter", "spring", "summer", "fall"];
    seasons.sort_by_key(|season| {
        let lowered = season.to_lowercase();
        std::cmp::Reverse(
            CALENDAR
                .iter()
                .position(|candidate| *candidate == lowered)
                .unwrap_or(usize::MAX),
        )
    });
    seasons
}

fn first_line(error: &Error) -> String {
    error
        .to_string()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{finish, order_seasons};
    use crate::live::LiveAnime;
    use crate::source::AnimeSummary;
    use anyhow::anyhow;

    fn summary(mal_id: u32, title: &str) -> AnimeSummary {
        AnimeSummary::from_live(&LiveAnime {
            mal_id,
            title: title.to_string(),
            ..LiveAnime::default()
        })
    }

    #[test]
    fn merged_rows_drop_repeats_and_respect_the_limit() {
        let rows = vec![summary(1, "Frieren"), summary(2, "frieren"), summary(3, "Bebop")];
        let merged = finish(rows, None, 10).expect("merge succeeds");
        assert_eq!(merged.len(), 2);
        assert_eq!(finish(vec![summary(1, "A")], None, 0).expect("limit").len(), 0);
    }

    #[test]
    fn an_api_failure_only_surfaces_when_nothing_else_was_found() {
        let error = || Some(anyhow!("offline"));
        assert!(finish(vec![summary(1, "Local")], error(), 10).is_ok());
        assert!(finish(Vec::new(), error(), 10).is_err());
    }

    #[test]
    fn seasons_are_listed_newest_first_within_a_year() {
        let ordered = order_seasons(vec!["winter".into(), "spring".into(), "fall".into()]);
        assert_eq!(ordered, vec!["fall", "spring", "winter"]);
    }
}
