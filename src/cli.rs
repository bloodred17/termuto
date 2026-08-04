use crate::mode::{MODE_ENV, Mode, resolve_mode};
use crate::source::{AnimeSummary, SeasonRef, Source};
use crate::tui;
use anyhow::{Result, anyhow};
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
}

pub async fn run(cli: Cli) -> Result<()> {
    let mode = resolve_mode(cli.mode).map_err(|message| anyhow!(message))?;
    let source = Source::open(mode, resolve_catalog_path(cli.catalog)).await?;
    if let Some(issue) = source.catalog_issue() {
        eprintln!("warning: continuing without the local catalog — {issue}");
    }

    match cli.command {
        None | Some(Command::Tui) => tui::run(source).await,
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
    }
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
