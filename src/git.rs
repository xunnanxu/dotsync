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

/// Pull in a disposable clone so a preview leaves the real repository untouched.
pub struct SyncPreviewRepo {
    _temp: tempfile::TempDir,
    path: PathBuf,
}

impl SyncPreviewRepo {
    pub fn prepare(repo: &Path) -> Result<Option<Self>> {
        // Match git_pull's no-op cases; the caller can read the repo directly.
        if !has_remote(repo) || !has_commits(repo) {
            return Ok(None);
        }
        if !run_git(repo, &["--no-optional-locks", "status", "--porcelain"])?.is_empty() {
            bail!("cannot preview remote sync with uncommitted files in {}; commit or stash them first", repo.display());
        }

        let branch = run_git(repo, &["symbolic-ref", "--short", "HEAD"])
            .context("cannot preview a pull with a detached HEAD")?;
        let mut remote = run_git(repo, &["remote", "get-url", "origin"])?;
        // Relative filesystem remotes are resolved from the real repo, not /tmp.
        let remote_path = repo.join(&remote);
        if remote_path.exists() {
            remote = remote_path.canonicalize()?.to_string_lossy().into_owned();
        }

        let temp = tempfile::tempdir().context("failed to create dry-run directory")?;
        let path = temp.path().join("repo");
        let output = Command::new("git")
            .args(["clone", "--no-hardlinks", "--quiet", "--"])
            .arg(repo)
            .arg(&path)
            .output()
            .context("failed to create dry-run clone")?;
        if !output.status.success() {
            bail!("failed to create dry-run clone: {}", String::from_utf8_lossy(&output.stderr));
        }
        run_git_quiet(&path, &["remote", "set-url", "origin", &remote])?;
        // clone's origin refs point at local commits. Replace them with the
        // source's remote-tracking refs so pull --rebase keeps those local commits.
        for reference in run_git(&path, &["for-each-ref", "--format=%(refname)", "refs/remotes/origin/"])?.lines() {
            run_git_quiet(&path, &["update-ref", "--no-deref", "-d", reference])?;
        }
        for reference in run_git(repo, &["for-each-ref", "--format=%(refname) %(objectname)", "refs/remotes/origin/"])?.lines() {
            if let Some((name, oid)) = reference.split_once(' ') {
                run_git_quiet(&path, &["update-ref", name, oid])?;
            }
        }
        // clone sets its own upstream; use the source branch's pull configuration.
        for suffix in ["remote", "merge"] {
            let key = format!("branch.{}.{}", branch, suffix);
            let _ = run_git(&path, &["config", "--unset-all", &key]);
            if let Ok(value) = run_git(repo, &["config", "--get", &key]) {
                run_git_quiet(&path, &["config", &key, &value])?;
            }
        }
        // A rebase of local commits may need identity configured only in this repo.
        for key in ["user.name", "user.email"] {
            if let Ok(value) = run_git(repo, &["config", "--get", key]) {
                run_git_quiet(&path, &["config", key, &value])?;
            }
        }
        git_pull(&path).context("dry-run pull failed; no sync directions can be reported")?;
        Ok(Some(Self { _temp: temp, path }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::time::SystemTime;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let remote = tmp.path().join("remote");
        let local = tmp.path().join("local");
        git_init(&remote).unwrap();
        run_git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]).unwrap();
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false")] {
            run_git(&remote, &["config", key, value]).unwrap();
        }
        fs::write(remote.join("settings"), "initial\n").unwrap();
        git_commit_all(&remote, "initial").unwrap();
        git_clone(remote.to_str().unwrap(), &local).unwrap();
        for (key, value) in [("user.name", "Test"), ("user.email", "test@example.com"), ("commit.gpgsign", "false")] {
            run_git(&local, &["config", key, value]).unwrap();
        }
        (tmp, local, remote)
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
        fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_owned(),
                        (fs::read(&path).unwrap(), fs::metadata(&path).unwrap().modified().unwrap()),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    #[test]
    fn preview_pulls_latest_remote_without_changing_either_repository() {
        let (_tmp, local, remote) = fixture();
        fs::write(remote.join("settings"), "latest\n").unwrap();
        git_commit_all(&remote, "remote update").unwrap();
        let local_before = snapshot(&local);
        let remote_before = snapshot(&remote);

        let preview = SyncPreviewRepo::prepare(&local).unwrap().unwrap();
        assert_eq!(fs::read_to_string(preview.path().join("settings")).unwrap(), "latest\n");
        let preview_path = preview.path().to_owned();
        drop(preview);

        assert!(!preview_path.exists(), "temporary clone must be cleaned up");
        assert_eq!(snapshot(&local), local_before);
        assert_eq!(snapshot(&remote), remote_before);
    }

    #[test]
    fn preview_rebases_local_commits_without_changing_the_original() {
        let (_tmp, local, remote) = fixture();
        fs::write(local.join("local-only"), "local commit\n").unwrap();
        git_commit_all(&local, "local update").unwrap();
        fs::write(remote.join("settings"), "latest\n").unwrap();
        git_commit_all(&remote, "remote update").unwrap();
        let before = snapshot(&local);

        let preview = SyncPreviewRepo::prepare(&local).unwrap().unwrap();
        assert_eq!(fs::read_to_string(preview.path().join("settings")).unwrap(), "latest\n");
        assert_eq!(fs::read_to_string(preview.path().join("local-only")).unwrap(), "local commit\n");
        assert_eq!(snapshot(&local), before);
    }

    #[test]
    fn preview_reports_rebase_conflicts_without_changing_the_original() {
        let (_tmp, local, remote) = fixture();
        fs::write(local.join("settings"), "local\n").unwrap();
        git_commit_all(&local, "local update").unwrap();
        fs::write(remote.join("settings"), "remote\n").unwrap();
        git_commit_all(&remote, "remote update").unwrap();
        let before = snapshot(&local);

        let result = SyncPreviewRepo::prepare(&local);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("dry-run pull failed"));
        assert_eq!(snapshot(&local), before);
    }

    #[test]
    fn preview_refuses_dirty_remote_repository_without_changing_it() {
        let (_tmp, local, _remote) = fixture();
        fs::write(local.join("settings"), "uncommitted\n").unwrap();
        let before = snapshot(&local);

        let result = SyncPreviewRepo::prepare(&local);
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("uncommitted files"));
        assert_eq!(snapshot(&local), before);
    }

    #[test]
    fn preview_resolves_relative_remote_paths() {
        let (_tmp, local, remote) = fixture();
        run_git(&local, &["remote", "set-url", "origin", "../remote"]).unwrap();
        fs::write(remote.join("settings"), "latest\n").unwrap();
        git_commit_all(&remote, "remote update").unwrap();

        let preview = SyncPreviewRepo::prepare(&local).unwrap().unwrap();
        assert_eq!(fs::read_to_string(preview.path().join("settings")).unwrap(), "latest\n");
    }

    #[test]
    fn preview_skips_pull_for_local_only_and_unborn_repositories() {
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path()).unwrap();
        assert!(SyncPreviewRepo::prepare(tmp.path()).unwrap().is_none());
        run_git(tmp.path(), &["remote", "add", "origin", "/nonexistent/dotsync-test-remote"]).unwrap();
        assert!(SyncPreviewRepo::prepare(tmp.path()).unwrap().is_none());
    }
}
