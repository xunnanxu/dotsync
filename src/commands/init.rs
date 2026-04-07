use anyhow::{Context, Result};
use clap::Args;
use std::path::Path;

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
    let repo_config = config::repo_config_path();
    resolve_config(&config_path, &repo_config)?;

    Ok(())
}

/// Decide how to initialise the local config file.
///
/// Priority:
/// 1. Repo already has a config → copy it to local (always).
/// 2. No repo config, no local config → create a default.
/// 3. No repo config, local exists → keep it.
fn resolve_config(config_path: &Path, repo_config: &Path) -> Result<()> {
    if repo_config.exists() {
        std::fs::copy(repo_config, config_path)
            .context("failed to copy .dotsync.yaml from repo")?;
        println!("Copied config from repo to {}", config_path.display());
    } else if !config_path.exists() {
        let content =
            serde_yaml::to_string(&DotsyncConfig::default()).context("failed to serialize default config")?;
        std::fs::write(config_path, &content)
            .with_context(|| format!("failed to write {}", config_path.display()))?;
        println!("Created config at {}", config_path.display());
    } else {
        println!("Config already exists at {}", config_path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn resolve_copies_repo_config_when_local_absent() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join("local/.dotsync.yaml");
        let repo = tmp.path().join("repo/.dotsync.yaml");
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        write(&repo, "files:\n- .bashrc\n");

        resolve_config(&local, &repo).unwrap();

        assert_eq!(fs::read_to_string(&local).unwrap(), "files:\n- .bashrc\n");
    }

    #[test]
    fn resolve_overwrites_local_config_from_repo() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join("local/.dotsync.yaml");
        let repo = tmp.path().join("repo/.dotsync.yaml");
        write(&local, "files: []\n");
        write(&repo, "files:\n- .zshrc\n");

        resolve_config(&local, &repo).unwrap();

        assert_eq!(
            fs::read_to_string(&local).unwrap(),
            "files:\n- .zshrc\n",
            "local config should be overwritten with repo version"
        );
    }

    #[test]
    fn resolve_creates_default_when_neither_exists() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join("local/.dotsync.yaml");
        let repo = tmp.path().join("repo/.dotsync.yaml");
        fs::create_dir_all(local.parent().unwrap()).unwrap();

        resolve_config(&local, &repo).unwrap();

        assert!(local.exists(), "default config should be created");
        let cfg: DotsyncConfig =
            serde_yaml::from_str(&fs::read_to_string(&local).unwrap()).unwrap();
        assert!(cfg.files.is_empty());
        assert!(cfg.metadata.is_empty());
    }

    #[test]
    fn resolve_keeps_local_when_repo_has_no_config() {
        let tmp = TempDir::new().unwrap();
        let local = tmp.path().join("local/.dotsync.yaml");
        let repo = tmp.path().join("repo/.dotsync.yaml");
        write(&local, "files:\n- .vimrc\n");

        resolve_config(&local, &repo).unwrap();

        assert_eq!(
            fs::read_to_string(&local).unwrap(),
            "files:\n- .vimrc\n",
            "existing local config should be preserved"
        );
    }
}
