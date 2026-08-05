//! The Tenrai API client and the payload types it decodes.

pub mod client;
pub mod model;

pub use client::{BASE_URL_ENV, DEFAULT_BASE_URL, LiveClient};
pub use model::{LiveAnime, LiveEpisode, Recommendation, SeasonYear};
