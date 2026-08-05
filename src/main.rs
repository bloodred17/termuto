use clap::Parser;
use termuto::cli::{self, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run(Cli::parse()).await
}
