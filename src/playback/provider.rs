//! Turning a selected title and episode into something a player can open.
//!
//! Providers are tried in order and the first one that recognises the request
//! wins. A provider that does not handle a request returns `Ok(None)` and the
//! chain moves on; only a provider that recognised the request and then broke
//! returns `Err`. Real providers scrape a host whose endpoints move, so each one
//! is a single self-contained implementation of [`StreamProvider`] — replacing a
//! dead extractor is a one-file change and never touches the frontends.

use super::prefs::{Audio, Quality, TrackPrefs};
use super::zoko::ZokoProvider;
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

    /// The catalog answers for its own rows; ZokoAnime answers for the API rows,
    /// which carry the MyAnimeList id it is addressed by. Adding another
    /// extractor means inserting it here.
    pub fn with_catalog(catalog: Option<CatalogRepository>) -> Result<Self> {
        Ok(Self::new(vec![
            Box::new(CatalogProvider { catalog }),
            Box::new(ZokoProvider::new()?),
        ]))
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

#[cfg(test)]
mod tests {
    use super::{ProviderChain, Stream, StreamProvider, StreamRequest};
    use crate::playback::prefs::TrackPrefs;
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

    /// The default chain is asserted here rather than exercised: resolving an
    /// API row means a request to the live host, which a unit test must not make.
    #[test]
    fn the_catalog_is_tried_before_the_remote_extractor() {
        let chain = ProviderChain::with_catalog(None).expect("the chain builds");
        assert_eq!(chain.names(), vec!["catalog", "zokoanime"]);
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

    /// ZokoAnime is addressed by MyAnimeList id, which a catalog row has none of,
    /// so nothing behind the catalog can stand in for it.
    #[tokio::test]
    async fn a_cached_row_no_provider_recognises_names_what_was_tried() {
        let chain = ProviderChain::with_catalog(None).expect("the chain builds");
        let error = chain
            .resolve(&request(Origin::Cached("solo-leveling".into())))
            .await
            .expect_err("nothing serves a catalog row without a catalog");
        let message = format!("{error:#}");
        assert!(message.contains("catalog, zokoanime"), "{message}");
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
