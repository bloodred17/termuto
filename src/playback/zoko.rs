//! The ZokoAnime extractor.
//!
//! ZokoAnime keys its player pages on MyAnimeList ids, which is exactly what a
//! [`Origin::Live`] row already carries, so no title matching is needed:
//! `/stream/mal/{mal_id}/{episode}/{sub|dub}` is the page, and it ships its
//! configuration inline as `window.__P` — base64 of the JSON, XOR'd against a
//! fixed key by the site's own `/core/obfuscate.js`. Undoing that yields the
//! HLS master playlist and any external subtitle tracks.
//!
//! The master playlist lists its renditions worst-first, so it is fetched and
//! resolved here rather than handed to the player, which would otherwise open
//! the 360p variant by default.

use super::prefs::{Audio, Quality};
use super::provider::{Stream, StreamProvider, StreamRequest};
use crate::source::Origin;
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::Url;
use serde::Deserialize;
use std::cmp::Reverse;
use std::env;
use std::time::Duration;

/// The site the player pages are served from.
pub const DEFAULT_BASE_URL: &str = "https://zokoanime.video";

/// Overrides [`DEFAULT_BASE_URL`], mainly so the host can be pointed at a mirror
/// when it moves domain.
pub const BASE_URL_ENV: &str = "TERMUTO_ZOKO_BASE";

/// The key `/core/obfuscate.js` XORs the configuration against.
const OBFUSCATION_KEY: &[u8] = b"otaku-embed-v1";

/// Where the obfuscated configuration is assigned in the page.
const PAYLOAD_MARKER: &str = "window.__P";

/// A browser user agent. The CDN serving these manifests rejects requests that
/// do not look like they came from the player page.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Resolves a MyAnimeList id and episode number to a playable rendition.
#[derive(Clone, Debug)]
pub struct ZokoProvider {
    http: reqwest::Client,
    base_url: String,
    /// Scheme and host of [`Self::base_url`], which is what the CDN wants to see
    /// as the referer.
    origin: String,
}

impl ZokoProvider {
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
            .context("Could not build the HTTP client for ZokoAnime")?;
        Ok(Self {
            http,
            base_url,
            origin,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Loads one player page and decodes its configuration. A page that exists
    /// but has nothing to play is not an error here — the other audio track is
    /// still worth trying — so it comes back as [`Page::Unavailable`].
    async fn page(&self, mal_id: u32, episode: u32, audio: Audio) -> Result<Page> {
        let url = stream_page_url(&self.base_url, mal_id, episode, audio);
        let html = self
            .fetch(&url, "text/html,application/xhtml+xml")
            .await
            .with_context(|| format!("Could not load the ZokoAnime player page at {url}"))?;

        // A removed episode still answers 200, with an error card in place of
        // the payload, so a missing payload is checked before it is decoded.
        if !html.contains(PAYLOAD_MARKER) {
            return Ok(Page::Unavailable(
                page_message(&html).unwrap_or_else(|| format!("{url} has no stream on it")),
            ));
        }

        let json = decode_config(&html)
            .with_context(|| format!("Could not decode the ZokoAnime configuration from {url}"))?;
        let config: PlayerConfig = serde_json::from_str(&json)
            .with_context(|| format!("Could not read the ZokoAnime configuration from {url}"))?;
        Ok(Page::Config(Box::new(config)))
    }

    /// Picks the rendition asked for out of `src`. A `src` that is already a
    /// media playlist, rather than a master listing variants, is used as it is.
    async fn rendition(&self, src: &str, quality: &Quality) -> Result<(String, Option<String>)> {
        let base = Url::parse(src)
            .with_context(|| format!("ZokoAnime returned an unusable stream URL: {src}"))?;
        let master = self
            .fetch(src, "application/vnd.apple.mpegurl,*/*")
            .await
            .with_context(|| format!("Could not load the ZokoAnime playlist at {src}"))?;

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
            .header("origin", &self.origin)
            .header("user-agent", BROWSER_USER_AGENT)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            bail!("the request failed ({status})");
        }
        Ok(response.text().await?)
    }
}

#[async_trait]
impl StreamProvider for ZokoProvider {
    fn name(&self) -> &str {
        "zokoanime"
    }

    async fn resolve(&self, request: &StreamRequest) -> Result<Option<Stream>> {
        // The host is addressed by MyAnimeList id, which only an API row has.
        // A catalog row carries no such id, so it is not a request served here.
        let Origin::Live(mal_id) = request.origin else {
            return Ok(None);
        };
        // A movie, or a series played from the beginning, is episode one.
        let episode = request.episode.unwrap_or(1);

        let mut unavailable = Vec::new();
        let mut found = None;
        for audio in track_order(request.prefs.audio) {
            match self.page(mal_id, episode, audio).await? {
                Page::Config(config) => {
                    found = Some((audio, config));
                    break;
                }
                Page::Unavailable(reason) => unavailable.push(format!("{audio}: {reason}")),
            }
        }

        let Some((audio, config)) = found else {
            bail!(
                "ZokoAnime has nothing to play for {} (MyAnimeList id {mal_id}).\n{}",
                request.label(),
                unavailable.join("\n")
            );
        };

        let src = config
            .src
            .as_deref()
            .map(str::trim)
            .filter(|src| !src.is_empty())
            .with_context(|| {
                format!(
                    "ZokoAnime listed no source for {} ({audio})",
                    request.label()
                )
            })?;

        let (url, quality) = self.rendition(src, &request.prefs.quality).await?;

        Ok(Some(Stream {
            url,
            provider: self.name().to_string(),
            headers: embed_headers(&self.origin),
            subtitles: subtitle_tracks(&config.subtitles),
            // What was served, which for a title with only one track is not
            // necessarily what was asked for.
            audio,
            quality,
        }))
    }
}

/// What one player page yielded.
enum Page {
    Config(Box<PlayerConfig>),
    /// The page loaded but carried no stream, described in the page's own words.
    Unavailable(String),
}

/// The fields of `window.__P` that playback uses. The payload carries player
/// skinning and view-tracking settings besides these, which are ignored.
#[derive(Debug, Default, Deserialize)]
struct PlayerConfig {
    /// The HLS master playlist.
    #[serde(default)]
    src: Option<String>,
    #[serde(default)]
    subtitles: Vec<Subtitle>,
}

#[derive(Debug, Deserialize)]
struct Subtitle {
    /// Absent on every track seen so far; when present it distinguishes caption
    /// tracks from the thumbnail and chapter tracks players also side-load.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    default: bool,
    #[serde(default, alias = "file", alias = "url")]
    src: Option<String>,
}

/// The requested track first and the other behind it: a title that carries only
/// one of the two should still play rather than reporting nothing.
fn track_order(preferred: Audio) -> [Audio; 2] {
    match preferred {
        Audio::Sub => [Audio::Sub, Audio::Dub],
        Audio::Dub => [Audio::Dub, Audio::Sub],
    }
}

fn stream_page_url(base_url: &str, mal_id: u32, episode: u32, audio: Audio) -> String {
    format!("{base_url}/stream/mal/{mal_id}/{episode}/{audio}")
}

/// The headers a manifest request has to carry to be served. Without them the
/// CDN answers 403 and the player exits without drawing a frame, so they travel
/// with the stream rather than being applied at resolve time only.
fn embed_headers(origin: &str) -> Vec<(String, String)> {
    vec![
        ("Referer".to_string(), format!("{origin}/")),
        ("Origin".to_string(), origin.to_string()),
        ("User-Agent".to_string(), BROWSER_USER_AGENT.to_string()),
    ]
}

/// Reverses the site's own obfuscation: base64, then a repeating-key XOR.
fn decode_config(html: &str) -> Result<String> {
    let payload = quoted_after(html, PAYLOAD_MARKER)
        .context("the page carries no window.__P payload; its layout has changed")?;
    // The assignment is a JavaScript string literal, which may be wrapped.
    let payload: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = STANDARD
        .decode(&payload)
        .context("the window.__P payload is not base64")?;
    let plain: Vec<u8> = bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ OBFUSCATION_KEY[index % OBFUSCATION_KEY.len()])
        .collect();
    String::from_utf8(plain).context("the decoded window.__P payload is not UTF-8")
}

/// The first double-quoted run following `marker`, so that the assignment is
/// read without caring how the site spaces it.
fn quoted_after<'a>(haystack: &'a str, marker: &str) -> Option<&'a str> {
    let after_marker = haystack.split_once(marker)?.1;
    let after_quote = after_marker.split_once('"')?.1;
    after_quote.split_once('"').map(|(value, _)| value)
}

/// The error card states its own reason, so a removed episode reads as one
/// rather than as a generic parse failure.
fn page_message(html: &str) -> Option<String> {
    let message = html
        .split_once("class=\"msg\">")?
        .1
        .split_once("</p>")?
        .0
        .trim();
    (!message.is_empty()).then(|| message.to_string())
}

/// One rendition listed in a master playlist.
#[derive(Debug, Eq, PartialEq)]
struct Variant {
    url: String,
    /// The vertical resolution, which is how a quality preference names it.
    height: Option<u32>,
    bandwidth: u64,
}

const STREAM_INF: &str = "#EXT-X-STREAM-INF:";

fn parse_variants(master: &str, base: &Url) -> Vec<Variant> {
    let lines: Vec<&str> = master.lines().map(str::trim).collect();
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let attributes = line.strip_prefix(STREAM_INF)?;
            // The URI follows the tag, past any blank or commented lines.
            let uri = lines[index + 1..]
                .iter()
                .find(|line| !line.is_empty() && !line.starts_with('#'))?;
            Some(Variant {
                url: base.join(uri).ok()?.to_string(),
                height: attribute(attributes, "RESOLUTION")
                    .and_then(|resolution| resolution.rsplit('x').next()?.parse().ok()),
                bandwidth: attribute(attributes, "BANDWIDTH")
                    .and_then(|bandwidth| bandwidth.parse().ok())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Splitting on commas can cut a quoted value such as `CODECS="a,b"` in half,
/// but only into fragments that match no name asked for here.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    attributes
        .split(',')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .map(|value| value.trim_matches('"'))
}

/// An exact quality that the host does not carry degrades to the nearest
/// rendition it does, per [`Quality::Exact`], rather than failing playback.
fn choose_variant<'a>(variants: &'a [Variant], quality: &Quality) -> Option<&'a Variant> {
    let highest = || variants.iter().max_by_key(|variant| variant.bandwidth);
    let Quality::Exact(height) = quality else {
        return highest();
    };
    let Ok(wanted) = height.parse::<u32>() else {
        return highest();
    };
    variants
        .iter()
        .filter_map(|variant| Some((variant, variant.height?)))
        .min_by_key(|(variant, height)| (height.abs_diff(wanted), Reverse(variant.bandwidth)))
        .map(|(variant, _)| variant)
        .or_else(highest)
}

/// Every caption track, the default one first so a player that takes the first
/// it is given takes the intended one.
fn subtitle_tracks(subtitles: &[Subtitle]) -> Vec<String> {
    let captions = subtitles.iter().filter(|track| {
        matches!(
            track.kind.as_deref(),
            None | Some("captions") | Some("subtitles")
        )
    });
    let (mut default, other): (Vec<&Subtitle>, Vec<&Subtitle>) =
        captions.partition(|track| track.default);
    default.extend(other);
    default
        .iter()
        .filter_map(|track| track.src.as_deref())
        .map(str::trim)
        .filter(|src| !src.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Audio, PlayerConfig, Quality, Subtitle, Url, Variant, ZokoProvider, choose_variant,
        decode_config, page_message, parse_variants, stream_page_url, subtitle_tracks, track_order,
    };
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    const MASTER: &str = "\
#EXTM3U
#EXT-X-VERSION:4
#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=640x360
360/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720
720/index.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5300000,RESOLUTION=1920x1080
1080/index.m3u8
";

    fn base() -> Url {
        Url::parse("https://hls2.example.test/v/a/b/c/master.m3u8").expect("a base")
    }

    /// The page ships the payload the way the site's own obfuscate.js writes it.
    fn obfuscate(json: &str) -> String {
        let key = super::OBFUSCATION_KEY;
        let bytes: Vec<u8> = json
            .bytes()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index % key.len()])
            .collect();
        format!(
            "<script>window.__P=\"{}\";</script>",
            STANDARD.encode(bytes)
        )
    }

    #[test]
    fn the_page_url_is_keyed_on_the_mal_id_episode_and_track() {
        assert_eq!(
            stream_page_url("https://zokoanime.video", 59193, 1, Audio::Sub),
            "https://zokoanime.video/stream/mal/59193/1/sub"
        );
        assert_eq!(
            stream_page_url("https://zokoanime.video", 1, 12, Audio::Dub),
            "https://zokoanime.video/stream/mal/1/12/dub"
        );
    }

    #[test]
    fn the_obfuscated_payload_round_trips_back_to_its_json() {
        let json = r#"{"src":"https://hls2.example.test/master.m3u8","subtitles":[]}"#;
        let decoded = decode_config(&obfuscate(json)).expect("the payload decodes");
        assert_eq!(decoded, json);

        let config: PlayerConfig = serde_json::from_str(&decoded).expect("the config parses");
        assert_eq!(
            config.src.as_deref(),
            Some("https://hls2.example.test/master.m3u8")
        );
    }

    #[test]
    fn a_page_without_a_payload_is_reported_rather_than_decoded() {
        assert!(decode_config("<html>nothing here</html>").is_err());
    }

    /// The host answers 200 with an error card for a removed episode, so its own
    /// wording is what the user should see.
    #[test]
    fn the_error_card_supplies_the_reason() {
        let html = r#"<div class="card-body"><h1 class="title">Not found.</h1>
      <p class="msg">We couldn't find anything to play here.</p></div>"#;
        assert_eq!(
            page_message(html).as_deref(),
            Some("We couldn't find anything to play here.")
        );
        assert_eq!(page_message("<html></html>"), None);
    }

    #[test]
    fn variants_are_read_with_their_height_and_bandwidth_and_resolved_against_the_master() {
        let variants = parse_variants(MASTER, &base());
        assert_eq!(variants.len(), 3);
        assert_eq!(
            variants[2],
            Variant {
                url: "https://hls2.example.test/v/a/b/c/1080/index.m3u8".into(),
                height: Some(1080),
                bandwidth: 5_300_000,
            }
        );
    }

    /// The playlist lists its renditions worst-first, so the default pick has to
    /// be resolved here rather than left to the player.
    #[test]
    fn the_best_quality_is_the_highest_bandwidth_not_the_first_listed() {
        let variants = parse_variants(MASTER, &base());
        let chosen = choose_variant(&variants, &Quality::Best).expect("a variant");
        assert_eq!(chosen.height, Some(1080));
    }

    #[test]
    fn an_exact_quality_is_matched_and_an_absent_one_degrades_to_the_nearest() {
        let variants = parse_variants(MASTER, &base());
        let height = |quality: &str| {
            choose_variant(&variants, &quality.parse().expect("a quality"))
                .expect("a variant")
                .height
        };
        assert_eq!(height("720"), Some(720));
        // 1440 is not carried; the nearest rendition plays instead of failing.
        assert_eq!(height("1440"), Some(1080));
        assert_eq!(height("240"), Some(360));
    }

    #[test]
    fn a_media_playlist_with_no_variants_selects_nothing() {
        let media = "#EXTM3U\n#EXTINF:4.0,\nseg-1.ts\n";
        assert!(choose_variant(&parse_variants(media, &base()), &Quality::Best).is_none());
    }

    #[test]
    fn the_default_caption_track_is_offered_first_and_other_kinds_are_dropped() {
        let track = |label: &str, default: bool, kind: Option<&str>| Subtitle {
            kind: kind.map(str::to_string),
            default,
            src: Some(format!("https://example.test/{label}.vtt")),
        };
        let tracks = subtitle_tracks(&[
            track("english", false, None),
            track("forced", true, None),
            track("thumbs", false, Some("thumbnails")),
        ]);
        assert_eq!(
            tracks,
            vec![
                "https://example.test/forced.vtt".to_string(),
                "https://example.test/english.vtt".to_string(),
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
        let provider = ZokoProvider::with_base_url("https://mirror.test/").expect("it builds");
        assert_eq!(provider.base_url(), "https://mirror.test");
        assert_eq!(provider.origin, "https://mirror.test");
        assert!(ZokoProvider::with_base_url("not a url").is_err());
    }
}
