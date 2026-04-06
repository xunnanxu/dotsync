# Dotsync

Rust-based dotfile syncing.

## Supported Features

* Syncing dot files (`.bashrc`, `.profile`, `.tmux.conf` etc.) across hosts via git repos.
* Support for shell history merging (e.g. `.bash_history`)
* Support for automatic background sync (via user cron jobs)

## Supported Platforms

Linux, MacOS

## Installation

```shell
$ cargo install --path .
$ dotsync help
```

## Commands

* `init [--repo <repo url>]`
    - sets up the tracking repo in `~/.dotsync`; if `--repo` is not provided, creates a local-only git repo.
* `sync`
    - pulls from remote, applies per-file sync strategy, then commits and pushes.
    - commit message includes the timestamp: `dotsync: sync YYYY-MM-DD HH:MM:SS`.
* `pull --commit <commit>`
    - checks out the given commit into a temporary worktree and applies the tracked files to local disk.
    - `last_synced` timestamps are ignored; local files are overwritten unconditionally.
    - does not alter repo history.
* `push`
    - overwrites remote files with local files and updates all `last_synced` timestamps.
* `config add <file> [--strategy overwrite|merge] [-v]`
    - adds a file to the tracked list; accepts absolute paths (`~/.tmux.conf`), home-relative paths (`.tmux.conf`), or nested paths (`.config/nvim/init.vim`).
    - `--strategy merge` populates a metadata entry so the file uses union-merge instead of overwrite.
    - `-v` prints the full tracked file list after the operation.
* `config remove <file> [-v]`
    - removes a file from the tracked list and clears its metadata entry.
    - `-v` prints the full tracked file list after the operation.
* `config list`
    - shows the current repo path, remote URL, and branch, followed by all tracked files with their strategy and last-synced timestamp.
* `config --auto-sync-interval <interval>`
    - installs a user crontab entry that runs `sync` automatically on the given interval.
    - interval is semi-natural: `30m`, `1h`, `6h`, `1d`, `"30 minutes"`, `"2 hours"`, etc.
    - minute intervals must evenly divide 60 (e.g. 5, 10, 15, 20, 30).
    - hour intervals must evenly divide 24 (e.g. 1, 2, 3, 4, 6, 8, 12).
* `config --disable-auto-sync`
    - removes the auto-sync crontab entry.

## How It Works

It requires an existing/empty git repository to begin with. All changes will be synced there.
It tracks files to be synced via `~/.dotsync/.config.yaml` (which is yet another file to be dotsync-ed).

The file has a YAML structure of

```yaml
files:
    - .bashrc
    - .profile
    - .tmux.conf
    - .zsh_history

metadata:
    - file: .zsh_history
      strategy: merge
    - file: .tmux.conf
      last_synced: <ISO timestamp>
```

The default sync strategy if not specified is `overwrite`,
meaning if the last modified timestamp of the local file is older than the `last_synced` timestamp,
then the local file will simply be overwitten by the remote file.

If the file was never synced, then the local file will be uploaded to remote repo,
and the `last_synced` timestamp will be updated to the last modified time of the local file.

In the case where this is not sufficient like the history files,
one can update the strategy to `merge`, in which case dotsync would try to combine the changes.
The order could be arbitrary and dotsync would not try to maintain chronological order,
similar to git `merge=union`.

