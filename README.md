# termuto-poc

`termuto-poc` is a deliberately small Rust proof of concept showing how one codebase can provide a local anime-catalog CLI and an interactive terminal UI. The installed executable is named `termuto`.

It demonstrates:

- Loading a typed local catalog with the [Deeb](https://github.com/The-Devoyage/deeb) JSON database library.
- Human-readable `latest`, `ongoing`, and case-insensitive `search` CLI commands.
- A Ratatui/Crossterm interface for browsing releases, ongoing series, search results, episodes, and movie details.
- Shared catalog query logic for both frontends.

It deliberately does **not** implement playback, stream resolution, remote APIs, authentication, downloads, history, favourites, preferences, image rendering, synchronization, scraping, plugins, a web UI, or DRM handling. Selecting a movie play action or an episode shows: `Playback is not implemented in this proof of concept.`

## Prerequisites

- A current stable Rust toolchain (the project uses Rust edition 2024).
- A terminal that supports an alternate screen and raw input for the interactive UI.

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run from the project directory with its sample catalog:

```bash
cargo run -- latest
cargo run -- search "solo"
cargo run -- ongoing
cargo run -- tui
```

## Installation

The Cargo package is named `termuto-poc`; it installs a binary named `termuto`.

```bash
cargo install --path . --locked
```

From any directory, supply an absolute catalog path for the first run:

```bash
termuto --catalog /absolute/path/to/catalog.json latest
termuto --catalog /absolute/path/to/catalog.json search "solo"
termuto --catalog /absolute/path/to/catalog.json ongoing
termuto --catalog /absolute/path/to/catalog.json tui
```

Uninstall with:

```bash
cargo uninstall termuto-poc
```

## Catalog path resolution

The catalog path is resolved in this order:

1. Global `--catalog <PATH>` option.
2. `TERMUTO_CATALOG` environment variable.
3. `./catalog.json`.

The global option is accepted before or after a subcommand, for example:

```bash
termuto --catalog ./catalog.json latest --limit 5
termuto search "frieren" --catalog ./catalog.json
TERMUTO_CATALOG=/data/anime.json termuto ongoing
```

If no file exists at the resolved path, the program does not create one. It returns an error including the attempted path and the remediation:

```text
Catalog not found at /path/catalog.json.
Pass --catalog <PATH> or set TERMUTO_CATALOG.
```

## CLI

Running `termuto` with no subcommand launches the TUI. The explicit equivalent is `termuto tui`.

```text
termuto latest [--limit N]
termuto search <QUERY>
termuto ongoing
termuto tui
```

`latest` and `ongoing` render aligned title, type, status, and release-date columns. `search` searches title and alternative titles using a trimmed, case-insensitive substring match. An empty query and an unmatched query are successful commands with no results.

## TUI shortcuts

Home menu entries are **Latest releases**, **Ongoing**, **Search**, and **Quit**.

| Keys | Action |
| --- | --- |
| `l` | Latest releases from Home |
| `o` | Ongoing from Home |
| `/` | Start search |
| `Up` / `k` | Previous item |
| `Down` / `j` | Next item |
| `Enter` | Select/open/play action |
| `Esc` | Go back (including directly from Search) |
| `q` | Quit from any non-search screen |
| `Ctrl-C` | Quit from any screen |

The latest and ongoing screens open a series episode list or movie detail screen. Search updates after each typed character. In the search text input, character keys are input (including `j`, `k`, and `q`); use arrow keys to navigate matching results. `Esc` returns directly to the previous screen.

The TUI enters raw mode and an alternate screen through a guard. Its cleanup path restores raw mode, the normal screen, and cursor visibility after normal exit or an event-loop error.

## Catalog schema and Deeb 0.0.13 behavior

`catalog.json` contains one top-level Deeb collection named `anime`. Each document has these typed fields:

- `id`: nonempty series or movie ID.
- `title`: nonempty primary title.
- `alternative_titles`: title aliases used by search.
- `kind`: `series` or `movie`.
- `status`: `ongoing`, `completed`, or `upcoming`.
- `latest_release_at`: optional RFC 3339 timestamp.
- `description`: text description.
- `episodes`: episode documents with a positive `number`, `title`, and optional `released_at` timestamp.

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
        {"number": 1, "title": "Episode 1", "released_at": "2026-07-20T18:30:00Z"}
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

All records are loaded and queried through this repository. `ongoing` uses Deeb's equality query. Deeb 0.0.13 `Query::like` is case-sensitive and does not conveniently query each alias, so search first retrieves records through Deeb and then performs a normalized case-insensitive substring comparison in the repository. Latest and ongoing sorting is also applied in the repository by typed release timestamp, with undated records last.
