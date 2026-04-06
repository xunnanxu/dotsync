use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::config::{self, DotsyncConfig, SyncStrategy};

const MARKER: &str = "# dotsync-auto-sync";

#[derive(Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: Option<ConfigSubcommand>,

    /// Enable automatic background sync on an interval (e.g. "30m", "1h", "6h", "1d")
    #[arg(long, conflicts_with = "disable_auto_sync")]
    pub auto_sync_interval: Option<String>,

    /// Remove the automatic background sync cron job
    #[arg(long)]
    pub disable_auto_sync: bool,
}

#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Add a file to the sync list
    Add(AddArgs),
    /// Remove a file from the sync list
    Remove(FileArgs),
    /// Show the current repo settings and tracked file config
    List,
}

#[derive(Args)]
pub struct AddArgs {
    /// Path to the file (e.g. ~/.tmux.conf or .tmux.conf)
    pub file: String,

    /// Sync strategy for this file (default: overwrite)
    #[arg(long, value_enum)]
    pub strategy: Option<SyncStrategy>,

    /// Print the full list of tracked files after the operation
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Args)]
pub struct FileArgs {
    /// Path to the file (e.g. ~/.tmux.conf or .tmux.conf)
    pub file: String,

    /// Print the full list of tracked files after the operation
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(args: ConfigArgs) -> Result<()> {
    match args.command {
        Some(ConfigSubcommand::Add(add_args)) => cmd_add(add_args),
        Some(ConfigSubcommand::Remove(file_args)) => cmd_remove(file_args),
        Some(ConfigSubcommand::List) => cmd_list(),
        None => {
            if args.disable_auto_sync {
                remove_crontab_entry()?;
                println!("Auto-sync disabled.");
            } else if let Some(interval) = args.auto_sync_interval {
                let cron_expr = parse_interval(&interval)?;
                let bin = dotsync_bin();
                let entry = format!("{} {} sync {}", cron_expr, bin, MARKER);
                install_crontab_entry(&entry)?;
                println!("Auto-sync enabled: every {} (cron: {})", interval, cron_expr);
            } else {
                bail!("provide a subcommand (add/remove) or a flag (--auto-sync-interval / --disable-auto-sync)");
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// add / remove
// ---------------------------------------------------------------------------

/// Strip the home directory prefix and return a path relative to `~`.
/// Accepts absolute paths, paths starting with `~`, or already-relative names.
fn to_relative(raw: &str) -> Result<String> {
    // Replace leading `~/` or bare `~` with an empty prefix so PathBuf works.
    let expanded = if raw == "~" {
        config::home_dir()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        config::home_dir().join(rest)
    } else {
        PathBuf::from(raw)
    };

    if expanded.is_absolute() {
        let home = config::home_dir();
        match expanded.strip_prefix(&home) {
            Ok(rel) => Ok(rel.to_string_lossy().into_owned()),
            Err(_) => bail!(
                "file '{}' is not under the home directory ({})",
                raw,
                home.display()
            ),
        }
    } else {
        Ok(expanded.to_string_lossy().into_owned())
    }
}

fn cmd_add(args: AddArgs) -> Result<()> {
    let file = to_relative(&args.file)?;
    let mut cfg = DotsyncConfig::load()?;

    if cfg.files.contains(&file) {
        println!("'{}' is already tracked.", file);
    } else {
        cfg.files.push(file.clone());
        println!("Added '{}'.", file);
    }

    if let Some(strategy) = args.strategy {
        let meta = cfg.get_or_insert_metadata(&file);
        meta.strategy = strategy.clone();
        println!("Strategy set to {:?}.", strategy);
    }

    cfg.save()?;

    if args.verbose {
        print_files(&cfg);
    }
    Ok(())
}

fn cmd_remove(args: FileArgs) -> Result<()> {
    let file = to_relative(&args.file)?;
    let mut cfg = DotsyncConfig::load()?;

    let before = cfg.files.len();
    cfg.files.retain(|f| f != &file);
    cfg.metadata.retain(|m| m.file != file);

    if cfg.files.len() == before {
        println!("'{}' was not in the tracked file list.", file);
    } else {
        cfg.save()?;
        println!("Removed '{}'.", file);
    }

    if args.verbose {
        print_files(&cfg);
    }
    Ok(())
}

fn cmd_list() -> Result<()> {
    let repo_dir = config::repo_dir();

    // Repo settings
    println!("Repo:");
    println!("  path:   {}", repo_dir.display());
    if repo_dir.exists() {
        let remote = crate::git::run_git(&repo_dir, &["remote", "get-url", "origin"])
            .unwrap_or_else(|_| "(none)".to_string());
        println!("  remote: {}", remote);

        let branch = crate::git::run_git(&repo_dir, &["rev-parse", "--abbrev-ref", "HEAD"])
            .unwrap_or_else(|_| "(no commits yet)".to_string());
        println!("  branch: {}", branch);
    } else {
        println!("  (not initialized — run `dotsync init`)");
    }

    // Config YAML
    println!();
    println!("Config: {}", config::config_path().display());
    let cfg = DotsyncConfig::load()?;
    print_files(&cfg);

    Ok(())
}

fn print_files(cfg: &DotsyncConfig) {
    if cfg.files.is_empty() {
        println!("Tracked files: (none)");
    } else {
        println!("Tracked files:");
        for f in &cfg.files {
            let strategy = cfg.strategy_for(f);
            let last_synced = cfg.last_synced_for(f)
                .map(|ts| ts.to_rfc3339())
                .unwrap_or_else(|| "never".to_string());
            println!("  {} (strategy: {:?}, last synced: {})", f, strategy, last_synced);
        }
    }
}

// ---------------------------------------------------------------------------
// Interval parsing
// ---------------------------------------------------------------------------

/// Normalize natural language → compact form (e.g. "30 minutes" → "30m").
fn normalize(s: &str) -> String {
    s.to_lowercase()
        .replace(' ', "")
        .replace("minutes", "m")
        .replace("minute", "m")
        .replace("hours", "h")
        .replace("hour", "h")
        .replace("days", "d")
        .replace("day", "d")
}

/// Parse an interval string into a 5-field cron expression.
///
/// Accepted formats after normalization:
///   `<N>m`  — every N minutes  (N must divide 60 and be ≤ 59)
///   `<N>h`  — every N hours    (N must divide 24)
///   `<N>d`  — every N days
fn parse_interval(raw: &str) -> Result<String> {
    let s = normalize(raw);

    if let Some(n_str) = s.strip_suffix('m') {
        let n: u32 = n_str
            .parse()
            .with_context(|| format!("invalid number in interval '{}'", raw))?;
        if n == 60 {
            return parse_interval("1h");
        }
        if n == 0 || n > 59 {
            bail!("minute interval must be between 1 and 59; got {}", n);
        }
        if 60 % n != 0 {
            bail!(
                "minute interval {} does not evenly divide 60; \
                 valid values: 1, 2, 3, 4, 5, 6, 10, 12, 15, 20, 30",
                n
            );
        }
        return Ok(format!("*/{} * * * *", n));
    }

    if let Some(n_str) = s.strip_suffix('h') {
        let n: u32 = n_str
            .parse()
            .with_context(|| format!("invalid number in interval '{}'", raw))?;
        if n == 0 {
            bail!("hour interval must be ≥ 1");
        }
        if 24 % n != 0 {
            bail!(
                "hour interval {} does not evenly divide 24; \
                 valid values: 1, 2, 3, 4, 6, 8, 12, 24",
                n
            );
        }
        let hour_field = if n == 1 {
            "*".to_string()
        } else {
            format!("*/{}", n)
        };
        return Ok(format!("0 {} * * *", hour_field));
    }

    if let Some(n_str) = s.strip_suffix('d') {
        let n: u32 = n_str
            .parse()
            .with_context(|| format!("invalid number in interval '{}'", raw))?;
        if n == 0 {
            bail!("day interval must be ≥ 1");
        }
        let dom_field = if n == 1 {
            "*".to_string()
        } else {
            format!("*/{}", n)
        };
        return Ok(format!("0 0 {} * *", dom_field));
    }

    bail!(
        "unrecognized interval '{}'; expected a value like 30m, 1h, 6h, 1d",
        raw
    );
}

// ---------------------------------------------------------------------------
// Crontab management
// ---------------------------------------------------------------------------

fn dotsync_bin() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "dotsync".to_string())
}

fn read_crontab() -> Result<String> {
    let out = Command::new("crontab")
        .arg("-l")
        .output()
        .context("failed to run crontab -l")?;

    // `crontab -l` exits non-zero with "no crontab for user" when empty — treat as blank
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("no crontab") {
            Ok(String::new())
        } else {
            bail!("crontab -l failed: {}", stderr.trim())
        }
    }
}

fn write_crontab(content: &str) -> Result<()> {
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to run crontab -")?;

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(content.as_bytes())
        .context("failed to write to crontab")?;

    let status = child.wait().context("crontab - did not finish")?;
    if !status.success() {
        bail!("crontab - exited with status {}", status);
    }
    Ok(())
}

fn install_crontab_entry(entry: &str) -> Result<()> {
    let existing = read_crontab()?;
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains(MARKER))
        .collect();
    lines.push(entry);
    let new_crontab = lines.join("\n") + "\n";
    write_crontab(&new_crontab)
}

fn remove_crontab_entry() -> Result<()> {
    let existing = read_crontab()?;
    let filtered: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains(MARKER))
        .collect();

    if filtered.len() == existing.lines().count() {
        println!("No auto-sync cron entry found.");
        return Ok(());
    }

    let new_crontab = if filtered.is_empty() {
        String::new()
    } else {
        filtered.join("\n") + "\n"
    };
    write_crontab(&new_crontab)
}
