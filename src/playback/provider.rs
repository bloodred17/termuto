//! Turning a selected title and episode into something a player can open.
//!
//! Providers are tried in order and the first one that recognises the request
//! wins. A provider that does not handle a request returns `Ok(None)` and the
//! chain moves on; only a provider that recognised the request and then broke
//! returns `Err`. Real providers scrape a host whose endpoints move, so each one
//! is a single self-contained implementation of [`StreamProvider`] — replacing a
//! dead extractor is a one-file change and never touches the frontends.

use super::prefs::{Audio, Quality, TrackPrefs};
use crate::catalog::{AnimeKind, CatalogRepository};
use crate::source::Origin;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::fmt;

/// What playback was asked for, before any provider has looked at it.
#[derive(Clone, Debug)]
pub struct StreamRequest {
    pub origin: Origin,
    /// Used for provider lookups that key off a title, and as the player's
    /// window title.
    pub title: String,
    /// `None` for a movie or a single-part title.
    pub episode: Option<u32>,
    pub prefs: TrackPrefs,
}

impl StreamRequest {
    /// How the request reads in a loading label or an error message.
    pub fn label(&self) -> String {
        match self.episode {
            Some(number) => format!("{} episode {number}", self.title),
            None => self.title.clone(),
        }
    }
}

/// A resolved, playable location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stream {
    /// A local path or a URL — the player accepts either.
    pub url: String,
    /// Which provider answered, shown to the user so a bad match is traceable.
    pub provider: String,
    /// Passed to the player as HTTP request headers. Embed hosts commonly reject
    /// a media request that arrives without the referer they issued it for.
    pub headers: Vec<(String, String)>,
    /// External subtitle tracks to side-load, if the provider supplies any.
    pub subtitles: Vec<String>,
    pub audio: Audio,
    /// What the provider actually served, which need not be what was asked for.
    pub quality: Option<String>,
}

impl fmt::Display for Stream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}", self.provider, self.audio)?;
        if let Some(quality) = &self.quality {
            write!(formatter, " {quality}p")?;
        }
        write!(formatter, ")")
    }
}

#[async_trait]
pub trait StreamProvider: Send + Sync + fmt::Debug {
    fn name(&self) -> &str;

    /// `Ok(None)` means "not a request I serve" and hands over to the next
    /// provider. `Err` means this provider owned the request and failed.
    async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>>;
}

/// The ordered list of providers consulted for one request.
#[derive(Debug)]
pub struct ProviderChain {
    providers: Vec<Box<dyn StreamProvider>>,
}

impl ProviderChain {
    pub fn new(providers: Vec<Box<dyn StreamProvider>>) -> Self {
        Self { providers }
    }

    /// The catalog answers for its own rows; the remote provider answers for
    /// everything else. Adding a real extractor means inserting it here.
    pub fn with_catalog(catalog: Option<CatalogRepository>) -> Self {
        Self::new(vec![
            Box::new(CatalogProvider { catalog }),
            Box::new(MockProvider),
        ])
    }

    pub fn names(&self) -> Vec<&str> {
        self.providers
            .iter()
            .map(|provider| provider.name())
            .collect()
    }

    /// Tries each provider in turn. A provider that fails does not end the
    /// attempt — its reason is kept and reported only if nothing later succeeds,
    /// so one dead extractor cannot block a working one behind it.
    pub async fn resolve(&self, request: &StreamRequest) -> Result<Stream> {
        let mut failures = Vec::new();
        for provider in &self.providers {
            match provider.resolve(request).await {
                Ok(Some(stream)) => return Ok(stream),
                Ok(None) => {}
                Err(error) => failures.push(format!("{}: {error:#}", provider.name())),
            }
        }

        if failures.is_empty() {
            bail!(
                "No provider could resolve {}. Providers tried: {}.",
                request.label(),
                self.names().join(", ")
            );
        }
        bail!(
            "No provider could resolve {}.\n{}",
            request.label(),
            failures.join("\n")
        );
    }
}

/// Plays what the local catalog points at: the `source` on an episode, or the
/// one on the title itself for a movie. Either may be a path or a URL.
#[derive(Debug)]
struct CatalogProvider {
    catalog: Option<CatalogRepository>,
}

#[async_trait]
impl StreamProvider for CatalogProvider {
    fn name(&self) -> &str {
        "catalog"
    }

    async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>> {
        let Origin::Cached(id) = &request.origin else {
            return Ok(None);
        };
        let Some(catalog) = &self.catalog else {
            return Ok(None);
        };
        let Some(anime) = catalog.find_by_id(id).await? else {
            bail!("The catalog no longer contains \"{id}\"");
        };

        let source = match (anime.kind, request.episode) {
            (AnimeKind::Movie, _) => anime.source.clone(),
            // An episode the catalog does not carry is not an error here — a
            // later provider may well have it. Whether the number is valid at
            // all is checked by the caller, which has the episode list.
            (AnimeKind::Series, Some(number)) => anime
                .episodes
                .iter()
                .find(|episode| episode.number == number)
                .and_then(|episode| episode.source.clone()),
            // A series with nothing selected plays from the beginning.
            (AnimeKind::Series, None) => anime
                .episodes
                .first()
                .and_then(|episode| episode.source.clone()),
        };

        // No `source` is not a catalog failure: the entry is metadata only, so
        // the next provider gets its turn.
        Ok(source.map(|url| Stream {
            url,
            provider: self.name().to_string(),
            headers: Vec::new(),
            subtitles: Vec::new(),
            audio: request.prefs.audio,
            quality: match &request.prefs.quality {
                Quality::Best => None,
                Quality::Exact(height) => Some(height.clone()),
            },
        }))
    }
}

/// Stands in for the per-host extractors, which are out of scope here. It
/// answers every request that reaches it with one fixed manifest, so the
/// resolve-and-play path is exercised end to end without a scraper.
#[derive(Debug)]
struct MockProvider;

/// The placeholder manifest served for any remote request.
const PLACEHOLDER_STREAM: &str =
    "https://hls2.aniwatchtv.uk/v/scxqicy/8sk32ez9st/20e7jag7ev/oedld66ntijsmh/1080/index.m3u8";

/// The site the placeholder manifest is served on behalf of.
const PLACEHOLDER_ORIGIN: &str = "https://zokoanime.video";

/// A browser user agent. The CDN serving these manifests rejects requests that
/// do not look like they came from the player page.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// The headers a manifest request has to carry to be served. Without them the
/// CDN answers 403 and the player exits without drawing a frame, so any provider
/// resolving an embedded stream must attach these alongside the URL.
fn embed_headers(origin: &str) -> Vec<(String, String)> {
    vec![
        ("Referer".to_string(), format!("{origin}/")),
        ("Origin".to_string(), origin.to_string()),
        ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
    ]
}

#[async_trait]
impl StreamProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>> {
        Ok(Some(Stream {
            url: PLACEHOLDER_STREAM.to_string(),
            provider: self.name().to_string(),
            headers: embed_headers(PLACEHOLDER_ORIGIN),
            subtitles: Vec::new(),
            audio: request.prefs.audio,
            quality: Some("1080".to_string()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{PLACEHOLDER_STREAM, ProviderChain, Stream, StreamProvider, StreamRequest};
    use crate::playback::prefs::{Audio, TrackPrefs};
    use crate::source::Origin;
    use anyhow::{Result, bail};
    use async_trait::async_trait;

    fn request(origin: Origin) -> StreamRequest {
        StreamRequest {
            origin,
            title: "Example".into(),
            episode: Some(3),
            prefs: TrackPrefs::default(),
        }
    }

    #[derive(Debug)]
    struct Broken;

    #[async_trait]
    impl StreamProvider for Broken {
        fn name(&self) -> &str {
            "broken"
        }
        async fn resolve(&self, _request: &StreamRequest) -> Result<Option<Stream>> {
            bail!("the host moved its endpoint");
        }
    }

    #[derive(Debug)]
    struct Silent;

    #[async_trait]
    impl StreamProvider for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        async fn resolve(&self, _request: &StreamRequest) -> Result<Option<Stream>> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn a_remote_row_resolves_through_the_mock_provider() {
        let chain = ProviderChain::with_catalog(None);
        let stream = chain
            .resolve(&request(Origin::Live(1)))
            .await
            .expect("the mock answers");
        assert_eq!(stream.url, PLACEHOLDER_STREAM);
        assert_eq!(stream.provider, "mock");
        assert_eq!(stream.audio, Audio::Sub);
    }

    /// The CDN answers 403 without these, and because the player is detached
    /// that failure is invisible — so the headers are asserted, not assumed.
    #[tokio::test]
    async fn an_embedded_stream_carries_the_headers_its_host_demands() {
        let chain = ProviderChain::with_catalog(None);
        let stream = chain
            .resolve(&request(Origin::Live(1)))
            .await
            .expect("the mock answers");
        let named = |name: &str| {
            stream
                .headers
                .iter()
                .find(|(header, _)| header == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(
            named("Referer").as_deref(),
            Some("https://zokoanime.video/")
        );
        assert_eq!(named("Origin").as_deref(), Some("https://zokoanime.video"));
        assert!(named("User-Agent").is_some_and(|agent| agent.starts_with("Mozilla/")));
    }

    #[tokio::test]
    async fn a_failing_provider_does_not_block_the_one_behind_it() {
        let chain = ProviderChain::new(vec![Box::new(Broken), Box::new(Silent)]);
        assert!(chain.resolve(&request(Origin::Live(1))).await.is_err());

        // The same broken provider, this time with a working one behind it.
        let chain = ProviderChain::new(vec![Box::new(Broken), Box::new(Working)]);
        let stream = chain
            .resolve(&request(Origin::Live(1)))
            .await
            .expect("the working provider answers");
        assert_eq!(stream.provider, "working");
    }

    #[derive(Debug)]
    struct Working;

    #[async_trait]
    impl StreamProvider for Working {
        fn name(&self) -> &str {
            "working"
        }
        async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>> {
            Ok(Some(Stream {
                url: "https://example.test/index.m3u8".into(),
                provider: self.name().to_string(),
                headers: Vec::new(),
                subtitles: Vec::new(),
                audio: request.prefs.audio,
                quality: None,
            }))
        }
    }

    #[tokio::test]
    async fn a_cached_row_with_no_catalog_falls_through_to_the_next_provider() {
        let chain = ProviderChain::with_catalog(None);
        let stream = chain
            .resolve(&request(Origin::Cached("solo-leveling".into())))
            .await
            .expect("falls through to the mock");
        assert_eq!(stream.provider, "mock");
    }

    #[tokio::test]
    async fn every_provider_failing_reports_each_reason() {
        let chain = ProviderChain::new(vec![Box::new(Broken), Box::new(Silent)]);
        let error = chain
            .resolve(&request(Origin::Live(1)))
            .await
            .expect_err("nothing resolves");
        let message = format!("{error:#}");
        assert!(message.contains("Example episode 3"), "{message}");
        assert!(message.contains("the host moved its endpoint"), "{message}");
    }

    #[tokio::test]
    async fn a_chain_that_recognises_nothing_names_what_it_tried() {
        let chain = ProviderChain::new(vec![Box::new(Silent)]);
        let error = chain
            .resolve(&request(Origin::Live(1)))
            .await
            .expect_err("nothing resolves");
        assert!(format!("{error:#}").contains("silent"));
    }
}
