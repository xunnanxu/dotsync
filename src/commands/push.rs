use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::fs;
use std::path::Path;

use crate::config::{self, DotsyncConfig};
use crate::git;

pub fn run() -> Result<()> {
    let repo_dir = config::repo_dir();
    let mut cfg = DotsyncConfig::load()?;

    let files = cfg.files.clone();
    for file in &files {
        let local = config::home_dir().join(file);
        let repo  = repo_dir.join(file);

        if !local.exists() {
            println!("  [skip] {} — not found locally", file);
            continue;
        }

        if let Some(parent) = repo.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create parent dir for {}", file))?;
        }

        fs::copy(&local, &repo)
            .with_context(|| format!("failed to copy {} to repo", file))?;

        let ts = local_mtime(&local)?;
        cfg.set_last_synced(file, ts);
        println!("  [pushed] {}", file);
    }

    cfg.save()?;
    std::fs::copy(&config::config_path(), &config::repo_config_path())
        .context("failed to copy .dotsync.yaml into repo")?;

    let committed = git::git_commit_all(&repo_dir, "dotsync: push")?;
    if git::has_remote(&repo_dir) && committed {
        println!("Pushing to remote...");
        git::git_push(&repo_dir)?;
    }

    println!("Push complete.");
    Ok(())
}

fn local_mtime(path: &Path) -> Result<DateTime<Utc>> {
    let modified = path
        .metadata()
        .and_then(|m| m.modified())
        .with_context(|| format!("failed to read mtime of {}", path.display()))?;
    Ok(DateTime::from(modified))
}
