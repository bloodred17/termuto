use termuto_poc::cli::{self, Cli};
use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run(Cli::parse()).await
}
