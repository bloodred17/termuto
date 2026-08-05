//! Fetching and decoding the still shown beside a selected episode.
//!
//! Drawing is left to `ratatui-image`, which asks the terminal what it can do
//! and uses the best answer: the Kitty graphics protocol, sixel, or iTerm2's
//! inline images all place real pixels, and only a terminal offering none of
//! them falls back to colouring half-block characters. Half-blocks cap the
//! preview at two pixels per cell, which is what makes them look blocky, so
//! they are the fallback rather than the plan.

use crate::live::LiveClient;
use image::DynamicImage;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::StatefulProtocol;
use std::env;

/// Overrides what the terminal reported, for one that answers wrongly or is
/// behind something that swallows the question. One of `kitty`, `sixel`,
/// `iterm2`, or `halfblocks`.
pub const IMAGE_PROTOCOL_ENV: &str = "TERMUTO_IMAGE_PROTOCOL";

/// One episode still. Fetched and decoded off the event loop; the protocol is
/// built on the drawing thread, since it belongs to the terminal.
pub(crate) enum Preview {
    Pending,
    Ready(Box<StatefulProtocol>),
    /// Fetched or decoded unsuccessfully. Kept so a broken URL is not retried
    /// every time the selection passes over it.
    Missing,
}

/// Asks the terminal how it draws images, letting [`IMAGE_PROTOCOL_ENV`] have
/// the last word. A terminal that does not answer leaves half-blocks, which
/// need no support at all.
pub(crate) fn picker() -> Picker {
    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    if let Some(forced) = forced_protocol() {
        picker.set_protocol_type(forced);
    }
    picker
}

fn forced_protocol() -> Option<ProtocolType> {
    match env::var(IMAGE_PROTOCOL_ENV)
        .ok()?
        .trim()
        .to_lowercase()
        .as_str()
    {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" => Some(ProtocolType::Iterm2),
        "halfblocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// What to call the protocol in the preview pane's title. Half-blocks are the
/// one that looks blocky, so which is in use is worth being able to see.
pub(crate) fn protocol_name(protocol: ProtocolType) -> &'static str {
    match protocol {
        ProtocolType::Halfblocks => "halfblocks",
        ProtocolType::Sixel => "sixel",
        ProtocolType::Kitty => "kitty",
        ProtocolType::Iterm2 => "iterm2",
    }
}

/// Tries each candidate for `url` in turn and decodes the first that answers.
pub(crate) async fn fetch(client: &LiveClient, url: &str) -> Option<DynamicImage> {
    for candidate in candidates(url) {
        if let Ok(bytes) = client.fetch_bytes(&candidate).await
            && let Ok(image) = image::load_from_memory(&bytes)
        {
            return Some(image);
        }
    }
    None
}

/// The URLs to try for a still, best first.
///
/// Episode stills come from Crunchyroll's image service, and the size the API
/// hands over — `_large` — is only 200px wide, which no amount of care in the
/// renderer can sharpen. The same path serves `_full` at 640x360. It is not
/// promised anywhere, so the size the API gave stays in the list behind it.
fn candidates(url: &str) -> Vec<String> {
    match url.strip_suffix("_large.jpg") {
        Some(stem) => vec![format!("{stem}_full.jpg"), url.to_string()],
        None => vec![url.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::candidates;

    #[test]
    fn a_crunchyroll_still_is_asked_for_at_full_size_first() {
        assert_eq!(
            candidates("https://img1.ak.crunchyroll.com/i/spire1-tmb/abc123_large.jpg"),
            vec![
                "https://img1.ak.crunchyroll.com/i/spire1-tmb/abc123_full.jpg",
                "https://img1.ak.crunchyroll.com/i/spire1-tmb/abc123_large.jpg",
            ]
        );
    }

    /// A poster from MyAnimeList is already the size it was asked for.
    #[test]
    fn any_other_url_is_left_alone() {
        let url = "https://cdn.myanimelist.net/images/anime/1015/138006l.jpg";
        assert_eq!(candidates(url), vec![url]);
    }
}
