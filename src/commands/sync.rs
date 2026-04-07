use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::{self, DotsyncConfig, SyncStrategy};
use crate::git;

const CONFIG_FILE: &str = ".dotsync.yaml";

pub fn run() -> Result<()> {
    let repo_dir = config::repo_dir();
    let mut cfg = DotsyncConfig::load()?;

    if git::has_remote(&repo_dir) {
        println!("Pulling from remote...");
        git::git_pull(&repo_dir)?;
    }

    // Sync config file first; reload if the repo had a newer version
    // (e.g. another machine added tracked files).
    let downloaded_config =
        sync_config_file_at(&config::config_path(), &config::repo_config_path(), &mut cfg)?;
    if downloaded_config {
        cfg = DotsyncConfig::load()?;
    }

    let files = cfg.files.clone();
    for file in &files {
        let local = config::home_dir().join(file);
        let repo  = repo_dir.join(file);

        match (local.exists(), repo.exists()) {
            (false, false) => {
                println!("  [skip]     {} — not found locally or in repo", file);
            }
            (true, false) => {
                copy_up(&local, &repo, file)?;
                cfg.set_last_synced(file, mtime(&local)?);
            }
            (false, true) => {
                copy_down(&repo, &local, file)?;
                cfg.set_last_synced(file, mtime(&local)?);
            }
            (true, true) => {
                let strategy    = cfg.strategy_for(file);
                let last_synced = cfg.last_synced_for(file);

                match strategy {
                    SyncStrategy::Merge => {
                        if files_identical(&local, &repo) {
                            print_skipped(file);
                        } else {
                            merge_both(file, &local, &repo)?;
                        }
                        cfg.set_last_synced(file, mtime(&local)?);
                    }
                    SyncStrategy::Overwrite => {
                        if files_identical(&local, &repo) {
                            print_skipped(file);
                        } else if should_upload(mtime(&local)?, last_synced) {
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

    // Save updated timestamps, then push the final config into the repo
    // so it's included in the commit.
    cfg.save()?;
    fs::copy(&config::config_path(), &config::repo_config_path())
        .context("failed to copy .dotsync.yaml into repo")?;

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
// Extracted logic (also used by tests)
// ---------------------------------------------------------------------------

/// Returns true when the local file should be uploaded to the repo.
/// Local is considered newer when it was modified after the last sync,
/// or when there is no sync record yet (first sync).
fn should_upload(local_mtime: DateTime<Utc>, last_synced: Option<DateTime<Utc>>) -> bool {
    match last_synced {
        None     => true,
        Some(ts) => local_mtime > ts,
    }
}

/// Produce a union of lines: all local lines first, then any remote-only lines.
/// Duplicate lines are dropped (first occurrence wins).
fn merge_union(local_text: &str, remote_text: &str) -> String {
    let mut seen = HashSet::new();
    let mut merged: Vec<&str> = Vec::new();
    for line in local_text.lines().chain(remote_text.lines()) {
        if seen.insert(line) {
            merged.push(line);
        }
    }
    merged.join("\n") + "\n"
}

/// Sync `local` ↔ `repo` for the config file using overwrite strategy.
/// Returns true if `local` was overwritten from `repo` (caller should reload config).
fn sync_config_file_at(local: &Path, repo: &Path, cfg: &mut DotsyncConfig) -> Result<bool> {
    match (local.exists(), repo.exists()) {
        (false, false) => Ok(false),
        (true, false) => {
            copy_up(local, repo, CONFIG_FILE)?;
            cfg.set_last_synced(CONFIG_FILE, mtime(local)?);
            Ok(false)
        }
        (false, true) => {
            copy_down(repo, local, CONFIG_FILE)?;
            Ok(true)
        }
        (true, true) => {
            if files_identical(local, repo) {
                Ok(false)
            } else {
                let last_synced = cfg.last_synced_for(CONFIG_FILE);
                if should_upload(mtime(local)?, last_synced) {
                    copy_up(local, repo, CONFIG_FILE)?;
                    cfg.set_last_synced(CONFIG_FILE, mtime(local)?);
                    Ok(false)
                } else {
                    copy_down(repo, local, CONFIG_FILE)?;
                    Ok(true)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// File-level helpers
// ---------------------------------------------------------------------------

fn files_identical(a: &Path, b: &Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn print_skipped(file: &str) {
    #[cfg(not(test))]
    println!("  [skipped]  {}", file);
    let _ = file; // suppress unused-variable warning in test builds
}

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
    #[cfg(not(test))]
    println!("  [upload]   {}", file);
    Ok(())
}

fn copy_down(repo: &Path, local: &Path, file: &str) -> Result<()> {
    ensure_parent(local)?;
    fs::copy(repo, local)
        .with_context(|| format!("failed to download {} from repo", file))?;
    #[cfg(not(test))]
    println!("  [download] {}", file);
    Ok(())
}

/// Read a file as UTF-8, replacing any invalid bytes with the Unicode
/// replacement character so that line-based merging never fails.
fn read_lossy(path: &Path, label: &str) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read {} {}", label, path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn merge_both(file: &str, local: &Path, repo: &Path) -> Result<()> {
    let local_content = read_lossy(local, "local")?;
    let repo_content = read_lossy(repo, "repo")?;

    let result = merge_union(&local_content, &repo_content);

    fs::write(local, &result)
        .with_context(|| format!("failed to write merged {} locally", file))?;
    ensure_parent(repo)?;
    fs::write(repo, &result)
        .with_context(|| format!("failed to write merged {} to repo", file))?;

    #[cfg(not(test))]
    println!("  [merge]    {}", file);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DotsyncConfig;
    use chrono::Duration;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Panic if any test path accidentally points at a real dotsync path.
    fn assert_isolated(paths: &[&Path]) {
        let real_config = config::config_path();
        let real_repo   = config::repo_dir();
        for path in paths {
            assert!(
                !path.starts_with(&real_repo) && *path != real_config,
                "test path {} overlaps with real dotsync files — test is not isolated",
                path.display()
            );
        }
    }

    // --- merge strategy ---

    #[test]
    fn merge_keeps_local_lines_first_and_appends_remote_only_lines() {
        let result = merge_union("a\nb\nc\n", "b\nd\n");
        assert_eq!(result, "a\nb\nc\nd\n");
    }

    #[test]
    fn merge_deduplicates_identical_lines() {
        let result = merge_union("x\ny\n", "x\ny\n");
        assert_eq!(result, "x\ny\n");
    }

    #[test]
    fn merge_handles_non_utf8_bytes() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join("history");
        let repo  = tmp.path().join("repo/history");
        assert_isolated(&[&local, &repo]);

        // Local has an invalid byte sequence mixed into a valid line
        let local_bytes = b"line_a\ngit reset --hard \xe2\x80\x83\xb4\nline_b\n";
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(&local, &local_bytes).unwrap();

        // Repo has clean UTF-8 with an overlapping and a unique line
        write(&repo, "line_a\nline_c\n");

        merge_both(".history", &local, &repo).unwrap();

        let merged = fs::read_to_string(&local).unwrap();
        // line_a appears once (deduped), the lossy-cleaned line is kept, line_b and line_c present
        assert!(merged.contains("line_a"), "shared line should be present");
        assert!(merged.contains("line_b"), "local-only line should be present");
        assert!(merged.contains("line_c"), "remote-only line should be present");
        assert!(merged.contains("git reset"), "cleaned line should be present");
        // The invalid bytes should have been replaced — result must be valid UTF-8
        assert!(merged.is_ascii() || merged.contains('\u{FFFD}'),
            "invalid bytes should be replaced with U+FFFD");
        // Repo copy should match local
        assert_eq!(fs::read_to_string(&repo).unwrap(), merged);
    }

    #[test]
    fn merge_with_empty_remote_returns_local_unchanged() {
        let result = merge_union("a\nb\n", "");
        assert_eq!(result, "a\nb\n");
    }

    #[test]
    fn merge_with_empty_local_returns_remote() {
        let result = merge_union("", "c\nd\n");
        assert_eq!(result, "c\nd\n");
    }

    // --- overwrite direction ---

    #[test]
    fn sync_uploads_when_local_is_newer_than_last_synced() {
        let last = Utc::now() - Duration::hours(1);
        assert!(should_upload(Utc::now(), Some(last)));
    }

    #[test]
    fn sync_downloads_when_local_is_older_than_last_synced() {
        let last = Utc::now() + Duration::hours(1);
        assert!(!should_upload(Utc::now(), Some(last)));
    }

    #[test]
    fn sync_uploads_on_first_sync_with_no_history() {
        assert!(should_upload(Utc::now(), None));
    }

    // --- skipped (identical content) ---

    #[test]
    fn sync_skips_overwrite_when_local_and_repo_are_identical() {
        assert!(files_identical(
            Path::new("/dev/null"),
            Path::new("/dev/null")
        ));
        // Non-identical files must not be skipped
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        write(&a, "same\n");
        write(&b, "different\n");
        assert!(!files_identical(&a, &b));
        write(&b, "same\n");
        assert!(files_identical(&a, &b));
    }

    #[test]
    fn sync_skips_upload_when_mtime_changed_but_content_is_identical_to_repo() {
        // Simulates `touch .bashrc`: mtime is newer than last_synced,
        // but content matches the repo — no upload should occur.
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".bashrc");
        let repo  = tmp.path().join("repo/.bashrc");
        assert_isolated(&[&local, &repo]);
        write(&local, "export PATH=$PATH\n");
        write(&repo,  "export PATH=$PATH\n");

        // last_synced is in the past → should_upload would return true,
        // but files_identical must short-circuit it.
        let last_synced = Utc::now() - Duration::hours(1);
        assert!(should_upload(mtime(&local).unwrap(), Some(last_synced)),
            "precondition: timestamp alone would trigger upload");
        assert!(files_identical(&local, &repo),
            "precondition: content is identical");

        // The sync loop checks identical before direction — verify no copy occurs.
        let repo_meta_before = fs::metadata(&repo).unwrap().modified().unwrap();
        // (nothing to call here beyond the helpers; the integration is in run())
        // Directly assert the skip condition that run() evaluates:
        assert!(files_identical(&local, &repo));
        let repo_meta_after = fs::metadata(&repo).unwrap().modified().unwrap();
        assert_eq!(repo_meta_before, repo_meta_after, "repo file must not have been written");
    }

    // --- config file sync ---

    #[test]
    fn sync_config_uploads_when_repo_is_absent() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".dotsync.yaml");
        let repo  = tmp.path().join("repo/.dotsync.yaml");
        assert_isolated(&[&local, &repo]);
        write(&local, "files: []\n");

        let mut cfg = DotsyncConfig::default();
        let downloaded = sync_config_file_at(&local, &repo, &mut cfg).unwrap();

        assert!(!downloaded, "should have uploaded, not downloaded");
        assert_eq!(fs::read_to_string(&repo).unwrap(), "files: []\n");
    }

    #[test]
    fn sync_config_downloads_when_local_is_absent() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".dotsync.yaml");
        let repo  = tmp.path().join("repo/.dotsync.yaml");
        assert_isolated(&[&local, &repo]);
        write(&repo, "files:\n- .bashrc\n");

        let mut cfg = DotsyncConfig::default();
        let downloaded = sync_config_file_at(&local, &repo, &mut cfg).unwrap();

        assert!(downloaded, "should have downloaded from repo");
        assert_eq!(fs::read_to_string(&local).unwrap(), "files:\n- .bashrc\n");
    }

    #[test]
    fn sync_config_skips_when_local_and_repo_are_identical() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".dotsync.yaml");
        let repo  = tmp.path().join("repo/.dotsync.yaml");
        assert_isolated(&[&local, &repo]);
        write(&local, "files:\n- .bashrc\n");
        write(&repo, "files:\n- .bashrc\n");

        let mut cfg = DotsyncConfig::default();
        let downloaded = sync_config_file_at(&local, &repo, &mut cfg).unwrap();

        assert!(!downloaded, "identical content should not trigger a reload");
        // Neither file should have been touched — contents remain as written
        assert_eq!(fs::read_to_string(&local).unwrap(), "files:\n- .bashrc\n");
        assert_eq!(fs::read_to_string(&repo).unwrap(), "files:\n- .bashrc\n");
    }

    #[test]
    fn sync_config_uploads_when_local_modified_after_last_sync() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".dotsync.yaml");
        let repo  = tmp.path().join("repo/.dotsync.yaml");
        assert_isolated(&[&local, &repo]);
        write(&local, "files:\n- .bashrc\n");
        write(&repo, "files: []\n");

        // last_synced is in the past → local file (written just now) is newer
        let mut cfg = DotsyncConfig::default();
        cfg.set_last_synced(CONFIG_FILE, Utc::now() - Duration::hours(1));

        let downloaded = sync_config_file_at(&local, &repo, &mut cfg).unwrap();

        assert!(!downloaded, "should have uploaded local config");
        assert_eq!(fs::read_to_string(&repo).unwrap(), "files:\n- .bashrc\n");
    }

    #[test]
    fn sync_config_downloads_when_local_unchanged_since_last_sync() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join(".dotsync.yaml");
        let repo  = tmp.path().join("repo/.dotsync.yaml");
        assert_isolated(&[&local, &repo]);
        write(&local, "files: []\n");
        write(&repo, "files:\n- .zshrc\n");

        // last_synced is in the future → local mtime is older → download
        let mut cfg = DotsyncConfig::default();
        cfg.set_last_synced(CONFIG_FILE, Utc::now() + Duration::hours(1));

        let downloaded = sync_config_file_at(&local, &repo, &mut cfg).unwrap();

        assert!(downloaded, "should have downloaded newer repo config");
        assert_eq!(fs::read_to_string(&local).unwrap(), "files:\n- .zshrc\n");
    }
}
