use super::model::{ApiError, Envelope, LiveAnime, LiveEpisode, Page, Recommendation, SeasonYear};
use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use std::env;
use std::time::Duration;

/// The public Tenrai v1 base. Every path below is joined onto it.
pub const DEFAULT_BASE_URL: &str = "https://api.tenrai.org/v1";

/// Overrides [`DEFAULT_BASE_URL`], mainly so the API can be pointed at a mirror.
pub const BASE_URL_ENV: &str = "TERMUTO_API_BASE";

/// The API rejects `limit` above 50, so larger requests are paged.
const MAX_PAGE_SIZE: usize = 50;

/// An upper bound on paging, so a caller asking for "everything" cannot walk
/// tens of thousands of pages.
const MAX_COLLECTED: usize = 200;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A thin, typed wrapper over the Tenrai REST API.
#[derive(Clone, Debug)]
pub struct LiveClient {
    http: reqwest::Client,
    base_url: String,
}

impl LiveClient {
    pub fn new() -> Result<Self> {
        let base_url = env::var(BASE_URL_ENV)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::with_base_url(base_url)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("termuto-poc/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("Could not build the HTTP client for the Tenrai API")?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `GET /top/anime` — the highest ranked titles.
    pub async fn top_anime(&self, limit: usize) -> Result<Vec<LiveAnime>> {
        self.collect("top/anime", &[], limit).await
    }

    /// `GET /top/anime?filter=airing` — the highest ranked titles still airing.
    pub async fn top_airing(&self, limit: usize) -> Result<Vec<LiveAnime>> {
        self.collect("top/anime", &[("filter", "airing".into())], limit)
            .await
    }

    /// `GET /seasons/now` — the season currently broadcasting.
    pub async fn current_season(&self, limit: usize) -> Result<Vec<LiveAnime>> {
        self.collect("seasons/now", &score_first(), limit).await
    }

    /// `GET /seasons/{year}/{season}`.
    pub async fn season(&self, year: u32, season: &str, limit: usize) -> Result<Vec<LiveAnime>> {
        let path = format!("seasons/{year}/{}", season.trim().to_lowercase());
        self.collect(&path, &score_first(), limit).await
    }

    /// `GET /seasons` — every year the API holds seasonal titles for.
    pub async fn seasons_index(&self) -> Result<Vec<SeasonYear>> {
        let page: Page<SeasonYear> = self.get("seasons", &[]).await?;
        Ok(page.data)
    }

    /// `GET /recommendations/anime` — recent user-submitted recommendations.
    pub async fn recommendations(&self, limit: usize) -> Result<Vec<Recommendation>> {
        self.collect("recommendations/anime", &[], limit).await
    }

    /// `GET /anime?q=…` — full-text search across the catalogue.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<LiveAnime>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.collect("anime", &[("q", query.to_string())], limit)
            .await
    }

    /// `GET /anime/{id}/full` — one title with its relations, songs, and links.
    pub async fn anime_full(&self, id: u32) -> Result<LiveAnime> {
        let envelope: Envelope<LiveAnime> = self.get(&format!("anime/{id}/full"), &[]).await?;
        Ok(envelope.data)
    }

    /// `GET /anime/{id}/episodes` — the episode list, with per-episode titles,
    /// air dates, scores, stills, and synopses.
    pub async fn anime_episodes(&self, id: u32, limit: usize) -> Result<Vec<LiveEpisode>> {
        self.collect(&format!("anime/{id}/episodes"), &[], limit)
            .await
    }

    /// Fetches an arbitrary URL as bytes. Episode stills are served from the
    /// streaming sites' own CDNs, so this deliberately does not join `base_url`.
    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("Could not reach {url}"))?
            .error_for_status()
            .with_context(|| format!("{url} could not be fetched"))?;
        Ok(response
            .bytes()
            .await
            .with_context(|| format!("Could not read the response from {url}"))?
            .to_vec())
    }

    /// Requests successive pages until `limit` items are gathered or the API
    /// reports no further page.
    async fn collect<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        limit: usize,
    ) -> Result<Vec<T>> {
        let limit = limit.min(MAX_COLLECTED);
        let mut collected: Vec<T> = Vec::new();
        let mut page = 1_u32;

        while collected.len() < limit {
            let remaining = limit - collected.len();
            let mut paged: Vec<(&str, String)> = query.to_vec();
            paged.push(("limit", remaining.min(MAX_PAGE_SIZE).to_string()));
            paged.push(("page", page.to_string()));

            let response: Page<T> = self.get(path, &paged).await?;
            let has_next = response
                .pagination
                .as_ref()
                .is_some_and(|pagination| pagination.has_next_page);
            if response.data.is_empty() {
                break;
            }
            collected.extend(response.data);
            if !has_next {
                break;
            }
            page += 1;
        }

        collected.truncate(limit);
        Ok(collected)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let url = format!("{}/{}", self.base_url, path.trim_start_matches('/'));
        let response = self
            .http
            .get(&url)
            .query(query)
            .send()
            .await
            .with_context(|| format!("Could not reach the Tenrai API at {url}"))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .with_context(|| format!("Could not read the Tenrai API response from {url}"))?;

        if !status.is_success() {
            bail!("Tenrai API request failed ({status}): {}", api_message(&body));
        }

        serde_json::from_str(&body)
            .with_context(|| format!("Could not parse the Tenrai API response from {url}"))
    }
}

/// Seasonal listings are chronological by default; ranking them puts the
/// notable titles of a season first.
fn score_first() -> Vec<(&'static str, String)> {
    vec![("order_by", "score".into()), ("sort", "desc".into())]
}

/// Prefers the API's own error text over a raw body dump.
fn api_message(body: &str) -> String {
    serde_json::from_str::<ApiError>(body)
        .ok()
        .and_then(|error| error.message.or(error.error))
        .unwrap_or_else(|| body.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_BASE_URL, LiveClient, api_message};

    #[test]
    fn base_url_loses_its_trailing_slash() {
        let client = LiveClient::with_base_url("https://example.test/v1/").expect("client builds");
        assert_eq!(client.base_url(), "https://example.test/v1");
        assert!(DEFAULT_BASE_URL.ends_with("/v1"));
    }

    #[test]
    fn error_bodies_surface_the_api_message() {
        let body = r#"{"status":404,"message":"Anime with ID 1 not found.","error":"nope"}"#;
        assert_eq!(api_message(body), "Anime with ID 1 not found.");
        assert_eq!(api_message("not json"), "not json");
    }
}
