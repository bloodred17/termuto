use crate::catalog::AnimeKind;
use crate::library::{Library, resolve_library_path};
use crate::mode::{MODE_ENV, Mode, resolve_mode};
use crate::playback::{
    Audio, PROVIDER_ENV, Playback, Quality, Switch, resolve_autoswitch, resolve_player,
    resolve_prefs,
};
use crate::source::{AnimeDetail, AnimeSummary, SeasonRef, Source};
use crate::tui;
use anyhow::{Result, anyhow, bail};
use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "termuto",
    about = "Browse anime from the terminal, live from the Tenrai API or from a local catalog",
    version
)]
pub struct Cli {
    /// Where titles are read from (defaults to TERMUTO_MODE or live)
    #[arg(long, global = true, value_name = "MODE", value_enum)]
    pub mode: Option<Mode>,

    /// Path to the Deeb JSON catalog (defaults to TERMUTO_CATALOG or ~/.termuto/catalog.json)
    #[arg(long, global = true, value_name = "PATH")]
    pub catalog: Option<PathBuf>,

    /// Path to the favourites and watch history (defaults to TERMUTO_LIBRARY or
    /// ~/.termuto/library.json)
    #[arg(long, global = true, value_name = "PATH")]
    pub library: Option<PathBuf>,

    /// Which audio track playback asks for (defaults to TERMUTO_AUDIO or sub)
    #[arg(long, global = true, value_name = "AUDIO", value_enum)]
    pub audio: Option<Audio>,

    /// Preferred rendition, e.g. 1080 or best (defaults to TERMUTO_QUALITY or best)
    #[arg(long, global = true, value_name = "QUALITY")]
    pub quality: Option<Quality>,

    /// Player to hand streams to (defaults to TERMUTO_PLAYER or mpv)
    #[arg(long, global = true, value_name = "PLAYER")]
    pub player: Option<String>,

    /// Host to resolve streams from first, e.g. megavid (defaults to
    /// TERMUTO_PROVIDER or zokoanime). The others stay on as fallbacks.
    #[arg(long, global = true, value_name = "PROVIDER")]
    pub provider: Option<String>,

    /// Whether a host with nothing to offer falls through to the next one
    /// (defaults to TERMUTO_AUTOSWITCH or on)
    #[arg(long, global = true, value_name = "ON|OFF", value_enum)]
    pub autoswitch: Option<Switch>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive terminal UI
    Tui,
    /// List the highest ranked titles (live and hybrid modes)
    Top {
        /// Maximum number of titles to show
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// List a broadcast season, defaulting to the one airing now
    Season {
        /// Season year, e.g. 2023 (requires --season)
        #[arg(long)]
        year: Option<u32>,
        /// winter, spring, summer, or fall (requires --year)
        #[arg(long, value_name = "SEASON")]
        season: Option<String>,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// List the years and seasons the API holds titles for
    Seasons,
    /// List recent user recommendations (live and hybrid modes)
    Recommendations {
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// List newest titles: the current season live, newest releases cached
    Latest {
        /// Maximum number of titles to show
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search titles (case-insensitive)
    Search {
        query: String,
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// List titles that are still airing
    Ongoing {
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// Resolve a stream for the best match of QUERY and open it in the player
    Play {
        query: String,
        /// Episode number, defaulting to the first (ignored for a movie)
        #[arg(long, value_name = "N")]
        episode: Option<u32>,
    },
}

pub async fn run(cli: Cli) -> Result<()> {
    let mode = resolve_mode(cli.mode).map_err(|message| anyhow!(message))?;
    let prefs = resolve_prefs(cli.audio, cli.quality).map_err(|message| anyhow!(message))?;
    let source = Source::open(mode, resolve_catalog_path(cli.catalog)).await?;
    if let Some(issue) = source.catalog_issue() {
        eprintln!("warning: continuing without the local catalog — {issue}");
    }
    let autoswitch = resolve_autoswitch(cli.autoswitch).map_err(|message| anyhow!(message))?;
    let mut playback = Playback::for_source(&source, prefs, resolve_player(cli.player))?;
    playback.set_autoswitch(autoswitch);
    if let Some(provider) = resolve_provider(cli.provider) {
        playback.prefer_provider(&provider)?;
    }

    match cli.command {
        None | Some(Command::Tui) => {
            let library = Library::open(resolve_library_path(cli.library));
            if let Some(issue) = library.issue() {
                eprintln!("warning: starting with empty lists — {issue}");
            }
            tui::run(source, playback, library).await
        }
        Some(Command::Top { limit }) => {
            print_listing(&format!("Top anime ({mode})"), &source.top(limit).await?);
            Ok(())
        }
        Some(Command::Season {
            year,
            season,
            limit,
        }) => {
            let (heading, anime) = match (year, season) {
                (Some(year), Some(season)) => {
                    let season = SeasonRef { year, season };
                    let anime = source.season(&season, limit).await?;
                    (season.label(), anime)
                }
                (None, None) => (
                    "Current season".to_string(),
                    source.current_season(limit).await?,
                ),
                _ => bail_season_arguments()?,
            };
            print_listing(&heading, &anime);
            Ok(())
        }
        Some(Command::Seasons) => {
            let seasons = source.seasons_index().await?;
            println!("Available seasons\n");
            for season in &seasons {
                println!("{:<6}{}", season.year, season.season);
            }
            Ok(())
        }
        Some(Command::Recommendations { limit }) => {
            print_listing("Recommendations", &source.recommendations(limit).await?);
            Ok(())
        }
        Some(Command::Latest { limit }) => {
            print_listing("Latest releases", &source.latest(limit).await?);
            Ok(())
        }
        Some(Command::Search { query, limit }) => {
            let anime = source.search(&query, limit).await?;
            if anime.is_empty() {
                println!("No anime found for \"{}\".", query.trim());
            } else {
                println!("{} results for \"{}\"\n", anime.len(), query.trim());
                print_rows(&anime);
            }
            Ok(())
        }
        Some(Command::Ongoing { limit }) => {
            print_listing("Ongoing", &source.ongoing(limit).await?);
            Ok(())
        }
        Some(Command::Play { query, episode }) => play(source, playback, &query, episode).await,
    }
}

/// Plays the best match for `query`. The match is the first search result, which
/// is reported so a wrong guess is obvious rather than silent.
async fn play(
    source: Source,
    mut playback: Playback,
    query: &str,
    episode: Option<u32>,
) -> Result<()> {
    let matches = source.search(query, 1).await?;
    let Some(row) = matches.first() else {
        bail!("No anime found for \"{}\".", query.trim());
    };

    // The detail decides whether an episode number applies at all, and gives the
    // title the player is labelled with.
    let detail = source.detail(&row.origin).await?;
    let (title, episode) = match &detail {
        AnimeDetail::Cached(anime) => match anime.kind {
            AnimeKind::Movie => (anime.title.clone(), None),
            AnimeKind::Series => {
                let number = episode.unwrap_or(1);
                if !anime.episodes.iter().any(|entry| entry.number == number) {
                    bail!(
                        "\"{}\" has no episode {number}. The catalog lists {}.",
                        anime.title,
                        episode_range(anime.episodes.iter().map(|entry| entry.number))
                    );
                }
                (anime.title.clone(), Some(number))
            }
        },
        AnimeDetail::Live(anime) => {
            if anime.is_movie() {
                (anime.display_title().to_string(), None)
            } else {
                let number = episode.unwrap_or(1);
                // The API knows a count, not a list; an unknown count is not
                // grounds to refuse a number.
                if let Some(count) = anime.episodes
                    && (number == 0 || number > count)
                {
                    bail!(
                        "\"{}\" has no episode {number}. It has {count}.",
                        anime.display_title()
                    );
                }
                (anime.display_title().to_string(), Some(number))
            }
        }
    };

    let request = playback.request(row.origin.clone(), title, episode);
    println!("Resolving {}…", request.label());
    let label = request.label();
    let stream = playback.play(request).await?;
    println!("Playing {label} via {stream} in {}", playback.player_name());
    println!("{}", stream.url);
    // The player is detached, so anything it rejects fails after this returns.
    println!("Player output: {}", playback.log_path().display());

    // This host's segments are repaired by a proxy running in this process, so
    // returning now would cut the stream off. Every other stream still returns
    // the moment the player is up.
    if let Some(port) = playback.proxy_port() {
        println!(
            "Proxying segments on 127.0.0.1:{port} — this command stays open until the \
             player exits."
        );
        playback.wait_for_players().await;
    }
    Ok(())
}

/// Resolution order: `--provider`, then `TERMUTO_PROVIDER`, then the chain's own
/// leading host. An empty value is no choice rather than an error.
fn resolve_provider(option: Option<String>) -> Option<String> {
    option
        .or_else(|| env::var(PROVIDER_ENV).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

pub fn resolve_catalog_path(option: Option<PathBuf>) -> PathBuf {
    option
        .or_else(|| env::var_os("TERMUTO_CATALOG").map(PathBuf::from))
        .unwrap_or_else(default_catalog_path)
}

/// `~/.termuto/catalog.json`, so the binary works from any directory. Falls back
/// to the working directory only when the home directory cannot be determined.
fn default_catalog_path() -> PathBuf {
    env::home_dir()
        .map(|home| home.join(".termuto").join("catalog.json"))
        .unwrap_or_else(|| PathBuf::from("catalog.json"))
}

/// The episode numbers a title holds, as `1–12` when they run consecutively and
/// as a plain list when they do not.
fn episode_range(numbers: impl Iterator<Item = u32>) -> String {
    let mut numbers: Vec<u32> = numbers.collect();
    numbers.sort_unstable();
    match numbers.as_slice() {
        [] => "no episodes".to_string(),
        [only] => format!("episode {only}"),
        [first, .., last] if (*last - *first) as usize + 1 == numbers.len() => {
            format!("episodes {first}–{last}")
        }
        _ => format!(
            "episodes {}",
            numbers
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn bail_season_arguments() -> Result<(String, Vec<AnimeSummary>)> {
    Err(anyhow!(
        "--year and --season go together. Pass both, or neither for the current season. \
         Run `termuto seasons` to see what is available."
    ))
}

fn print_listing(heading: &str, anime: &[AnimeSummary]) {
    println!("{heading}\n");
    if anime.is_empty() {
        println!("Nothing to show. Mode is set by --mode or {MODE_ENV}.");
        return;
    }
    print_rows(anime);
}

fn print_rows(anime: &[AnimeSummary]) {
    // Title-only listings, such as recommendations, have no columns to head.
    if !anime.iter().all(AnimeSummary::is_bare) {
        println!("{}", AnimeSummary::header());
    }
    for entry in anime {
        println!("{}", entry.row());
        if let Some(note) = &entry.note {
            println!("  ↳ {note}");
        }
    }
}
