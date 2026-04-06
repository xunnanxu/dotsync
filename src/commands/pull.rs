use anyhow::{Context, Result};
use clap::Args;
use std::fs;

use crate::config::{self, DotsyncConfig};
use crate::git::WorktreeGuard;

#[derive(Args)]
pub struct PullArgs {
    /// The commit SHA (or ref) to restore files from
    #[arg(long, required = true)]
    pub commit: String,
}

pub fn run(args: PullArgs) -> Result<()> {
    let repo_dir = config::repo_dir();
    let cfg = DotsyncConfig::load()?;

    println!("Checking out commit {}...", args.commit);

    // Guard ensures `git worktree remove` runs even on error.
    let wt = WorktreeGuard::add(&repo_dir, &args.commit)?;

    for file in &cfg.files {
        let src  = wt.path().join(file);
        let dest = config::home_dir().join(file);

        if !src.exists() {
            println!("  [skip]     {} — not present in commit {}", file, &args.commit[..8.min(args.commit.len())]);
            continue;
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dir for {}", file))?;
        }

        fs::copy(&src, &dest)
            .with_context(|| format!("failed to restore {} from commit", file))?;

        println!("  [restored] {}", file);
    }

    // WorktreeGuard drops here, removing the worktree.
    drop(wt);

    println!("Pull complete.");
    Ok(())
}
