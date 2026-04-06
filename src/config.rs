use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME environment variable not set"))
}

/// The git tracking repo: ~/.dotsync/
pub fn repo_dir() -> PathBuf {
    home_dir().join(".dotsync")
}

/// The live config file: ~/.dotsync.yaml
pub fn config_path() -> PathBuf {
    home_dir().join(".dotsync.yaml")
}

/// The config file's copy inside the repo: ~/.dotsync/.dotsync.yaml
pub fn repo_config_path() -> PathBuf {
    repo_dir().join(".dotsync.yaml")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SyncStrategy {
    Overwrite,
    Merge,
}

impl Default for SyncStrategy {
    fn default() -> Self {
        SyncStrategy::Overwrite
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub file: String,
    #[serde(default, skip_serializing_if = "is_overwrite")]
    pub strategy: SyncStrategy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced: Option<DateTime<Utc>>,
}

fn is_overwrite(s: &SyncStrategy) -> bool {
    *s == SyncStrategy::Overwrite
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct DotsyncConfig {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub metadata: Vec<FileMetadata>,
}

impl DotsyncConfig {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        serde_yaml::from_str(&content).context("failed to parse config YAML")
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        let content = serde_yaml::to_string(self).context("failed to serialize config")?;
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn strategy_for(&self, file: &str) -> SyncStrategy {
        self.metadata
            .iter()
            .find(|m| m.file == file)
            .map(|m| m.strategy.clone())
            .unwrap_or_default()
    }

    pub fn last_synced_for(&self, file: &str) -> Option<DateTime<Utc>> {
        self.metadata
            .iter()
            .find(|m| m.file == file)
            .and_then(|m| m.last_synced)
    }

    pub fn get_or_insert_metadata(&mut self, file: &str) -> &mut FileMetadata {
        if let Some(pos) = self.metadata.iter().position(|m| m.file == file) {
            return &mut self.metadata[pos];
        }
        self.metadata.push(FileMetadata {
            file: file.to_string(),
            strategy: SyncStrategy::Overwrite,
            last_synced: None,
        });
        self.metadata.last_mut().unwrap()
    }

    pub fn set_last_synced(&mut self, file: &str, ts: DateTime<Utc>) {
        if let Some(m) = self.metadata.iter_mut().find(|m| m.file == file) {
            m.last_synced = Some(ts);
        } else {
            self.metadata.push(FileMetadata {
                file: file.to_string(),
                strategy: SyncStrategy::Overwrite,
                last_synced: Some(ts),
            });
        }
    }
}
