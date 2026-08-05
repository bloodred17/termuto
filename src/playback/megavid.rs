//! The MegaVid extractor.
//!
//! MegaVid keys episodes the same way ZokoAnime does — on a MyAnimeList id an
//! [`Origin::Live`] row already carries — but hands its configuration over as
//! JSON rather than hiding it in the page: `GET /mal/{id}/{episode}/{sub|dub}
//! /source` answers with the HLS master playlist and every caption track.
//!
//! What makes it awkward is the media, not the lookup. Every segment is
//! prefixed with a small valid PNG, so ffmpeg probes a segment as `png,video`
//! and playback never starts. That cannot be fixed with headers, so a stream
//! from here is marked [`Stream::strip_segment_prefix`] and playback routes it
//! through the local [`super::proxy`]. Coverage is the reason to put up with
//! it: MegaVid carries titles ZokoAnime does not.

use super::hls::{choose_variant, parse_variants};
use super::prefs::{Audio, Quality};
use super::provider::{Stream, StreamProvider, StreamRequest};
use crate::source::Origin;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;
use std::env;
use std::time::Duration;

/// The site the source API is served from.
pub const DEFAULT_BASE_URL: &str = "https://megavid.buzz";

/// Overrides [`DEFAULT_BASE_URL`], mainly so the host can be pointed at a mirror
/// when it moves domain.
pub const BASE_URL_ENV: &str = "TERMUTO_MEGAVID_BASE";

/// The site keys episodes by catalogue: `mal`, `anilist`, `kisskh`, `aniwave`.
/// Only MyAnimeList ids are reachable from a row here.
const ID_NAMESPACE: &str = "mal";

/// A browser user agent. The CDN rejects requests that do not look like they
/// came from the player page.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Resolves a MyAnimeList id and episode number to a playable rendition.
#[derive(Clone, Debug)]
pub struct MegavidProvider {
    http: reqwest::Client,
    base_url: String,
    /// Scheme and host of [`Self::base_url`]. Segments are refused outright
    /// without a referer, but any referer on this host is accepted — so one
    /// value serves every request, including the proxy's.
    origin: String,
}

impl MegavidProvider {
    pub fn new() -> Result<Self> {
        let base_url = env::var(BASE_URL_ENV)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::with_base_url(base_url)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let origin = Url::parse(&base_url)
            .map(|url| url.origin().ascii_serialization())
            .with_context(|| format!("\"{base_url}\" is not a usable {BASE_URL_ENV}"))?;
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .context("Could not build the HTTP client for MegaVid")?;
        Ok(Self {
            http,
            base_url,
            origin,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Asks the source API for one episode. An episode the host does not carry
    /// answers `200` with a `missing` status, which is not an error here — the
    /// other audio track is still worth trying.
    async fn source(&self, mal_id: u32, episode: u32, audio: Audio) -> Result<Lookup> {
        let page = page_url(&self.base_url, mal_id, episode, audio);
        let url = format!("{page}/source");
        let body = self
            .fetch(&url, "application/json")
            .await
            .with_context(|| format!("Could not reach the MegaVid source API at {url}"))?;

        let response: SourceResponse = serde_json::from_str(&body)
            .with_context(|| format!("Could not read the MegaVid response from {url}"))?;

        if response.status.as_deref() != Some("ok") {
            return Ok(Lookup::Unavailable(response.reason()));
        }
        Ok(Lookup::Source(Box::new(response)))
    }

    /// Picks the rendition asked for out of `src`. A `src` that is already a
    /// media playlist, rather than a master listing variants, is used as it is.
    async fn rendition(&self, src: &str, quality: &Quality) -> Result<(String, Option<String>)> {
        let base = Url::parse(src)
            .with_context(|| format!("MegaVid returned an unusable stream URL: {src}"))?;
        let master = self
            .fetch(src, "application/vnd.apple.mpegurl,*/*")
            .await
            .with_context(|| format!("Could not load the MegaVid playlist at {src}"))?;

        let variants = parse_variants(&master, &base);
        Ok(match choose_variant(&variants, quality) {
            Some(variant) => (
                variant.url.clone(),
                variant.height.map(|height| height.to_string()),
            ),
            None => (src.to_string(), None),
        })
    }

    async fn fetch(&self, url: &str, accept: &str) -> Result<String> {
        let response = self
            .http
            .get(url)
            .header("accept", accept)
            .header("referer", format!("{}/", self.origin))
            .header("user-agent", BROWSER_USER_AGENT)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            bail!("the request failed ({status})");
        }
        Ok(response.text().await?)
    }

    /// What the proxy has to send upstream, and what mpv needs for the caption
    /// tracks it fetches itself.
    fn headers(&self) -> Vec<(String, String)> {
        vec![
            ("Referer".to_string(), format!("{}/", self.origin)),
            ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
        ]
    }
}

#[async_trait]
impl StreamProvider for MegavidProvider {
    fn name(&self) -> &str {
        "megavid"
    }

    async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>> {
        // The host is addressed by MyAnimeList id, which only an API row has.
        let Origin::Live(mal_id) = request.origin else {
            return Ok(None);
        };
        // A movie, or a series played from the beginning, is episode one.
        let episode = request.episode.unwrap_or(1);

        let mut unavailable = Vec::new();
        let mut found = None;
        for audio in track_order(request.prefs.audio) {
            match self.source(mal_id, episode, audio).await? {
                Lookup::Source(response) => {
                    found = Some((audio, response));
                    break;
                }
                Lookup::Unavailable(reason) => unavailable.push(format!("{audio}: {reason}")),
            }
        }

        let Some((audio, response)) = found else {
            bail!(
                "MegaVid has nothing to play for {} (MyAnimeList id {mal_id}).\n{}",
                request.label(),
                unavailable.join("\n")
            );
        };

        let src = response
            .source
            .as_deref()
            .map(str::trim)
            .filter(|src| !src.is_empty())
            .with_context(|| {
                format!(
                    "MegaVid reported no source for {} ({audio})",
                    request.label()
                )
            })?;

        let (url, quality) = self.rendition(src, &request.prefs.quality).await?;

        Ok(Some(Stream {
            url,
            provider: self.name().to_string(),
            headers: self.headers(),
            subtitles: subtitle_tracks(&response.tracks),
            audio,
            quality,
            // Every segment carries a decoy PNG header, so the player cannot be
            // handed this URL directly.
            strip_segment_prefix: true,
        }))
    }
}

/// What one source lookup yielded.
enum Lookup {
    Source(Box<SourceResponse>),
    /// The API answered, but with nothing to play, in its own words.
    Unavailable(String),
}

#[derive(Debug, Default, Deserialize)]
struct SourceResponse {
    /// `ok` when there is something to play, `missing` when there is not.
    #[serde(default)]
    status: Option<String>,
    /// The HLS master playlist.
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    tracks: Vec<Track>,
}

impl SourceResponse {
    /// The API's own explanation, preferred over a status word on its own.
    fn reason(&self) -> String {
        self.message
            .as_deref()
            .map(str::trim)
            .filter(|message| !message.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "the API answered \"{}\"",
                    self.status.as_deref().unwrap_or("nothing")
                )
            })
    }
}

#[derive(Debug, Deserialize)]
struct Track {
    /// `captions` for a subtitle track; hosts also list thumbnail tracks here.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    default: bool,
    #[serde(default, alias = "src", alias = "url")]
    file: Option<String>,
}

/// The requested track first and the other behind it: a title that carries only
/// one of the two should still play rather than reporting nothing.
fn track_order(preferred: Audio) -> [Audio; 2] {
    match preferred {
        Audio::Sub => [Audio::Sub, Audio::Dub],
        Audio::Dub => [Audio::Dub, Audio::Sub],
    }
}

fn page_url(base_url: &str, mal_id: u32, episode: u32, audio: Audio) -> String {
    format!("{base_url}/{ID_NAMESPACE}/{mal_id}/{episode}/{audio}")
}

/// Every caption track, the default one first so a player that takes the first
/// it is given takes the intended one.
fn subtitle_tracks(tracks: &[Track]) -> Vec<String> {
    let captions = tracks
        .iter()
        .filter(|track| matches!(track.kind.as_deref(), None | Some("captions")));
    let (mut default, other): (Vec<&Track>, Vec<&Track>) =
        captions.partition(|track| track.default);
    default.extend(other);
    default
        .iter()
        .filter_map(|track| track.file.as_deref())
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Audio, MegavidProvider, SourceResponse, Track, page_url, subtitle_tracks, track_order,
    };

    #[test]
    fn the_source_url_is_keyed_on_the_mal_id_episode_and_track() {
        assert_eq!(
            page_url("https://megavid.buzz", 59193, 1, Audio::Sub),
            "https://megavid.buzz/mal/59193/1/sub"
        );
        assert_eq!(
            page_url("https://megavid.buzz", 1, 12, Audio::Dub),
            "https://megavid.buzz/mal/1/12/dub"
        );
    }

    #[test]
    fn an_ok_response_yields_its_source_and_caption_tracks() {
        let body = r#"{"status":"ok","source":"https://megavid.buzz/vid/a/b/master.m3u8",
            "tracks":[{"label":"English","kind":"captions","default":true,"file":"https://megavid.buzz/sub/en.vtt"},
                      {"label":"Thai","kind":"captions","default":false,"file":"https://megavid.buzz/sub/th.vtt"}]}"#;
        let response: SourceResponse = serde_json::from_str(body).expect("it parses");
        assert_eq!(response.status.as_deref(), Some("ok"));
        assert_eq!(
            response.source.as_deref(),
            Some("https://megavid.buzz/vid/a/b/master.m3u8")
        );
        assert_eq!(subtitle_tracks(&response.tracks).len(), 2);
    }

    /// A removed episode answers 200 with a `missing` status, so the status is
    /// what decides, and the API's own message is what surfaces.
    #[test]
    fn a_missing_episode_is_reported_in_the_apis_own_words() {
        let body = r#"{"status":"missing","code":404,
            "message":"We couldn't find this episode. It may not exist, isn't available yet, or has been removed.",
            "retryable":false}"#;
        let response: SourceResponse = serde_json::from_str(body).expect("it parses");
        assert_ne!(response.status.as_deref(), Some("ok"));
        assert!(
            response
                .reason()
                .starts_with("We couldn't find this episode")
        );
    }

    #[test]
    fn a_response_with_no_message_still_names_its_status() {
        let response: SourceResponse =
            serde_json::from_str(r#"{"status":"blocked"}"#).expect("it parses");
        assert_eq!(response.reason(), "the API answered \"blocked\"");
    }

    #[test]
    fn the_default_caption_track_is_offered_first_and_other_kinds_are_dropped() {
        let track = |name: &str, default: bool, kind: Option<&str>| Track {
            kind: kind.map(str::to_string),
            default,
            file: Some(format!("https://megavid.buzz/sub/{name}.vtt")),
        };
        let tracks = subtitle_tracks(&[
            track("thai", false, Some("captions")),
            track("english", true, Some("captions")),
            track("thumbs", false, Some("thumbnails")),
        ]);
        assert_eq!(
            tracks,
            vec![
                "https://megavid.buzz/sub/english.vtt".to_string(),
                "https://megavid.buzz/sub/thai.vtt".to_string(),
            ]
        );
    }

    /// A title that carries only one of the two tracks should still play.
    #[test]
    fn the_requested_track_is_tried_first_and_the_other_is_the_fallback() {
        assert_eq!(track_order(Audio::Dub), [Audio::Dub, Audio::Sub]);
        assert_eq!(track_order(Audio::Sub), [Audio::Sub, Audio::Dub]);
    }

    #[test]
    fn the_base_url_loses_its_trailing_slash_and_yields_the_referer_origin() {
        let provider = MegavidProvider::with_base_url("https://mirror.test/").expect("it builds");
        assert_eq!(provider.base_url(), "https://mirror.test");
        // Segments are refused outright without a referer on the host.
        assert!(
            provider
                .headers()
                .iter()
                .any(|(name, value)| name == "Referer" && value == "https://mirror.test/")
        );
        assert!(MegavidProvider::with_base_url("not a url").is_err());
    }
}
