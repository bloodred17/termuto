use crate::catalog::{Anime, CatalogRepository};
use crate::tui;
use anyhow::Result;
use clap::{Parser, Subcommand};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "termuto",
    about = "Browse a local anime catalog from the terminal",
    version
)]
pub struct Cli {
    /// Path to the Deeb JSON catalog (defaults to TERMUTO_CATALOG or ./catalog.json)
    #[arg(long, global = true, value_name = "PATH")]
    pub catalog: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch the interactive terminal UI
    Tui,
    /// List titles by newest release first
    Latest {
        /// Maximum number of titles to show
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search titles and alternative titles (case-insensitive)
    Search { query: String },
    /// List ongoing titles by newest release first
    Ongoing,
}

pub async fn run(cli: Cli) -> Result<()> {
    let catalog_path = resolve_catalog_path(cli.catalog);
    let repository = CatalogRepository::open(catalog_path).await?;

    match cli.command {
        None | Some(Command::Tui) => tui::run(repository).await,
        Some(Command::Latest { limit }) => {
            let anime = repository.latest(limit).await?;
            print_listing("Latest releases", &anime, true);
            Ok(())
        }
        Some(Command::Search { query }) => {
            let anime = repository.search(&query).await?;
            if anime.is_empty() {
                println!("No anime found for \"{}\".", query.trim());
            } else {
                println!("{} results for \"{}\"\n", anime.len(), query.trim());
                print_rows(&anime, false);
            }
            Ok(())
        }
        Some(Command::Ongoing) => {
            let anime = repository.ongoing().await?;
            print_listing("Ongoing", &anime, true);
            Ok(())
        }
    }
}

pub fn resolve_catalog_path(option: Option<PathBuf>) -> PathBuf {
    option
        .or_else(|| env::var_os("TERMUTO_CATALOG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("catalog.json"))
}

fn print_listing(heading: &str, anime: &[Anime], show_release: bool) {
    println!("{heading}\n");
    print_rows(anime, show_release);
}

fn print_rows(anime: &[Anime], show_release: bool) {
    if show_release {
        println!(
            "{:<38}  {:<10}  {:<11}  RELEASED",
            "TITLE", "TYPE", "STATUS"
        );
        for entry in anime {
            println!(
                "{:<38}  {:<10}  {:<11}  {}",
                entry.title,
                entry.kind,
                entry.status,
                release_date(entry)
            );
        }
    } else {
        println!("{:<38}  {:<10}  STATUS", "TITLE", "TYPE");
        for entry in anime {
            println!("{:<38}  {:<10}  {}", entry.title, entry.kind, entry.status);
        }
    }
}

pub fn release_date(anime: &Anime) -> String {
    anime
        .latest_release_at
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "—".to_string())
}
