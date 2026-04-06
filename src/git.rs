use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Run a git command in `repo`, returning trimmed stdout on success.
pub fn run_git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("failed to invoke git")?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }
}

/// Like `run_git` but discards stdout (errors still propagate via stderr).
fn run_git_quiet(repo: &Path, args: &[&str]) -> Result<()> {
    run_git(repo, args).map(|_| ())
}

// ---------------------------------------------------------------------------
// High-level operations
// ---------------------------------------------------------------------------

pub fn git_init(repo: &Path) -> Result<()> {
    std::fs::create_dir_all(repo)
        .with_context(|| format!("failed to create {}", repo.display()))?;
    run_git_quiet(repo, &["init"])?;
    Ok(())
}

pub fn git_clone(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("git")
        .args(["clone", url])
        .arg(dest)
        .output()
        .context("failed to invoke git clone")?;
    if !out.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Returns true when `origin` remote is configured.
pub fn has_remote(repo: &Path) -> bool {
    run_git(repo, &["remote", "get-url", "origin"]).is_ok()
}

/// Returns true when the repo has at least one commit.
fn has_commits(repo: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Pull from origin (rebase). No-ops when there is no remote or no commits yet.
pub fn git_pull(repo: &Path) -> Result<()> {
    if !has_remote(repo) || !has_commits(repo) {
        return Ok(());
    }
    run_git_quiet(repo, &["pull", "--rebase", "origin"])
        .context("git pull --rebase failed")?;
    Ok(())
}

/// Stage all changes and commit. Returns false if the working tree was clean.
pub fn git_commit_all(repo: &Path, message: &str) -> Result<bool> {
    run_git_quiet(repo, &["add", "-A"])?;
    let status = run_git(repo, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok(false);
    }
    run_git_quiet(repo, &["commit", "-m", message])?;
    Ok(true)
}

/// Push to origin. Sets upstream on the first push.
pub fn git_push(repo: &Path) -> Result<()> {
    let has_upstream = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if has_upstream {
        run_git_quiet(repo, &["push"]).context("git push failed")
    } else {
        let branch = run_git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_else(|_| "main".to_string());
        run_git_quiet(repo, &["push", "-u", "origin", &branch])
            .context("git push failed")
    }
}

// ---------------------------------------------------------------------------
// WorktreeGuard — RAII wrapper for `git worktree add --detach`
// ---------------------------------------------------------------------------

pub struct WorktreeGuard {
    repo: PathBuf,
    path: PathBuf,
}

impl WorktreeGuard {
    /// Adds a detached worktree for `commit` at a unique temp path.
    pub fn add(repo: &Path, commit: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "dotsync-wt-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));

        run_git_quiet(
            repo,
            &["worktree", "add", "--detach", path.to_str().unwrap(), commit],
        )
        .with_context(|| format!("git worktree add for commit {}", commit))?;

        Ok(WorktreeGuard {
            repo: repo.to_path_buf(),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        // Best-effort: git worktree remove also deletes the directory.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
