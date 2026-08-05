//! Turning a selected title and episode into something a player can open.
//!
//! Providers are tried in order and the first one that recognises the request
//! wins. A provider that does not handle a request returns `Ok(None)` and the
//! chain moves on; only a provider that recognised the request and then broke
//! returns `Err`. Real providers scrape a host whose endpoints move, so each one
//! is a single self-contained implementation of [`StreamProvider`] — replacing a
//! dead extractor is a one-file change and never touches the frontends.

use super::megavid::MegavidProvider;
use super::prefs::{Audio, Quality, TrackPrefs};
use super::zoko::ZokoProvider;
use crate::catalog::{AnimeKind, CatalogRepository};
use crate::source::Origin;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::fmt;

/// Names the host consulted first when `--provider` is absent.
pub const PROVIDER_ENV: &str = "TERMUTO_PROVIDER";

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
    /// Set when the host disguises its segments and no player can read them
    /// directly. Playback routes such a stream through [`super::proxy`] rather
    /// than handing [`Self::url`] to the player.
    pub strip_segment_prefix: bool,
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

    /// Whether this is a remote host the user can choose between. The catalog
    /// is not one: it serves only its own rows and declines everything else, so
    /// preferring it would change nothing.
    fn is_remote(&self) -> bool {
        true
    }
}

/// The ordered list of providers consulted for one request.
#[derive(Debug)]
pub struct ProviderChain {
    providers: Vec<Box<dyn StreamProvider>>,
    /// Which remote host to ask first. The rest still follow as fallbacks, so
    /// choosing one changes which host answers when several could, without
    /// costing the coverage of the others.
    preferred: Option<String>,
    /// Whether a host that cannot serve a request hands over to the next one.
    /// On by default: coverage differs per host, so falling through is what
    /// makes a title the leading host lacks playable at all. Turning it off
    /// pins playback to the chosen host, which is what you want when the
    /// fallback is the wrong stream rather than no stream.
    autoswitch: bool,
}

impl ProviderChain {
    pub fn new(providers: Vec<Box<dyn StreamProvider>>) -> Self {
        Self {
            providers,
            preferred: None,
            autoswitch: true,
        }
    }

    /// The catalog answers for its own rows; the extractors answer for the API
    /// rows, which carry the MyAnimeList id both are addressed by. ZokoAnime
    /// leads because it plays without a proxy; MegaVid follows because it
    /// carries titles ZokoAnime does not. Adding another means inserting it here.
    pub fn with_catalog(catalog: Option<CatalogRepository>) -> Result<Self> {
        Ok(Self::new(vec![
            Box::new(CatalogProvider { catalog }),
            Box::new(ZokoProvider::new()?),
            Box::new(MegavidProvider::new()?),
        ]))
    }

    /// The hosts the user can choose between, in declared order.
    pub fn remote_names(&self) -> Vec<&str> {
        self.providers
            .iter()
            .filter(|provider| provider.is_remote())
            .map(|provider| provider.name())
            .collect()
    }

    /// The host asked first, which is the leading remote one unless a choice
    /// has been made.
    pub fn preferred(&self) -> Option<&str> {
        self.preferred
            .as_deref()
            .or_else(|| self.remote_names().first().copied())
    }

    /// Moves the preference to the next host, wrapping at the end.
    pub fn cycle_preferred(&mut self) {
        let names = self.remote_names();
        let Some(current) = self.preferred() else {
            return;
        };
        let next = names
            .iter()
            .position(|name| *name == current)
            .map(|index| (index + 1) % names.len())
            .unwrap_or_default();
        self.preferred = names.get(next).map(|name| name.to_string());
    }

    /// Chooses `name` as the leading host. Fails rather than silently ignoring
    /// an unknown name, which would otherwise look like the choice took effect.
    pub fn prefer(&mut self, name: &str) -> Result<()> {
        let names = self.remote_names();
        match names.iter().find(|known| **known == name) {
            Some(known) => {
                self.preferred = Some((*known).to_string());
                Ok(())
            }
            None => bail!(
                "Unknown provider \"{name}\". Available providers: {}.",
                names.join(", ")
            ),
        }
    }

    /// Whether a host that cannot serve a request hands over to the next one.
    pub fn autoswitch(&self) -> bool {
        self.autoswitch
    }

    pub fn set_autoswitch(&mut self, on: bool) {
        self.autoswitch = on;
    }

    /// Flips it, returning the new setting so a caller can report it.
    pub fn toggle_autoswitch(&mut self) -> bool {
        self.autoswitch = !self.autoswitch;
        self.autoswitch
    }

    /// The order this chain is actually consulted in: the preferred host first,
    /// then the others — unless autoswitch is off, which drops them.
    fn order(&self) -> Vec<&dyn StreamProvider> {
        let preferred = self.preferred();
        let leads = |provider: &&dyn StreamProvider| {
            provider.is_remote() && Some(provider.name()) == preferred
        };
        let all = self.providers.iter().map(Box::as_ref);
        let (front, rest): (Vec<_>, Vec<_>) = all.partition(leads);
        // The catalog stays ahead of every host: it plays a local file, which
        // beats a scrape whenever it has one. It is not a host to switch
        // between, so autoswitch has no say over it.
        let (local, remote): (Vec<_>, Vec<_>) =
            rest.into_iter().partition(|provider| !provider.is_remote());
        let fallbacks = if self.autoswitch { remote } else { Vec::new() };
        local.into_iter().chain(front).chain(fallbacks).collect()
    }

    pub fn names(&self) -> Vec<&str> {
        self.providers
            .iter()
            .map(|provider| provider.name())
            .collect()
    }

    /// Says so when hosts were held back, since "nothing could resolve this"
    /// otherwise reads as "no host has it" when one of them might.
    fn autoswitch_note(&self) -> String {
        let held_back: Vec<&str> = self
            .remote_names()
            .into_iter()
            .filter(|name| Some(*name) != self.preferred())
            .collect();
        if self.autoswitch || held_back.is_empty() {
            return String::new();
        }
        format!(
            "\nAutoswitch is off, so {} {} not tried.",
            held_back.join(" and "),
            if held_back.len() == 1 { "was" } else { "were" }
        )
    }

    /// Tries each provider in turn. A provider that fails does not end the
    /// attempt — its reason is kept and reported only if nothing later succeeds,
    /// so one dead extractor cannot block a working one behind it.
    pub async fn resolve(&self, request: &StreamRequest) -> Result<Stream> {
        let mut failures = Vec::new();
        for provider in self.order() {
            match provider.resolve(request).await {
                Ok(Some(stream)) => return Ok(stream),
                Ok(None) => {}
                Err(error) => failures.push(format!("{}: {error:#}", provider.name())),
            }
        }

        let tried: Vec<&str> = self
            .order()
            .iter()
            .map(|provider| provider.name())
            .collect();
        if failures.is_empty() {
            bail!(
                "No provider could resolve {}. Providers tried: {}.{}",
                request.label(),
                tried.join(", "),
                self.autoswitch_note()
            );
        }
        bail!(
            "No provider could resolve {}.\n{}{}",
            request.label(),
            failures.join("\n"),
            self.autoswitch_note()
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

    /// Not a choice: it serves its own rows and declines everything else.
    fn is_remote(&self) -> bool {
        false
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
            strip_segment_prefix: false,
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
    fn the_catalog_leads_and_zokoanime_is_the_default_host() {
        let chain = ProviderChain::with_catalog(None).expect("the chain builds");
        assert_eq!(chain.names(), vec!["catalog", "zokoanime", "megavid"]);
        assert_eq!(chain.remote_names(), vec!["zokoanime", "megavid"]);
        // ZokoAnime leads because it plays without a proxy.
        assert_eq!(chain.preferred(), Some("zokoanime"));
    }

    /// `p` in the TUI, and `--provider` on the CLI, only change which host is
    /// asked first — the catalog still leads, and the other host still follows
    /// as a fallback, so choosing one never costs the coverage of the other.
    #[test]
    fn choosing_a_host_reorders_only_the_hosts() {
        let mut chain = ProviderChain::with_catalog(None).expect("the chain builds");
        chain.prefer("megavid").expect("a known host");
        assert_eq!(chain.preferred(), Some("megavid"));

        let order: Vec<&str> = chain.order().iter().map(|p| p.name()).collect();
        assert_eq!(order, vec!["catalog", "megavid", "zokoanime"]);
    }

    /// Autoswitch is what makes a title the leading host lacks playable at all,
    /// so it is on unless turned off.
    #[test]
    fn autoswitch_is_on_by_default_and_turning_it_off_drops_the_fallbacks() {
        let mut chain = ProviderChain::with_catalog(None).expect("the chain builds");
        assert!(chain.autoswitch());
        let order = |chain: &ProviderChain| -> Vec<String> {
            chain
                .order()
                .iter()
                .map(|provider| provider.name().to_string())
                .collect()
        };
        assert_eq!(order(&chain), vec!["catalog", "zokoanime", "megavid"]);

        assert!(!chain.toggle_autoswitch());
        // The catalog is not a host to switch between, so it stays.
        assert_eq!(order(&chain), vec!["catalog", "zokoanime"]);

        // And the chosen host is the one that is kept, not merely the first.
        chain.prefer("megavid").expect("a known host");
        assert_eq!(order(&chain), vec!["catalog", "megavid"]);

        assert!(chain.toggle_autoswitch());
        assert_eq!(order(&chain), vec!["catalog", "megavid", "zokoanime"]);
    }

    /// "Nothing could resolve this" otherwise reads as "no host has it" when a
    /// host that was held back might well have.
    #[tokio::test]
    async fn a_failure_with_autoswitch_off_says_what_was_held_back() {
        let mut chain = ProviderChain::with_catalog(None).expect("the chain builds");
        chain.set_autoswitch(false);
        let error = chain
            .resolve(&request(Origin::Cached("solo-leveling".into())))
            .await
            .expect_err("no catalog, so nothing resolves");
        let message = format!("{error:#}");
        assert!(message.contains("Autoswitch is off"), "{message}");
        assert!(message.contains("megavid was not tried"), "{message}");
    }

    #[tokio::test]
    async fn a_failure_with_autoswitch_on_adds_no_such_note() {
        let chain = ProviderChain::with_catalog(None).expect("the chain builds");
        let error = chain
            .resolve(&request(Origin::Cached("solo-leveling".into())))
            .await
            .expect_err("no catalog, so nothing resolves");
        assert!(!format!("{error:#}").contains("Autoswitch"));
    }

    #[test]
    fn cycling_walks_the_hosts_and_wraps() {
        let mut chain = ProviderChain::with_catalog(None).expect("the chain builds");
        chain.cycle_preferred();
        assert_eq!(chain.preferred(), Some("megavid"));
        chain.cycle_preferred();
        assert_eq!(chain.preferred(), Some("zokoanime"));
    }

    /// Silently ignoring an unknown name would look like the choice took effect.
    #[test]
    fn an_unknown_host_is_rejected_and_names_the_known_ones() {
        let mut chain = ProviderChain::with_catalog(None).expect("the chain builds");
        let error = chain.prefer("nyaa").expect_err("not a host");
        let message = format!("{error:#}");
        assert!(message.contains("nyaa"), "{message}");
        assert!(message.contains("zokoanime, megavid"), "{message}");
        // The rejected choice leaves the previous one standing.
        assert_eq!(chain.preferred(), Some("zokoanime"));
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
                strip_segment_prefix: false,
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
