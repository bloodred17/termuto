# termuto

[![CI](https://github.com/bloodred17/termuto/actions/workflows/ci.yml/badge.svg)](https://github.com/bloodred17/termuto/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/bloodred17/termuto)](https://github.com/bloodred17/termuto/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`termuto` is a small Rust program that provides an anime CLI and an interactive terminal UI over more than one data source. Jump to [Installation](#installation).

It demonstrates:

- Three interchangeable data sources chosen by a single `mode` option: the live [Tenrai API](https://api.tenrai.org/documentation), a local JSON catalog, or both.
- Live browsing of top anime, broadcast seasons, and user recommendations, with full details for any title.
- Loading a typed local catalog with the [Deeb](https://github.com/The-Devoyage/deeb) JSON database library.
- Human-readable `top`, `season`, `seasons`, `recommendations`, `latest`, `ongoing`, and `search` CLI commands.
- A Ratatui/Crossterm interface that draws a loading frame before each request and turns a failed request into a dismissible overlay instead of ending the session.
- Playback through an external player, behind a provider chain that resolves a title and episode into something playable.
- Shared query and row-formatting logic behind both frontends.

It deliberately does **not** implement authentication, downloads, history, favourites, synchronization, plugins, a web UI, or DRM handling. Stream resolution is a pluggable interface with two host extractors behind it: see [Playback](#playback).

## Modes

The mode decides where titles come from.

| Mode | Source | Notes |
| --- | --- | --- |
| `live` | The Tenrai API | The default. No catalog file needed. |
| `cached` | The local Deeb catalog | Fully offline. The API-only listings are unavailable. |
| `hybrid` | Both | Catalog rows first, API rows appended and de-duplicated by title. A failed API call degrades to the catalog instead of failing the screen, and a missing catalog degrades to the API. |

Resolution order:

1. Global `--mode <cached|live|hybrid>` option.
2. `TERMUTO_MODE` environment variable.
3. `live`.

```bash
termuto top                                     # live, the default
TERMUTO_MODE=hybrid termuto search "frieren"
termuto --mode cached --catalog ./catalog.json latest
```

An unrecognised mode is rejected rather than silently falling back.

## API endpoints used

Live and hybrid modes read `https://api.tenrai.org/v1`:

| Screen | Endpoint |
| --- | --- |
| Top anime | `GET /top/anime` |
| Airing now | `GET /top/anime?filter=airing` |
| This season | `GET /seasons/now` |
| Browse seasons | `GET /seasons`, then `GET /seasons/{year}/{season}` |
| Recommendations | `GET /recommendations/anime` |
| Search | `GET /anime?q=…` |
| Title details | `GET /anime/{id}/full` |

Set `TERMUTO_API_BASE` to point at a mirror; it defaults to the URL above. Responses are decoded permissively: unknown fields are ignored, an entry that cannot be decoded is dropped rather than failing its page, and partial air dates such as `2026-10` are accepted.

## Prerequisites

- A terminal that supports an alternate screen and raw input for the interactive UI.
- Network access for `live` and `hybrid` modes.
- [mpv](https://mpv.io) on `PATH` for playback, or another player named with `--player`.
- Rust 1.88 or newer, but only to build from source. The prebuilt binaries need no toolchain.

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

The test suite runs offline: every test either pins `--mode cached` or asserts on mode handling itself. The playback tests pass `--player true`, so resolving and spawning are exercised without opening a window.

## CLI

Running `termuto` with no subcommand launches the TUI. The explicit equivalent is `termuto tui`.

```text
termuto top [--limit N]
termuto season [--year Y --season S] [--limit N]
termuto seasons
termuto recommendations [--limit N]
termuto latest [--limit N]
termuto ongoing [--limit N]
termuto search <QUERY> [--limit N]
termuto play <QUERY> [--episode N]
termuto tui
```

```bash
cargo run -- top --limit 25
cargo run -- season                              # the season airing now
cargo run -- season --year 2023 --season fall
cargo run -- seasons                             # what the API holds
cargo run -- search "solo leveling"
```

Listings render aligned title, type, status, score, episode-count, and release-date columns; a column the source cannot fill shows `—`. Recommendations carry only titles, so they print as a title per row with the title each was recommended from beneath it.

`--year` and `--season` are given together or not at all. `top`, `season`, `seasons`, and `recommendations` need the API, so under `--mode cached` they report the mode requirement rather than guessing. In `cached` mode, `search` matches titles and alternative titles with a trimmed, case-insensitive substring comparison; an empty or unmatched query is a successful command with no results.

The global options are accepted before or after a subcommand:

```bash
termuto --mode hybrid --catalog ./catalog.json latest --limit 5
termuto search "frieren" --mode live
TERMUTO_CATALOG=/data/anime.json termuto --mode cached ongoing
```

## Terminal UI

```bash
cargo run -- tui        # or just `cargo run --`
```

The home menu follows the mode. With the API in play it offers **Top anime**, **This season**, **Browse seasons**, **Recommendations**, **Airing now**, **Search**, and **Quit**; in `cached` mode it offers **Latest releases**, **Airing now**, **Search**, and **Quit**.

| Keys | Action |
| --- | --- |
| `Up` / `k` | Previous item |
| `Down` / `j` | Next item |
| `PgUp` / `PgDn` | Page through a list, or scroll a detail |
| `Enter` | Select, open, or play |
| `/` | Start a search |
| `n` | Sort the list by name; again for Z–A |
| `d` | Sort the list by date; again for oldest first |
| `f` | Filter the list by name, as you type |
| `t` | Step through the types the list holds, and back to all of them |
| `p` | Switch which host streams resolve from |
| `a` | Toggle autoswitch (shown as `auto` in the header) |
| `v` | Show or hide the episode still (episode picker, on by default) |
| `s` | Show or hide the episode synopsis (episode picker, on by default) |
| `Esc` | Go back |
| `q` | Quit from any non-search screen |
| `Ctrl-C` | Quit from any screen |
| `y` / `n` | Answer the quit prompt |

Opening a row loads its details: `/anime/{id}/full` for an API row, and the episode list or movie screen for a catalog row. Live searches run on `Enter` rather than on each keystroke, so typing does not fire a request per character; `↓` moves from the query into the results and `Esc` moves back.

Every list — titles, search results, episodes, and the season index — can be reordered and narrowed without another request. `n` and `d` sort by name and by date, and pressing the same key again reverses it; rows the source has no date for stay at the bottom either way. `f` starts a filter that matches anywhere in a row's name and narrows the list on each keystroke: `Enter` keeps it and hands the keys back to the list, `Esc` abandons it. `t` steps through the types a listing holds — `TV`, `Movie`, `OVA`, and whatever else is in its **TYPE** column — and past the last one back to all of them; the types on offer are only the ones the name filter has left, so `t` never lands on an empty screen, and the two filters narrow together. A list with no type column, such as an episode picker, has nothing to step through and `t` does nothing there. How a list is ordered and how much the filter is holding back are shown under its bottom border, and a sort or a filter moves the rows under the highlight rather than moving the highlight, so `Enter` still opens what it was pointing at. Loading a new list starts it over in the order its source chose. Lists keep their own settings, so filtering a listing and stepping into a title does not filter its episodes too.

`Enter` on a title plays it. A catalog series and a live series both open an episode list first; a movie plays straight away. Once the player has the stream, a **Now playing** overlay reports which provider answered and what was handed over, and any key dismisses it.

The episode picker for a live series is filled from `/anime/{id}/episodes`: each row carries the episode's title, air date, and score, and beside the list sit two panes for whichever episode is selected — its still, and its synopsis with the runtime in the pane's title. `v` and `s` toggle the panes, both start open, and on a terminal narrower than 76 columns neither is drawn, so the list keeps the whole width.

That endpoint is paged, and a long-running or still-airing title has more episodes than one fetch reaches. Whatever it does not cover is still listed and still playable, numbered from the count the title itself reports. A failed episode fetch degrades to that numbering rather than raising an error over the screen.

Stills are fetched and decoded on their own task, so moving down the list never waits on the network; each one is kept by URL, so scrolling back redraws immediately, and the cache is trimmed to the one on screen once it passes two dozen. An episode the API has no still for falls back to the title's poster.

**Drawing them sharply** takes two things, and missing either one is what makes a preview look blocky.

The first is a terminal that draws pixels. On startup the terminal is asked what it supports, and the answer picks the protocol: Kitty graphics, sixel, and iTerm2 inline images all place real pixels; a terminal offering none of them falls back to colouring half-block characters, `▀`, which caps the preview at two pixels per cell no matter how large the pane.

The question needs two answers, though, and asking is not enough on its own:

- **Which protocol.** `ratatui-image` has its iTerm2 query commented out, so iTerm2 is never detected by asking — only by the name it leaves in the environment. `TERM_PROGRAM` carries that locally but does not survive SSH; iTerm2's `LC_TERMINAL` usually does, since `LC_*` is in OpenSSH's default `SendEnv`. Both are checked, along with `ITERM_SESSION_ID`, and the terminals that borrow iTerm2's protocol rather than defining one — WezTerm, VS Code, Hyper, Warp, and the rest.
- **How big a cell is.** Sixel and iTerm2 state an image's size in pixels, so drawing to an assumed cell size lands the still at the wrong size. The cell-size query is `CSI 16 t`, which Kitty and Ghostty answer and iTerm2 does not; the fallback is the pixel size the kernel holds for the pty, which is usually zero over SSH. Without a real cell size a pixel protocol is never chosen on its own — half-blocks always draw at *some* size, and a wrong size is worse than a blocky one.
- **Whether it will actually draw.** VS Code's terminal names itself exactly as the terminals borrowing iTerm2's protocol do, and does implement sixel and iTerm2 inline images — but only behind [`terminal.integrated.enableImages`](https://code.visualstudio.com/docs/terminal/advanced), which is off by default and cannot be queried. Drawing to it while that is off swallows the escape sequences and leaves an *empty* pane, which is worse than a blocky one. So VS Code stays on half-blocks until `TERMUTO_IMAGE_PROTOCOL` says otherwise, overriding `ratatui-image`, which maps it to iTerm2 on its own.

The pane's title reports both, so the diagnosis is one glance: `Preview · kitty 10x20` against `Preview · halfblocks 10x20?`, where the trailing `?` means the cell size was assumed rather than reported. Two variables override the answers — `TERMUTO_IMAGE_PROTOCOL` (`kitty`, `sixel`, `iterm2`, `halfblocks`) and `TERMUTO_IMAGE_CELL` (`7x15`) — and both are taken at their word, since a terminal that reports nothing cannot be argued with.

The second is a source worth drawing. Stills come from Crunchyroll's image service, and the size the API hands over — `_large` — is only 200px wide, which no renderer can sharpen. The same path serves `_full` at 640×360, so that is asked for first, with the API's own URL behind it in case a title does not have one.

`p` cycles which host is asked first, and `a` toggles autoswitch; the header shows the current host and, while autoswitch is on, `auto`. Both take effect on the next play, not on anything already handed to the player. Like `q`, they are ordinary letters, so they reach the query instead while a search is being typed.

```text
┌─ termuto ────────────────────────────────────────────────────────────┐
│ termuto — anime from the terminal · mode: live · player: mpv ·        │
│ provider: zokoanime · auto                                           │
└──────────────────────────────────────────────────────────────────────┘
```

Every request is queued, drawn as a loading frame, and only then awaited, so a slow response shows progress instead of a frozen UI. Resolving a stream is queued the same way. A failed request raises an overlay that any key dismisses, returning to the previous screen.

The TUI enters raw mode and an alternate screen through a guard. Its cleanup path restores raw mode, the normal screen, and cursor visibility after normal exit or an event-loop error.

## Playback

Playback is two independent steps, and each is replaceable on its own.

**Resolution.** A selected title and episode number go to a *provider chain*. Each provider implements `StreamProvider` and is asked in turn. A provider that does not serve the request returns nothing and the chain moves on; a provider that owned the request and then failed has its reason kept, and that reason is reported only if nothing behind it succeeds. One dead provider therefore cannot block a working one.

Three providers ship:

| Provider | Serves | Notes |
| --- | --- | --- |
| `catalog` | Catalog rows with a `source` | Plays the local path or URL the catalog points at. A row without a `source` is passed on rather than failed. |
| `zokoanime` | API rows | Scrapes ZokoAnime for the HLS rendition behind a MyAnimeList id. Plays directly. |
| `megavid` | API rows | Same lookup, wider coverage, but its segments need the local proxy. |

Extractors scrape hosts whose endpoints move, so each provider is one self-contained implementation. Replacing a dead extractor is a one-file change and never touches the frontends. Add one by inserting it into `ProviderChain::with_catalog` in [`src/playback/provider.rs`](src/playback/provider.rs). Master-playlist parsing and rendition choice are the one thing every host shares, so they live in [`src/playback/hls.rs`](src/playback/hls.rs) rather than in each extractor.

**Choosing a host.** The catalog always leads — a local file beats a scrape. Behind it, `zokoanime` is asked first because it plays without a proxy, and `megavid` follows. Press `p` in the TUI, or pass `--provider`, to put a different host first. An unknown name is rejected rather than ignored, which would otherwise look like the choice took effect.

**Autoswitch** decides whether a host with nothing to offer hands over to the next one. It is **on** by default, and the header shows `auto` while it is; `a` toggles it, and `--autoswitch off` turns it off for a run.

It is worth having as a toggle because the two things it trades off are both real. Coverage differs per host and *moves*: over the course of writing this, `frieren` went from missing on ZokoAnime to present, while `bocchi the rock` is currently the reverse. Falling through is what makes such a title playable at all. But a fallback is a different stream — possibly a different rendition, subtitle set, or cut — so when you have chosen a host deliberately, silently getting another one is the wrong answer. Off pins playback to the host you picked:

```text
$ termuto --mode live --autoswitch off play "bocchi the rock"
Error: No provider could resolve Bocchi the Rock! episode 1.
zokoanime: ZokoAnime has nothing to play for Bocchi the Rock! episode 1 (MyAnimeList id 47917).
sub: We couldn't find anything to play here. The episode may have been removed or the link is incorrect.
dub: We couldn't find anything to play here. The episode may have been removed or the link is incorrect.
Autoswitch is off, so megavid was not tried.
```

That last line is the point: without it, "no provider could resolve this" reads as "no host has it" when a host that was held back might well have. The catalog is not a host to switch between, so autoswitch has no say over it — a catalog row with a `source` still plays either way.

**ZokoAnime**, in [`src/playback/zoko.rs`](src/playback/zoko.rs), is keyed on the MyAnimeList id an API row already carries, so no title matching is involved: `/stream/mal/{mal_id}/{episode}/{sub|dub}` is the player page. The page ships its configuration inline as `window.__P` — base64 of the JSON, XOR'd against a fixed key by the site's own `/core/obfuscate.js` — which is undone to reach the HLS master playlist and the external subtitle tracks. Three things follow from what the host actually serves:

- **The rendition is chosen here, not by the player.** The master playlist lists its variants worst-first, so a player left to pick would open 360p. `--quality` is matched against the variants' resolutions, and a rendition the host does not carry degrades to the nearest one it does rather than failing.
- **The other audio track is a fallback.** The requested track is tried first; a title carrying only one of the two still plays, and the report names what was served rather than what was asked for.
- **A removed episode answers `200`.** The host returns an error card in place of the payload, so a missing payload is detected and the card's own wording is what surfaces, per track:

  ```text
  $ termuto --mode live play "mushoku tensei iii" --episode 13
  Error: No provider could resolve Mushoku Tensei: Jobless Reincarnation Season 3 episode 13.
  zokoanime: ZokoAnime has nothing to play for … episode 13 (MyAnimeList id 59193).
  sub: We couldn't find anything to play here. The episode may have been removed or the link is incorrect.
  dub: We couldn't find anything to play here. The episode may have been removed or the link is incorrect.
  megavid: MegaVid has nothing to play for … episode 13 (MyAnimeList id 59193).
  sub: We couldn't find this episode. It may not exist, isn't available yet, or has been removed.
  dub: We couldn't find this episode. It may not exist, isn't available yet, or has been removed.
  ```

  Every host that owned the request reports in its own words, so an episode that is genuinely gone is distinguishable from one host having a bad day.

`TERMUTO_ZOKO_BASE` overrides the host, mainly for when it moves domain.

**MegaVid**, in [`src/playback/megavid.rs`](src/playback/megavid.rs), is keyed the same way but hands its configuration over as JSON instead of hiding it in the page: `GET /mal/{id}/{episode}/{sub|dub}/source` answers with the master playlist and every caption track. Rendition choice, the audio-track fallback, and reporting a missing episode in the host's own words all work as above — a removed episode answers `200` with `{"status":"missing"}` rather than a 404, so the status is what decides. Two differences are worth knowing:

- **It carries titles ZokoAnime does not**, which is why it is in the chain at all. Which titles those are changes over time, so the chain covers the gap rather than any one host being right.
- **Its segments are disguised, and that costs a proxy** — see below.

`TERMUTO_MEGAVID_BASE` overrides the host.

**A host that disguises its segments needs the local proxy.** MegaVid prefixes every segment with a small valid PNG, so a segment probes as an image and playback never starts:

```text
$ ffprobe -show_entries stream=codec_name segment.ts
png,video                       # not h264 — the demuxer never reaches the video
```

No player can be told to skip those bytes, because the fetch happens inside its own HLS demuxer. [`src/playback/proxy.rs`](src/playback/proxy.rs) therefore binds a loopback port and serves `GET /?u=<upstream>`: a playlist comes back with every URI rewritten to point at the proxy, so the player follows the whole tree through it, and anything else is served with its decoy header removed. The host's headers are attached upstream, since the CDN answers `403` without them.

The proxy runs **in this process**, which is the one place playback is not fire-and-forget. The TUI is unaffected — it is running anyway — but `termuto play` would kill the proxy by exiting, so for a proxied stream it stays open until the player exits and says so. Every other stream still returns the moment the player is up:

```text
$ termuto --mode live play "bocchi the rock"
Playing Bocchi the Rock! episode 1 via megavid (sub 1080p) in mpv
https://megavid.buzz/vid/…/index-f1-v1-a1.m3u8
Player output: /tmp/termuto-player.log
Proxying segments on 127.0.0.1:44465 — this command stays open until the player exits.
```

The URL reported is the host's own, not the `127.0.0.1` one the player was given: a loopback address would say nothing about which host answered or whether the match was right.

**A provider must return the headers its host demands, not just a URL.** CDNs serving embedded streams check `Referer`, `Origin`, and `User-Agent`, and answer `403` to a request without them. A `Stream` therefore carries headers alongside its URL, and the player is given them — for mpv, as `--http-header-fields`. This is the difference between a stream that plays and one that fails before the first frame.

**Playing.** The resolved stream is handed to an external player, spawned detached: it opens in its own window, the terminal is never given up, and the TUI keeps drawing. Nothing waits on the player — except a one-shot `play` of a proxied stream, which has to, per above — so finished ones are reaped before each new spawn rather than lingering for the session. mpv is given the media title, any headers the provider attached, and any subtitle tracks it supplied; any other player is given the URL alone.

Because nothing waits on it, a player that dies on its first request would fail invisibly. Its stderr is therefore appended to `$TMPDIR/termuto-player.log` rather than discarded — writing it to the terminal would corrupt the TUI's alternate screen. Both frontends report the path, so `nothing happened` is always diagnosable:

```text
$ tail -2 /tmp/termuto-player.log
[ffmpeg] https: HTTP error 403 Forbidden
Failed to open https://…/index.m3u8.
```

```bash
termuto play "solo leveling"                       # first episode of the best match
termuto play "solo leveling" --episode 4
termuto --mode live --audio dub --quality 720 play "mushoku tensei iii"
termuto --player vlc play "look back"
termuto --mode live --provider megavid play "bocchi the rock"
termuto --mode live --autoswitch off play "frieren"    # this host or nothing
```

`play` reports which title it matched, which provider answered, and what it handed over, so a wrong match is visible rather than silent. An episode number the title does not have is rejected against the episode list, or against the API's episode count, before any provider is asked.

| Option | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--audio <sub\|dub>` | `TERMUTO_AUDIO` | `sub` | Which audio track to ask for |
| `--quality <best\|1080>` | `TERMUTO_QUALITY` | `best` | Preferred rendition; `1080` and `1080p` are the same request |
| `--player <NAME>` | `TERMUTO_PLAYER` | `mpv` | The player to hand streams to |
| `--provider <NAME>` | `TERMUTO_PROVIDER` | `zokoanime` | Host asked first; the rest stay on as fallbacks |
| `--autoswitch <on\|off>` | `TERMUTO_AUTOSWITCH` | `on` | Whether a host with nothing to offer falls through to the next |

All five are global, so they apply to the TUI as well. Like `--mode`, an unrecognised value is rejected rather than silently falling back, and what a provider actually served is reported rather than assumed to be what was asked for.

## Installation

Every route installs a single binary named `termuto`. Nothing else is written
until you ask for a catalog, so `termuto top`, `termuto season`, and `termuto tui`
work immediately: `live` is the default mode.

### Install script

Downloads the release build for your platform, verifies it against the release's
`SHA256SUMS`, and installs it to `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/bloodred17/termuto/main/install.sh | sh
```

Set `TERMUTO_BIN_DIR` to install somewhere else, or `TERMUTO_VERSION=v0.1.0` to
pin a release instead of taking the latest.

### Homebrew

```bash
brew install bloodred17/termuto/termuto
```

### Prebuilt binaries

Grab a tarball from the [releases page](https://github.com/bloodred17/termuto/releases)
and put `termuto` on your `PATH`. Builds are published for:

| Platform | Target |
| --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple silicon | `aarch64-apple-darwin` |

Linux builds are made on Ubuntu 22.04, so they need glibc 2.35 or newer. Each
release also carries `SHA256SUMS`; verify with `sha256sum -c SHA256SUMS`.

macOS builds are unsigned, so Gatekeeper will quarantine a downloaded tarball.
Clear it with `xattr -d com.apple.quarantine ./termuto`, or install through
Homebrew, which handles this.

### From source

Needs Rust 1.88 or newer (the project uses edition 2024).

```bash
cargo install --git https://github.com/bloodred17/termuto --locked
```

Or from a checkout:

```bash
cargo install --path . --locked
```

### Uninstall

```bash
rm ~/.local/bin/termuto      # install script
brew uninstall termuto       # homebrew
cargo uninstall termuto      # cargo install
```

The catalog and its directory, if you created one, are left alone; remove
`~/.termuto` yourself.

## Local catalog

Only `cached` and `hybrid` modes read a catalog. Install one at the default location:

```bash
mkdir -p ~/.termuto
cp catalog.json ~/.termuto/catalog.json
```

The catalog path is resolved in this order:

1. Global `--catalog <PATH>` option.
2. `TERMUTO_CATALOG` environment variable.
3. `~/.termuto/catalog.json`.

If the home directory cannot be determined, the last step falls back to `./catalog.json`. The program never creates a catalog. A missing file is an error in `cached` mode:

```text
Catalog not found at /home/you/.termuto/catalog.json.
Pass --catalog <PATH>, set TERMUTO_CATALOG, or place a catalog at ~/.termuto/catalog.json.
```

In `hybrid` mode the same situation is a warning on stderr, and the session continues against the API alone.

## Catalog schema and Deeb 0.0.13 behavior

`catalog.json` contains one top-level Deeb collection named `anime`. Each document has these typed fields:

- `id`: nonempty series or movie ID.
- `title`: nonempty primary title.
- `alternative_titles`: title aliases used by search.
- `kind`: `series` or `movie`.
- `status`: `ongoing`, `completed`, or `upcoming`.
- `latest_release_at`: optional RFC 3339 timestamp.
- `description`: text description.
- `source`: optional path or URL a **movie** plays from.
- `episodes`: episode documents with a positive `number`, `title`, optional `released_at` timestamp, and optional `source` path or URL.

`source` is what makes a catalog row playable. It is optional on purpose: a row without one is metadata only, and playback falls through to the next provider. In the included catalog, `solo-leveling-season-2` and `look-back` carry one and `frieren` does not, so both paths are exercised. Nothing behind the catalog can stand in for a missing `source`, though: both extractors are addressed by MyAnimeList id, which a catalog row does not carry, so `frieren` reports that no provider served it rather than playing something else.

For this POC, series need at least one episode, movies must have none, anime IDs and per-series episode numbers must be unique, and invalid records are errors.

The Deeb 0.0.13 crate currently deserializes on-disk collections as JSON objects keyed by document name/primary key, despite its README's array-shaped quick-start example. The included working catalog therefore uses this Deeb-native form:

```json
{
  "anime": {
    "solo-leveling-season-2": {
      "id": "solo-leveling-season-2",
      "title": "Solo Leveling Season 2",
      "alternative_titles": ["Ore dake Level Up na Ken Season 2"],
      "kind": "series",
      "status": "ongoing",
      "latest_release_at": "2026-08-03T18:30:00Z",
      "description": "Sample series used by the proof of concept.",
      "episodes": [
        {
          "number": 1,
          "title": "Episode 1",
          "released_at": "2026-07-20T18:30:00Z",
          "source": "media/solo-leveling-season-2/01.mkv"
        }
      ]
    }
  }
}
```

This difference is necessary for the published `deeb = 0.0.13` API to load the file without an application-side JSON parser.

## How Deeb is used

`Anime` derives `Collection`, `Serialize`, and `Deserialize` with `#[deeb(name = "anime", primary_key = "id")]`. The repository checks that the file exists first (so Deeb does not create an empty database), creates `Deeb::new()`, and awaits:

```rust
db.add_instance("catalog", catalog_path, vec![Anime::entity()]).await?;
let anime = Anime::find_many(&db, Query::all(), None, None).await?;
```

All catalog records are loaded and queried through this repository. `ongoing` uses Deeb's equality query. Deeb 0.0.13 `Query::like` is case-sensitive and does not conveniently query each alias, so search first retrieves records through Deeb and then performs a normalized case-insensitive substring comparison in the repository. Latest and ongoing sorting is also applied in the repository by typed release timestamp, with undated records last.

## Layout

```text
src/mode.rs        the cached/live/hybrid option and how it is resolved
src/live/          the Tenrai API client and the payloads it decodes
src/catalog/       the Deeb-backed local catalog
src/source/        one query surface over both, plus the shared row/detail view models
src/playback/      stream resolution (the provider chain) and the external player
src/cli.rs         the command-line frontend
src/tui/           the interactive frontend
```

Both frontends talk only to `source::Source` and `playback::Playback`, so neither knows which mode is active nor which provider resolved a stream.

## Releasing

Releases are cut from a tag. The version in `Cargo.toml` and the tag must agree,
or the workflow stops before building anything.

```bash
# 1. bump the version in Cargo.toml, then refresh the lockfile
cargo build --locked || cargo update -p termuto
git commit -am "release: v0.2.0"

# 2. tag and push
git tag -a v0.2.0 -m "v0.2.0"
git push origin main --follow-tags
```

`.github/workflows/release.yml` then builds all four targets, writes
`SHA256SUMS`, publishes a GitHub release with generated notes, and commits an
updated formula to the `bloodred17/homebrew-termuto` tap. The tap step needs a
`HOMEBREW_TAP_TOKEN` repository secret with `contents: write` on the tap repo; it
warns and skips if the secret is missing, and is skipped for prerelease versions
(any version containing `-`).

## Legal

`termuto` is a client. It ships no media, hosts nothing, and includes no
credentials or DRM circumvention. Listings come from a public metadata API, and
playback hands a URL resolved from a third-party host to a player you supply.

Those hosts are not licensed distributors, and neither this project nor its
author is affiliated with them or with any rights holder. Whether streaming from
them is lawful is your responsibility and depends on where you are. Use a
licensed service for anything you intend to watch.

## License

MIT. See [LICENSE](LICENSE).
