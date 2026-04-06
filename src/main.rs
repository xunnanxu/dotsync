mod config;
mod git;
mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};

use commands::{config_cmd, init, pull, push, sync};

#[derive(Parser)]
#[command(name = "dotsync", about = "Dotfile syncing across hosts via git")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the tracking repository
    Init(init::InitArgs),
    /// Bidirectional sync: pull, apply per-file strategy, push
    Sync,
    /// Push all local files to the tracking repo (overwrite remote)
    Push,
    /// Restore files from a specific commit without altering repo history
    Pull(pull::PullArgs),
    /// Configure dotsync settings (e.g. auto-sync interval)
    Config(config_cmd::ConfigArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init(args)   => init::run(args),
        Commands::Sync         => sync::run(),
        Commands::Push         => push::run(),
        Commands::Pull(args)   => pull::run(args),
        Commands::Config(args) => config_cmd::run(args),
    }
}
