//! Playing a selected title.
//!
//! Playback is two independent steps. [`provider`] turns a row and an episode
//! number into a [`Stream`] — a location a player can open — and [`player`]
//! hands that location to an external player. Both frontends go through
//! [`Playback`], so neither knows which provider answered.

pub mod player;
pub mod prefs;
pub mod provider;

use crate::catalog::CatalogRepository;
use crate::source::Source;
use anyhow::Result;

pub use player::{DEFAULT_PLAYER, PLAYER_ENV, Player, resolve_player};
pub use prefs::{AUDIO_ENV, Audio, QUALITY_ENV, Quality, TrackPrefs, resolve_prefs};
pub use provider::{ProviderChain, Stream, StreamProvider, StreamRequest};

#[derive(Debug)]
pub struct Playback {
    chain: ProviderChain,
    player: Player,
    prefs: TrackPrefs,
}

impl Playback {
    pub fn new(catalog: Option<CatalogRepository>, prefs: TrackPrefs, player: String) -> Self {
        Self {
            chain: ProviderChain::with_catalog(catalog),
            player: Player::new(player),
            prefs,
        }
    }

    /// Builds the same playback surface both frontends use from an open source.
    pub fn for_source(source: &Source, prefs: TrackPrefs, player: String) -> Self {
        Self::new(source.catalog().cloned(), prefs, player)
    }

    pub fn prefs(&self) -> &TrackPrefs {
        &self.prefs
    }

    pub fn player_name(&self) -> &str {
        self.player.program()
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
        self.player.play(&stream, &request.label())?;
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
