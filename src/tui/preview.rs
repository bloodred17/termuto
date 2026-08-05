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

/// Overrides the cell size, as `WIDTHxHEIGHT` in pixels — `7x15`. Needed only
/// where the terminal reports neither, which is what the trailing `?` in the
/// preview pane's title means.
pub const IMAGE_CELL_ENV: &str = "TERMUTO_IMAGE_CELL";

/// One episode still. Fetched and decoded off the event loop; the protocol is
/// built on the drawing thread, since it belongs to the terminal.
pub(crate) enum Preview {
    Pending,
    Ready(Box<StatefulProtocol>),
    /// Fetched or decoded unsuccessfully. Kept so a broken URL is not retried
    /// every time the selection passes over it.
    Missing,
}

/// How stills get drawn, and whether the terminal actually said so.
pub(crate) struct Renderer {
    picker: Picker,
    /// Whether the cell size was measured rather than assumed. Sixel and iTerm2
    /// state the image's size in pixels, so an assumed cell size draws the
    /// still at the wrong size — which is why a pixel protocol is never chosen
    /// automatically without one.
    measured: bool,
}

impl Renderer {
    /// Works out how to draw, from what the terminal answers and what the
    /// environment says, with [`IMAGE_PROTOCOL_ENV`] having the last word.
    ///
    /// The query alone is not enough. `ratatui-image` never asks about iTerm2
    /// — that query is commented out in the crate — so iTerm2 can only ever be
    /// recognised from the environment. And when a terminal names a protocol
    /// but does not report its cell size, the crate discards the protocol and
    /// returns half-blocks. iTerm2 answers neither question: it does not
    /// implement the cell-size query, so both gates close on it at once. That
    /// is the difference between it and Kitty or Ghostty, which answer both.
    pub(crate) fn detect() -> Self {
        let queried = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
        let mut protocol = queried.protocol_type();
        // The crate keeps a protocol only when a cell size came with it, so
        // anything but half-blocks here means both were answered for.
        let mut measured = protocol != ProtocolType::Halfblocks;
        let mut cell = queried.font_size();

        // Nothing was settled. A cell size from elsewhere makes the
        // environment's word on the protocol safe to act on.
        if !measured && let Some(found) = cell_override().or_else(measured_cell) {
            cell = found;
            measured = true;
            if let Some(named) = protocol_from_env() {
                protocol = named;
            }
        }

        // Applied last of the automatic steps, and to whatever the crate
        // decided as much as to the step above: `ratatui-image` maps VS Code to
        // iTerm2 on its own, and drawing there without the setting turned on
        // leaves nothing on screen at all.
        if image_support_is_opt_in_from_env() {
            protocol = ProtocolType::Halfblocks;
        }

        // Asked for outright, so both are taken at their word — the protocol
        // even without a measured cell, since [`IMAGE_CELL_ENV`] is there to
        // correct the size if the assumed one draws it wrong.
        if let Some(given) = cell_override() {
            cell = given;
            measured = true;
        }
        if let Some(forced) = forced_protocol() {
            protocol = forced;
        }

        Self {
            picker: picker_for(cell, protocol),
            measured,
        }
    }

    /// The renderer that needs nothing from the terminal, for tests.
    #[cfg(test)]
    pub(crate) fn halfblocks() -> Self {
        Self {
            picker: Picker::halfblocks(),
            measured: false,
        }
    }

    pub(crate) fn protocol(&self) -> ProtocolType {
        self.picker.protocol_type()
    }

    pub(crate) fn new_protocol(&self, image: DynamicImage) -> StatefulProtocol {
        self.picker.new_resize_protocol(image)
    }

    /// What the preview pane's title says. Half-blocks are the one that looks
    /// blocky and a mis-measured cell is the one that draws at the wrong size,
    /// so both are worth being able to read off the screen. A trailing `?`
    /// marks a cell size that was assumed rather than reported.
    pub(crate) fn label(&self) -> String {
        let (width, height) = self.picker.font_size();
        let guess = if self.measured { "" } else { "?" };
        format!("{} {width}x{height}{guess}", protocol_name(self.protocol()))
    }
}

/// Builds a picker around a known cell size and protocol. The crate exposes no
/// setter for the cell size, and this constructor is deprecated in favour of
/// the query that cannot answer for the terminals this exists to serve.
fn picker_for(cell: (u16, u16), protocol: ProtocolType) -> Picker {
    #[allow(deprecated)]
    let mut picker = Picker::from_fontsize(cell);
    picker.set_protocol_type(protocol);
    picker
}

/// The cell size in pixels, as the kernel reports it for the terminal. Filled
/// in by terminals that set it on the pty; zeroes mean nobody did, which is
/// usual over SSH.
fn measured_cell() -> Option<(u16, u16)> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.width == 0 || size.height == 0 || size.columns == 0 || size.rows == 0 {
        return None;
    }
    Some((size.width / size.columns, size.height / size.rows))
}

fn cell_override() -> Option<(u16, u16)> {
    parse_cell(&env::var(IMAGE_CELL_ENV).ok()?)
}

fn parse_cell(value: &str) -> Option<(u16, u16)> {
    let value = value.trim().to_lowercase();
    let (width, height) = value.split_once('x')?;
    let width = width.trim().parse().ok().filter(|width| *width > 0)?;
    let height = height.trim().parse().ok().filter(|height| *height > 0)?;
    Some((width, height))
}

fn forced_protocol() -> Option<ProtocolType> {
    parse_protocol(&env::var(IMAGE_PROTOCOL_ENV).ok()?)
}

fn parse_protocol(value: &str) -> Option<ProtocolType> {
    match value.trim().to_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "sixel" => Some(ProtocolType::Sixel),
        "iterm2" => Some(ProtocolType::Iterm2),
        "halfblocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

fn protocol_from_env() -> Option<ProtocolType> {
    protocol_from(|key| env::var(key).ok())
}

/// What the environment says the terminal is, for the ones the capability query
/// cannot reach.
///
/// Terminal identity does not survive SSH intact: `TERM_PROGRAM` is not
/// forwarded, while iTerm2's `LC_TERMINAL` usually is, since `LC_*` is in the
/// default `SendEnv`. Both are checked for that reason.
fn protocol_from(var: impl Fn(&str) -> Option<String>) -> Option<ProtocolType> {
    let has = |key: &str| var(key).is_some_and(|value| !value.trim().is_empty());
    let contains = |key: &str, needle: &str| {
        var(key).is_some_and(|value| value.to_lowercase().contains(needle))
    };

    if has("KITTY_WINDOW_ID") || contains("TERM", "kitty") || contains("TERM", "ghostty") {
        return Some(ProtocolType::Kitty);
    }
    // WezTerm and the rest speak iTerm2's inline images rather than their own.
    // VS Code does too, but only on request — see [`image_support_is_opt_in`].
    const ITERM2_PROGRAMS: [&str; 8] = [
        "iterm",
        "wezterm",
        "mintty",
        "tabby",
        "hyper",
        "rio",
        "bobcat",
        "warpterminal",
    ];
    if ITERM2_PROGRAMS
        .iter()
        .any(|program| contains("TERM_PROGRAM", program))
        || contains("LC_TERMINAL", "iterm")
        || has("ITERM_SESSION_ID")
    {
        return Some(ProtocolType::Iterm2);
    }
    None
}

fn image_support_is_opt_in_from_env() -> bool {
    image_support_is_opt_in(|key| env::var(key).ok())
}

/// Whether the terminal can draw images but will not unless it has been told
/// to, in a way nothing here can ask about.
///
/// VS Code is the case. Its terminal advertises itself exactly as the other
/// terminals borrowing iTerm2's protocol do, and it does implement sixel and
/// iTerm2 inline images — but only behind `terminal.integrated.enableImages`,
/// which is off by default and cannot be queried. Drawing to it while it is off
/// leaves an empty pane, since the escape sequences are simply swallowed, and
/// an empty pane is worse than a blocky one: half-blocks always draw. So it
/// falls back unless [`IMAGE_PROTOCOL_ENV`] asks for otherwise.
fn image_support_is_opt_in(var: impl Fn(&str) -> Option<String>) -> bool {
    var("TERM_PROGRAM").is_some_and(|value| value.to_lowercase().contains("vscode"))
}

fn protocol_name(protocol: ProtocolType) -> &'static str {
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
    use super::{
        ProtocolType, Renderer, candidates, image_support_is_opt_in, parse_cell, parse_protocol,
        protocol_from,
    };

    /// Looks up from a fixed set rather than the process environment, which is
    /// shared by every test running alongside this one.
    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_string())
        }
    }

    /// iTerm2 is the reason this exists: `ratatui-image` has its iTerm2 query
    /// commented out, so nothing but the environment can name it.
    #[test]
    fn iterm2_is_recognised_by_the_name_it_leaves_in_the_environment() {
        assert_eq!(
            protocol_from(env(&[("TERM_PROGRAM", "iTerm.app")])),
            Some(ProtocolType::Iterm2)
        );
        // Over SSH `TERM_PROGRAM` does not survive but `LC_TERMINAL` does.
        assert_eq!(
            protocol_from(env(&[
                ("TERM", "xterm-256color"),
                ("LC_TERMINAL", "iTerm2")
            ])),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            protocol_from(env(&[("ITERM_SESSION_ID", "w0t0p0:1234")])),
            Some(ProtocolType::Iterm2)
        );
    }

    /// VS Code implements iTerm2's protocol but keeps it behind a setting that
    /// is off by default and cannot be queried, so drawing to it uninvited
    /// leaves an empty pane. Half-blocks always draw, so it has to opt in.
    #[test]
    fn vscode_is_left_on_halfblocks_until_it_asks_otherwise() {
        assert!(image_support_is_opt_in(env(&[("TERM_PROGRAM", "vscode")])));
        assert_eq!(protocol_from(env(&[("TERM_PROGRAM", "vscode")])), None);

        assert!(!image_support_is_opt_in(env(&[(
            "TERM_PROGRAM",
            "iTerm.app"
        )])));
        assert!(!image_support_is_opt_in(env(&[("TERM", "xterm-ghostty")])));
    }

    /// These speak iTerm2's inline images rather than a protocol of their own.
    #[test]
    fn the_terminals_borrowing_iterm2s_protocol_are_named_too() {
        for program in ["WezTerm", "Hyper", "WarpTerminal"] {
            assert_eq!(
                protocol_from(env(&[("TERM_PROGRAM", program)])),
                Some(ProtocolType::Iterm2),
                "{program}"
            );
        }
    }

    #[test]
    fn kitty_and_ghostty_are_recognised_as_kitty() {
        assert_eq!(
            protocol_from(env(&[("TERM", "xterm-ghostty")])),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(
            protocol_from(env(&[("KITTY_WINDOW_ID", "1")])),
            Some(ProtocolType::Kitty)
        );
    }

    /// A terminal that says nothing is left alone rather than guessed at: the
    /// wrong protocol draws nothing at all, where half-blocks always draw.
    #[test]
    fn an_anonymous_terminal_is_not_guessed_at() {
        assert_eq!(protocol_from(env(&[("TERM", "xterm-256color")])), None);
        assert_eq!(protocol_from(env(&[("KITTY_WINDOW_ID", "  ")])), None);
        assert_eq!(protocol_from(env(&[])), None);
    }

    #[test]
    fn the_protocol_override_takes_the_four_names() {
        assert_eq!(parse_protocol(" Kitty "), Some(ProtocolType::Kitty));
        assert_eq!(parse_protocol("iterm2"), Some(ProtocolType::Iterm2));
        assert_eq!(parse_protocol("sixel"), Some(ProtocolType::Sixel));
        assert_eq!(parse_protocol("halfblocks"), Some(ProtocolType::Halfblocks));
        assert_eq!(parse_protocol("nonsense"), None);
    }

    /// The title is the whole diagnosis: which protocol, at what cell size,
    /// and whether that size was reported or assumed.
    #[test]
    fn the_label_names_the_protocol_and_flags_an_assumed_cell_size() {
        let assumed = Renderer {
            picker: super::picker_for((10, 20), ProtocolType::Iterm2),
            measured: false,
        };
        assert_eq!(assumed.label(), "iterm2 10x20?");

        let measured = Renderer {
            picker: super::picker_for((7, 15), ProtocolType::Kitty),
            measured: true,
        };
        assert_eq!(measured.label(), "kitty 7x15");
    }

    /// A queried protocol and cell size have to survive being rebuilt, since
    /// every renderer goes through this — including the terminals that already
    /// answered for themselves.
    #[test]
    fn a_rebuilt_picker_keeps_its_cell_size_and_protocol() {
        let picker = super::picker_for((9, 18), ProtocolType::Kitty);
        assert_eq!(picker.font_size(), (9, 18));
        assert_eq!(picker.protocol_type(), ProtocolType::Kitty);
    }

    #[test]
    fn the_cell_override_is_read_as_width_by_height() {
        assert_eq!(parse_cell("7x15"), Some((7, 15)));
        assert_eq!(parse_cell(" 10X20 "), Some((10, 20)));
        // A zero would divide the image into nothing.
        assert_eq!(parse_cell("0x15"), None);
        assert_eq!(parse_cell("7"), None);
        assert_eq!(parse_cell("axb"), None);
    }

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
