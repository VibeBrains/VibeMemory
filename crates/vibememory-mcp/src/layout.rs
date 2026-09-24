//! Where things live on the store's host.
//!
//! One place, because two callers get no flags at all: the forced command of a key line runs
//! `shell <key>` and nothing more, and a repository's `pre-receive` hook runs `pre-receive`. The
//! commands an operator starts take `--access` and `--teams` over these, for a copy of the layout
//! somewhere else.

use std::path::{Path, PathBuf};

use crate::access::{is_day, is_name};

/// The server's binary: the forced command of every key line runs it.
pub const BINARY: &str = "/srv/vibememory/bin/vibememory-mcp";
/// The settings of a bare store repository, shared with `hostBootstrap.sh`.
pub const STORE_INIT: &str = "/srv/vibememory/bin/storeInit.sh";
/// The access snapshot the cabinet publishes.
pub const ACCESS_FILE: &str = "/srv/vibememory/access/access.json";
/// The team repositories, `<slug>.git` each.
pub const TEAMS_DIR: &str = "/srv/vibememory/teams";
/// What the last application of a snapshot did, next to the snapshot.
const APPLIED_NAME: &str = "applied.json";
/// The host's report for the cabinet, next to the snapshot.
const REPORT_NAME: &str = "host.json";
/// What the application and the hourly report hold while one of them writes `host.json`.
const REPORT_LOCK_NAME: &str = ".host.json.lock";
/// What an application of the snapshot holds from start to end: the run on a change of the file
/// and the timer's catch-up are separate units and must not apply side by side.
const APPLY_LOCK_NAME: &str = ".access-apply.lock";
/// The bytes of the snapshot the host applied last, next to the snapshot: what is in force when a
/// stale `access.json` lands over a newer one.
const APPLIED_SNAPSHOT_NAME: &str = "applied-snapshot.json";
/// When the nightly backup last finished, next to the snapshot.
const BACKUP_NAME: &str = "backup.json";
/// The keys of `vmgit`, which only the application of a snapshot writes.
pub const AUTHORIZED_KEYS: &str = "/home/vmgit/.ssh/authorized_keys";
/// Public keys of the host itself: `ssh_host_<type>_key.pub`.
pub const HOST_KEYS_DIR: &str = "/etc/ssh";
/// The suffix of a store's directory under `teams/`.
const STORE_SUFFIX: &str = ".git";
/// What a deleted team's directory is renamed with: `<slug>.deleted-<day>.git`.
const DELETED_INFIX: &str = ".deleted-";
/// Free space a push may not take from the partition every team shares.
pub const DEFAULT_RESERVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// The services whose state the report carries, as `systemctl is-active` names them.
pub const SERVICES: &[&str] = &["vibememory-mcp", "caddy", "postgresql"];

/// `applied.json` beside the snapshot at `access`.
#[must_use]
pub fn applied_file(access: &Path) -> PathBuf {
    access.with_file_name(APPLIED_NAME)
}

/// `applied-snapshot.json` beside the snapshot at `access`.
#[must_use]
pub fn applied_snapshot_file(access: &Path) -> PathBuf {
    access.with_file_name(APPLIED_SNAPSHOT_NAME)
}

/// The lock the writers of `host.json` take in turn, beside the snapshot at `access`.
#[must_use]
pub fn report_lock_file(access: &Path) -> PathBuf {
    access.with_file_name(REPORT_LOCK_NAME)
}

/// The lock an application of the snapshot holds, beside the snapshot at `access`.
#[must_use]
pub fn apply_lock_file(access: &Path) -> PathBuf {
    access.with_file_name(APPLY_LOCK_NAME)
}

/// `host.json` beside the snapshot at `access`.
#[must_use]
pub fn report_file(access: &Path) -> PathBuf {
    access.with_file_name(REPORT_NAME)
}

/// `backup.json` beside the snapshot at `access`.
#[must_use]
pub fn backup_file(access: &Path) -> PathBuf {
    access.with_file_name(BACKUP_NAME)
}

/// What a directory under `teams/` is, by its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamDir {
    /// `<slug>.git`.
    Store(String),
    /// `<slug>.deleted-<day>.git`.
    Deleted {
        /// The slug.
        slug: String,
        /// The day of the deletion.
        date: String,
    },
}

/// Reads a directory name under `teams/`; `None` for anything that is not a store's name.
#[must_use]
pub fn team_dir(name: &str) -> Option<TeamDir> {
    let stem = name.strip_suffix(STORE_SUFFIX)?;
    if let Some((slug, date)) = stem.split_once(DELETED_INFIX) {
        return (is_name(slug) && is_day(date)).then(|| TeamDir::Deleted {
            slug: slug.to_owned(),
            date: date.to_owned(),
        });
    }
    is_name(stem).then(|| TeamDir::Store(stem.to_owned()))
}

/// The directory of a live team: `<slug>.git`.
#[must_use]
pub fn store_dir(slug: &str) -> String {
    format!("{slug}{STORE_SUFFIX}")
}

/// The directory a deleted team is renamed to: `<slug>.deleted-<day>.git`.
#[must_use]
pub fn retired_dir(slug: &str, date: &str) -> String {
    format!("{slug}{DELETED_INFIX}{date}{STORE_SUFFIX}")
}
