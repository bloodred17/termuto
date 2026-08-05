//! A local HTTP proxy that makes a disguised stream playable.
//!
//! Some hosts prefix every media segment with a small valid PNG, so a segment
//! probes as an image: ffmpeg reads `png,video` and playback never starts. No
//! player can be told to skip those bytes — the fetch happens inside its HLS
//! demuxer — so the bytes have to be repaired before the player sees them.
//!
//! This binds a loopback listener and serves `GET /?u=<upstream>`. A playlist
//! comes back with every URI rewritten to point at this proxy, so the player
//! follows the whole tree through it; anything else is treated as a segment and
//! served with its fake header removed. The host's own headers are attached to
//! the upstream request, since the CDN answers 403 without them.
//!
//! The proxy lives in this process, so whatever owns it has to outlive
//! playback. Dropping it stops the listener, which is what ends it.

use anyhow::{Context, Result};
use reqwest::Url;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The signature every PNG starts with, and the chunk every PNG ends with.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
const PNG_END: &[u8] = b"IEND";

/// `IEND` is followed by its four-byte CRC, and the media begins after that.
const PNG_END_LENGTH: usize = PNG_END.len() + 4;

/// A request line plus headers past this is not one a player sent.
const MAX_REQUEST_BYTES: usize = 16 * 1024;

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(30);

/// The query parameter carrying the upstream URL.
const TARGET_PARAM: &str = "u";

/// A running proxy. Dropping it closes the listener and ends the accept loop.
#[derive(Debug)]
pub struct Proxy {
    port: u16,
    accept: JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // In-flight segment requests are already spawned and finish on their
        // own; this stops new ones and frees the port.
        self.accept.abort();
    }
}

struct State {
    http: reqwest::Client,
    /// Attached to every upstream request. The CDN answers 403 without them.
    headers: Vec<(String, String)>,
    port: u16,
}

impl Proxy {
    /// Binds an ephemeral loopback port and starts serving.
    pub async fn start(headers: Vec<(String, String)>) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .context("Could not bind a local port for the segment proxy")?;
        let port = listener
            .local_addr()
            .context("Could not read the segment proxy's port")?
            .port();

        let http = reqwest::Client::builder()
            .timeout(UPSTREAM_TIMEOUT)
            .build()
            .context("Could not build the HTTP client for the segment proxy")?;
        let state = Arc::new(State {
            http,
            headers,
            port,
        });

        let accept = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    continue;
                };
                let state = Arc::clone(&state);
                // A failed connection must not take the proxy down with it: the
                // player retries, and a dead listener would end playback.
                tokio::spawn(async move {
                    let _ = serve(socket, state).await;
                });
            }
        });

        Ok(Self { port, accept })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The local URL that plays `upstream` through this proxy.
    pub fn url_for(&self, upstream: &str) -> String {
        proxied(self.port, upstream)
    }
}

fn proxied(port: u16, upstream: &str) -> String {
    let mut url = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("a valid loopback url");
    url.query_pairs_mut().append_pair(TARGET_PARAM, upstream);
    url.to_string()
}

async fn serve(mut socket: TcpStream, state: Arc<State>) -> Result<()> {
    let Some(target) = read_target(&mut socket).await? else {
        return respond(&mut socket, "400 Bad Request", "text/plain", b"").await;
    };
    let Some(upstream) = target_url(&target) else {
        return respond(&mut socket, "400 Bad Request", "text/plain", b"").await;
    };

    let mut request = state.http.get(upstream.clone());
    for (name, value) in &state.headers {
        request = request.header(name, value);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(_) => return respond(&mut socket, "502 Bad Gateway", "text/plain", b"").await,
    };
    let status = response.status();
    if !status.is_success() {
        let line = format!(
            "{} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        return respond(&mut socket, &line, "text/plain", b"").await;
    }
    let Ok(body) = response.bytes().await else {
        return respond(&mut socket, "502 Bad Gateway", "text/plain", b"").await;
    };

    // A playlist is rewritten so the player keeps following it through here;
    // everything else is a segment, and only needs its fake header removed.
    if is_playlist(&body) {
        let text = String::from_utf8_lossy(&body);
        let rewritten = rewrite_playlist(&text, &upstream, state.port);
        respond(
            &mut socket,
            "200 OK",
            "application/vnd.apple.mpegurl",
            rewritten.as_bytes(),
        )
        .await
    } else {
        respond(&mut socket, "200 OK", "video/mp2t", strip_png_prefix(&body)).await
    }
}

/// The request target from the first line, once the headers have been read off
/// the socket. `None` means the request was malformed or oversized.
async fn read_target(socket: &mut TcpStream) -> Result<Option<String>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = socket.read(&mut chunk).await?;
        if read == 0 || buffer.len() + read > MAX_REQUEST_BYTES {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    let head = String::from_utf8_lossy(&buffer);
    let line = head.lines().next().unwrap_or_default();
    // "GET /?u=… HTTP/1.1"
    Ok(line.split_whitespace().nth(1).map(str::to_string))
}

/// The upstream URL carried in the target's `u` parameter.
fn target_url(target: &str) -> Option<Url> {
    let local = Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    let value = local
        .query_pairs()
        .find(|(name, _)| name == TARGET_PARAM)?
        .1
        .into_owned();
    Url::parse(&value).ok()
}

fn is_playlist(body: &[u8]) -> bool {
    body.strip_prefix(b"\xef\xbb\xbf")
        .unwrap_or(body)
        .starts_with(b"#EXTM3U")
}

/// Points every URI in a playlist back at this proxy, resolved against the
/// playlist's own location so relative segment names keep working.
fn rewrite_playlist(body: &str, base: &Url, port: u16) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
        } else if let Some(rest) = trimmed.strip_prefix('#') {
            // Tags carry their own targets: EXT-X-KEY, EXT-X-MAP, EXT-X-MEDIA.
            out.push_str(&rewrite_tag_uri(trimmed, rest, base, port));
            out.push('\n');
        } else {
            match base.join(trimmed) {
                Ok(absolute) => out.push_str(&proxied(port, absolute.as_str())),
                // An unresolvable URI is left alone rather than dropped, so the
                // player reports it rather than playing a truncated playlist.
                Err(_) => out.push_str(trimmed),
            }
            out.push('\n');
        }
    }
    out
}

fn rewrite_tag_uri(line: &str, tag: &str, base: &Url, port: u16) -> String {
    let Some(start) = tag.find("URI=\"") else {
        return line.to_string();
    };
    // Offsets are against `tag`, which is `line` past its leading '#'.
    let value_start = start + "URI=\"".len() + 1;
    let Some(length) = tag[value_start - 1..].find('"') else {
        return line.to_string();
    };
    let value = &line[value_start..value_start + length];
    let Ok(absolute) = base.join(value) else {
        return line.to_string();
    };
    format!(
        "{}{}{}",
        &line[..value_start],
        proxied(port, absolute.as_str()),
        &line[value_start + length..]
    )
}

/// Removes the decoy PNG a host wraps its segments in. A segment that does not
/// carry one is passed through untouched, so this is safe to apply to every
/// response the proxy did not recognise as a playlist.
fn strip_png_prefix(body: &[u8]) -> &[u8] {
    if !body.starts_with(PNG_SIGNATURE) {
        return body;
    }
    let Some(end) = body
        .windows(PNG_END.len())
        .position(|window| window == PNG_END)
    else {
        return body;
    };
    body.get(end + PNG_END_LENGTH..).unwrap_or(body)
}

async fn respond(
    socket: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    // Closing each response keeps this to one exchange per connection, which is
    // all a segment fetch needs and avoids parsing keep-alive framing.
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: {content_type}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        PNG_SIGNATURE, Proxy, Url, is_playlist, proxied, rewrite_playlist, strip_png_prefix,
        target_url,
    };

    fn base() -> Url {
        Url::parse("https://host.test/vid/abc/index-f1.m3u8").expect("a base")
    }

    /// A real megavid segment: a 70-byte PNG, then the MPEG-TS sync byte.
    fn disguised_segment() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(PNG_SIGNATURE);
        body.extend_from_slice(b"\x00\x00\x00\x0dIHDR____________crc");
        body.extend_from_slice(b"\x00\x00\x00\x00IEND\xae\x42\x60\x82");
        body.extend_from_slice(b"\x47\x40\x11\x11media");
        body
    }

    /// ffmpeg probes an untouched segment as `png,video` and never starts, so
    /// the media has to begin at the very first byte the player sees.
    #[test]
    fn the_decoy_png_is_removed_and_the_media_starts_at_the_sync_byte() {
        let segment = disguised_segment();
        let stripped = strip_png_prefix(&segment);
        assert_eq!(stripped, b"\x47\x40\x11\x11media");
        assert_eq!(stripped[0], 0x47);
    }

    #[test]
    fn a_segment_without_a_decoy_is_passed_through_untouched() {
        let plain = b"\x47\x40\x11\x11media";
        assert_eq!(strip_png_prefix(plain), plain);
        // A truncated decoy is left alone rather than cut at a guessed offset.
        assert_eq!(strip_png_prefix(PNG_SIGNATURE), PNG_SIGNATURE);
    }

    #[test]
    fn playlists_are_recognised_and_segments_are_not() {
        assert!(is_playlist(b"#EXTM3U\n#EXT-X-VERSION:3\n"));
        assert!(is_playlist(b"\xef\xbb\xbf#EXTM3U\n"));
        assert!(!is_playlist(&disguised_segment()));
    }

    /// Every URI has to come back through the proxy, or the player fetches the
    /// segments directly and gets the decoy bytes again.
    #[test]
    fn every_playlist_uri_is_rewritten_through_the_proxy() {
        let playlist = "#EXTM3U\n#EXTINF:4.0,\nseg-1.ts\n#EXTINF:4.0,\nhttps://cdn.test/seg-2.ts\n";
        let rewritten = rewrite_playlist(playlist, &base(), 4321);
        assert!(rewritten.contains(&proxied(4321, "https://host.test/vid/abc/seg-1.ts")));
        assert!(rewritten.contains(&proxied(4321, "https://cdn.test/seg-2.ts")));
        // Tags and their ordering survive untouched.
        assert!(rewritten.starts_with("#EXTM3U\n#EXTINF:4.0,\n"));
        assert!(!rewritten.contains("\nseg-1.ts\n"));
    }

    #[test]
    fn a_uri_attribute_on_a_tag_is_rewritten_too() {
        let playlist = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x00\nseg.ts\n";
        let rewritten = rewrite_playlist(playlist, &base(), 4321);
        assert!(rewritten.contains(&format!(
            "URI=\"{}\"",
            proxied(4321, "https://host.test/vid/abc/key.bin")
        )));
        // The rest of the tag is preserved on either side of the URI.
        assert!(rewritten.contains("#EXT-X-KEY:METHOD=AES-128,URI=\""));
        assert!(rewritten.contains("\",IV=0x00"));
    }

    #[test]
    fn the_upstream_url_survives_the_round_trip_through_the_query() {
        let upstream = "https://host.test/vid/a+b/seg.ts?token=x%2Fy&n=1";
        let local = proxied(9, upstream);
        let target = local
            .strip_prefix("http://127.0.0.1:9")
            .expect("a local url");
        assert_eq!(target_url(target).expect("a target").as_str(), upstream);
    }

    #[test]
    fn a_target_without_an_upstream_is_rejected() {
        assert!(target_url("/").is_none());
        assert!(target_url("/?u=not-a-url").is_none());
    }

    #[tokio::test]
    async fn a_started_proxy_binds_a_port_and_addresses_upstreams_on_it() {
        let proxy = Proxy::start(vec![("Referer".into(), "https://host.test/".into())])
            .await
            .expect("the proxy binds");
        assert!(proxy.port() > 0);
        assert!(
            proxy
                .url_for("https://host.test/seg.ts")
                .starts_with(&format!("http://127.0.0.1:{}/?u=", proxy.port()))
        );
    }
}
