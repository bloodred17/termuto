//! Playing a selected title.
//!
//! Playback is two independent steps. [`provider`] turns a row and an episode
//! number into a [`Stream`] — a location a player can open — and [`player`]
//! hands that location to an external player. Both frontends go through
//! [`Playback`], so neither knows which provider answered.
//!
//! A third step exists only for hosts that disguise their segments: such a
//! stream is routed through the local [`proxy`], which repairs the bytes on the
//! way to the player. That proxy runs in this process, so [`Playback`] must
//! outlive the player whenever [`Playback::is_proxying`] holds — a one-shot
//! caller has to [`Playback::wait_for_players`] rather than exiting.

pub mod hls;
pub mod megavid;
pub mod player;
pub mod prefs;
pub mod provider;
pub mod proxy;
pub mod zoko;

use crate::catalog::CatalogRepository;
use crate::source::Source;
use anyhow::Result;
use proxy::Proxy;
use std::time::Duration;

pub use player::{DEFAULT_PLAYER, PLAYER_ENV, Player, resolve_player};
pub use prefs::{AUDIO_ENV, Audio, QUALITY_ENV, Quality, TrackPrefs, resolve_prefs};
pub use provider::{PROVIDER_ENV, ProviderChain, Stream, StreamProvider, StreamRequest};

/// How often [`Playback::wait_for_players`] looks at the player again. Long
/// enough to cost nothing, short enough to exit promptly when the player does.
const WAIT_POLL: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct Playback {
    chain: ProviderChain,
    player: Player,
    prefs: TrackPrefs,
    /// Started on the first stream that needs it and kept for the session. The
    /// hosts that need one accept a single referer for every request, so one
    /// proxy serves them all.
    proxy: Option<Proxy>,
}

impl Playback {
    pub fn new(
        catalog: Option<CatalogRepository>,
        prefs: TrackPrefs,
        player: String,
    ) -> Result<Self> {
        Ok(Self {
            chain: ProviderChain::with_catalog(catalog)?,
            player: Player::new(player),
            prefs,
            proxy: None,
        })
    }

    /// Builds the same playback surface both frontends use from an open source.
    pub fn for_source(source: &Source, prefs: TrackPrefs, player: String) -> Result<Self> {
        Self::new(source.catalog().cloned(), prefs, player)
    }

    pub fn prefs(&self) -> &TrackPrefs {
        &self.prefs
    }

    pub fn player_name(&self) -> &str {
        self.player.program()
    }

    /// The hosts that can be chosen between, in the order they are asked.
    pub fn provider_names(&self) -> Vec<&str> {
        self.chain.remote_names()
    }

    /// Which host is asked first.
    pub fn provider(&self) -> Option<&str> {
        self.chain.preferred()
    }

    /// Moves to the next host, wrapping at the end. Only affects which one is
    /// asked first — the others stay on as fallbacks.
    pub fn cycle_provider(&mut self) {
        self.chain.cycle_preferred();
    }

    /// Chooses `name` as the leading host, failing on an unknown one.
    pub fn prefer_provider(&mut self, name: &str) -> Result<()> {
        self.chain.prefer(name)
    }

    /// Whether a stream is playing through the in-process proxy. While this
    /// holds, dropping this `Playback` stops playback, so a one-shot caller has
    /// to [`Self::wait_for_players`] instead of returning.
    pub fn is_proxying(&self) -> bool {
        self.proxy.is_some()
    }

    /// The loopback port the proxy is serving on, worth reporting because it is
    /// what a stalled stream is diagnosed against.
    pub fn proxy_port(&self) -> Option<u16> {
        self.proxy.as_ref().map(Proxy::port)
    }

    /// Waits until every player started here has exited. Polls rather than
    /// blocking, so the proxy's own tasks keep being served meanwhile.
    pub async fn wait_for_players(&mut self) {
        while self.player.any_running() {
            tokio::time::sleep(WAIT_POLL).await;
        }
    }

    /// Where the player writes its own diagnostics. Worth reporting: the player
    /// is detached, so a stream it rejects fails after termuto has moved on.
    pub fn log_path(&self) -> &std::path::Path {
        self.player.log_path()
    }

    /// Resolves `title`/`episode` through the provider chain and starts the
    /// player on whatever answered. The resolved stream is returned so the
    /// caller can report which provider was used.
    pub async fn play(&mut self, request: StreamRequest) -> Result<Stream> {
        let stream = self.chain.resolve(&request).await?;

        // The player is given the proxied location, but the stream reported
        // back keeps the host's own URL: a 127.0.0.1 address would say nothing
        // about which host answered or whether the match was right.
        let mut playing = stream.clone();
        if stream.strip_segment_prefix {
            if self.proxy.is_none() {
                self.proxy = Some(Proxy::start(stream.headers.clone()).await?);
            }
            let proxy = self.proxy.as_ref().expect("just started");
            playing.url = proxy.url_for(&stream.url);
        }

        self.player.play(&playing, &request.label())?;
        Ok(stream)
    }

    /// A request for `title` carrying the session's track preferences.
    pub fn request(
        &self,
        origin: crate::source::Origin,
        title: impl Into<String>,
        episode: Option<u32>,
    ) -> StreamRequest {
        StreamRequest {
            origin,
            title: title.into(),
            episode,
            prefs: self.prefs.clone(),
        }
    }
}
