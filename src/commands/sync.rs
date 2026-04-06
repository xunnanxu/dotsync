use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::{self, DotsyncConfig, SyncStrategy};
use crate::git;

pub fn run() -> Result<()> {
    let repo_dir = config::repo_dir();
    let mut cfg = DotsyncConfig::load()?;

    // Pull remote changes first
    if git::has_remote(&repo_dir) {
        println!("Pulling from remote...");
        git::git_pull(&repo_dir)?;
    }

    let files = cfg.files.clone();
    for file in &files {
        let local = config::home_dir().join(file);
        let repo  = repo_dir.join(file);

        let local_exists = local.exists();
        let repo_exists  = repo.exists();

        match (local_exists, repo_exists) {
            (false, false) => {
                println!("  [skip]     {} — not found locally or in repo", file);
            }
            (true, false) => {
                // Repo doesn't have it yet → always upload
                copy_up(&local, &repo, file)?;
                cfg.set_last_synced(file, mtime(&local)?);
            }
            (false, true) => {
                // Local missing → download (e.g. new machine)
                copy_down(&repo, &local, file)?;
                cfg.set_last_synced(file, mtime(&local)?);
            }
            (true, true) => {
                let strategy   = cfg.strategy_for(file);
                let last_synced = cfg.last_synced_for(file);

                match strategy {
                    SyncStrategy::Merge => {
                        merge_both(file, &local, &repo)?;
                        cfg.set_last_synced(file, mtime(&local)?);
                    }
                    SyncStrategy::Overwrite => {
                        let local_newer = match last_synced {
                            None => true, // never synced → treat local as source of truth
                            Some(ts) => mtime(&local)? > ts,
                        };
                        if local_newer {
                            copy_up(&local, &repo, file)?;
                        } else {
                            copy_down(&repo, &local, file)?;
                        }
                        cfg.set_last_synced(file, mtime(&local)?);
                    }
                }
            }
        }
    }

    cfg.save()?;

    let committed = git::git_commit_all(
        &repo_dir,
        &format!("dotsync: sync {}", Utc::now().format("%Y-%m-%d %H:%M:%S")),
    )?;

    if git::has_remote(&repo_dir) && committed {
        println!("Pushing to remote...");
        git::git_push(&repo_dir)?;
    }

    println!("Sync complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn mtime(path: &Path) -> Result<DateTime<Utc>> {
    let modified = path
        .metadata()
        .and_then(|m| m.modified())
        .with_context(|| format!("failed to read mtime of {}", path.display()))?;
    Ok(DateTime::from(modified))
}

fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent dir for {}", path.display()))?;
    }
    Ok(())
}

fn copy_up(local: &Path, repo: &Path, file: &str) -> Result<()> {
    ensure_parent(repo)?;
    fs::copy(local, repo)
        .with_context(|| format!("failed to upload {} to repo", file))?;
    println!("  [upload]   {}", file);
    Ok(())
}

fn copy_down(repo: &Path, local: &Path, file: &str) -> Result<()> {
    ensure_parent(local)?;
    fs::copy(repo, local)
        .with_context(|| format!("failed to download {} from repo", file))?;
    println!("  [download] {}", file);
    Ok(())
}

/// Union merge: local lines first, then any remote-only lines appended.
/// Result written to both local and repo.
fn merge_both(file: &str, local: &Path, repo: &Path) -> Result<()> {
    let local_content = fs::read_to_string(local)
        .with_context(|| format!("failed to read local {}", file))?;
    let repo_content = fs::read_to_string(repo)
        .with_context(|| format!("failed to read repo {}", file))?;

    let mut seen = HashSet::new();
    let mut merged: Vec<&str> = Vec::new();

    for line in local_content.lines().chain(repo_content.lines()) {
        if seen.insert(line) {
            merged.push(line);
        }
    }

    let result = merged.join("\n") + "\n";
    fs::write(local, &result)
        .with_context(|| format!("failed to write merged {} locally", file))?;
    ensure_parent(repo)?;
    fs::write(repo, &result)
        .with_context(|| format!("failed to write merged {} to repo", file))?;

    println!("  [merge]    {}", file);
    Ok(())
}
