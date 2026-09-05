//! The outbox: files the CLI owns, replicated by copying instead of by merging.
//!
//! `history.jsonl` and `tasks/` are live files of Claude Code. The engine may not put them in the
//! store and link them back — the CLI writes them under its own lock, on its own schedule, and a
//! link would turn every prompt into a git operation. So each machine writes its own copy into
//! `machines/<id>/`, everybody reads everybody's, and there is nothing to conflict over.
//!
//! Coming back in, the one file that has to be merged is `history.jsonl`: it is a single list the
//! CLI appends to. That is done under the CLI's own lock, in its own form — a directory named
//! `history.jsonl.lock`, considered stale after ten seconds.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use vibememory_core::desktop::roots::Roots;

/// The prompt history of the terminal CLI.
pub const HISTORY_FILE: &str = "history.jsonl";
/// The CLI's lock for it: a directory, not a file.
const HISTORY_LOCK: &str = "history.jsonl.lock";
/// How long the CLI waits before treating that lock as abandoned.
const LOCK_STALE_AFTER: Duration = Duration::from_secs(10);
/// The directory of background tasks.
pub const TASKS_DIR: &str = "tasks";
/// The key of a history line that names the project it belongs to.
const PROJECT_KEY: &str = "project";

/// What one round of outbox work moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Moved {
    /// History lines this machine published.
    pub history_out: usize,
    /// History lines taken from other machines and added here.
    pub history_in: usize,
    /// Task files published.
    pub tasks_out: usize,
    /// Task files brought in.
    pub tasks_in: usize,
}

/// Copies this machine's live files into its own outbox, translating paths so another machine can
/// read them.
///
/// # Errors
///
/// The text of what went wrong.
pub fn publish(
    config_dir: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
) -> Result<Moved, String> {
    let out = store.join("machines").join(machine_id);
    std::fs::create_dir_all(&out).map_err(|error| error.to_string())?;
    let mut moved = Moved::default();

    if let Some(lines) = read_lines(&config_dir.join(HISTORY_FILE))? {
        let portable: Vec<String> = lines
            .iter()
            .map(|line| rewrite_project(line, |path| roots.to_portable(path).ok()))
            .collect();
        moved.history_out = portable.len();
        write_lines(&out.join(HISTORY_FILE), &portable)?;
    }

    moved.tasks_out = copy_tree(&config_dir.join(TASKS_DIR), &out.join(TASKS_DIR))?;
    Ok(moved)
}

/// Brings what other machines published into this machine's live files.
///
/// `live_here` are the sessions running on this machine: their task files are being written right
/// now, and another machine's copy of them is older by construction.
///
/// # Errors
///
/// The text of what went wrong.
pub fn import(
    config_dir: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
    live_here: &BTreeSet<String>,
) -> Result<Moved, String> {
    let mut moved = Moved::default();
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Ok(moved);
    };

    let mut incoming_history: Vec<String> = Vec::new();
    for machine in machines.filter_map(Result::ok) {
        if machine.file_name().to_string_lossy() == machine_id {
            continue;
        }
        if let Some(lines) = read_lines(&machine.path().join(HISTORY_FILE))? {
            incoming_history.extend(
                lines
                    .iter()
                    .map(|line| rewrite_project(line, |path| roots.to_local(path).ok())),
            );
        }
        moved.tasks_in += import_tasks(
            &machine.path().join(TASKS_DIR),
            &config_dir.join(TASKS_DIR),
            live_here,
        )?;
    }

    if !incoming_history.is_empty() {
        moved.history_in = merge_history(&config_dir.join(HISTORY_FILE), &incoming_history)?;
    }
    Ok(moved)
}

/// Adds lines the local history does not have, under the CLI's own lock.
///
/// Order is preserved and nothing is removed: the history is what the up-arrow shows, and losing
/// somebody's line is worse than showing one twice — so lines already present are skipped by exact
/// bytes rather than by any cleverness about what "the same prompt" means.
fn merge_history(local: &Path, incoming: &[String]) -> Result<usize, String> {
    let _lock = HistoryLock::take(local)?;
    let existing = read_lines(local)?.unwrap_or_default();
    let known: BTreeSet<&str> = existing.iter().map(String::as_str).collect();
    let additions: Vec<String> = incoming
        .iter()
        .filter(|line| !known.contains(line.as_str()))
        .cloned()
        .collect();
    if additions.is_empty() {
        return Ok(0);
    }
    let mut all = existing;
    all.extend(additions.iter().cloned());
    write_lines(local, &all)?;
    Ok(additions.len())
}

/// The CLI's lock for `history.jsonl`: a directory whose existence is the claim.
struct HistoryLock {
    path: PathBuf,
}

impl HistoryLock {
    fn take(history: &Path) -> Result<Self, String> {
        let path = history.with_file_name(HISTORY_LOCK);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        for _ in 0..3 {
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !is_stale(&path) {
                        return Err("the CLI is writing the history right now".to_owned());
                    }
                    // Ten seconds is the CLI's own patience with this lock; past that it is a
                    // leftover of a process that died mid-write.
                    let _ = std::fs::remove_dir(&path);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("the history lock could not be taken".to_owned())
    }
}

impl Drop for HistoryLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.path);
    }
}

/// Whether a lock has been held longer than the CLI itself would wait.
fn is_stale(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return true;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age > LOCK_STALE_AFTER)
}

/// Rewrites the `project` field of one history line, leaving the line alone when it has none or
/// when the path cannot be translated. A line nobody can translate is still a line worth keeping.
fn rewrite_project(line: &str, translate: impl Fn(&str) -> Option<String>) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) else {
        return line.to_owned();
    };
    let Some(project) = value.get(PROJECT_KEY).and_then(serde_json::Value::as_str) else {
        return line.to_owned();
    };
    let Some(rewritten) = translate(project) else {
        return line.to_owned();
    };
    let Some(object) = value.as_object_mut() else {
        return line.to_owned();
    };
    object.insert(PROJECT_KEY.to_owned(), serde_json::Value::String(rewritten));
    serde_json::to_string(&value).unwrap_or_else(|_| line.to_owned())
}

/// Lines of a file, or `None` when it does not exist.
fn read_lines(path: &Path) -> Result<Option<Vec<String>>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes lines whole, through a temporary file: a reader must never catch half a history.
fn write_lines(path: &Path, lines: &[String]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let temporary = path.with_extension("jsonl.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

/// Copies a directory tree of small files, replacing what is there.
fn copy_tree(from: &Path, to: &Path) -> Result<usize, String> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(0);
    };
    let mut copied = 0;
    for entry in entries.filter_map(Result::ok) {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copied += copy_tree(&entry.path(), &target)?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// Brings in task files for sessions that are not running here.
///
/// A session live on this machine is writing its own task files; another machine's copy of them is
/// a snapshot from before, and writing it over the live one would undo work in progress.
fn import_tasks(from: &Path, to: &Path, live_here: &BTreeSet<String>) -> Result<usize, String> {
    let Ok(sessions) = std::fs::read_dir(from) else {
        return Ok(0);
    };
    let mut copied = 0;
    for session in sessions.filter_map(Result::ok) {
        let name = session.file_name().to_string_lossy().into_owned();
        if live_here.contains(&name) {
            continue;
        }
        copied += copy_tree(&session.path(), &to.join(&name))?;
    }
    Ok(copied)
}
