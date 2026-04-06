use anyhow::{Context, Result};
use clap::Args;

use crate::config::{self, DotsyncConfig};
use crate::git;

#[derive(Args)]
pub struct InitArgs {
    /// URL of an existing git repository to use as the tracking remote
    #[arg(long)]
    pub repo: Option<String>,
}

pub fn run(args: InitArgs) -> Result<()> {
    let repo_dir = config::repo_dir();

    if repo_dir.exists() && repo_dir.join(".git").exists() {
        println!("Tracking repo already exists at {}", repo_dir.display());
    } else if let Some(url) = args.repo {
        println!("Cloning {} ...", url);
        git::git_clone(&url, &repo_dir)
            .with_context(|| format!("failed to clone {}", url))?;
        println!("Cloned into {}", repo_dir.display());
    } else {
        git::git_init(&repo_dir)?;
        println!("Initialized local repo at {}", repo_dir.display());
    }

    let config_path = config::config_path();
    if !config_path.exists() {
        DotsyncConfig::default().save()?;
        println!("Created config at {}", config_path.display());
    } else {
        println!("Config already exists at {}", config_path.display());
    }

    Ok(())
}
