//! Sessions a team's clone holds for this machine alone: those that were on the machine before the
//! project went to the team. The team gets the sessions started after that; the older ones stay
//! beside them, so `claude --resume` opens them, but they never reach the team's host and take no
//! room of the plan.
//!
//! The one record of them is a block of `.git/info/exclude` in the clone: git itself then leaves
//! them out of every listing the tick commits from, and the `Stop` hook, which hands a snapshot to
//! git directly, asks the same block.

use std::collections::BTreeSet;
use std::path::Path;

/// The first line of the engine's block in `.git/info/exclude`.
const BEGIN: &str = "# vibememory: sessions kept on this machine only — begin";

/// The last line of the block.
const END: &str = "# vibememory: sessions kept on this machine only — end";

fn exclude_file(clone: &Path) -> std::path::PathBuf {
    clone.join(".git").join("info").join("exclude")
}

/// The paths in the block, relative to the clone.
fn kept(text: &str) -> BTreeSet<String> {
    let mut inside = false;
    let mut paths = BTreeSet::new();
    for line in text.lines() {
        match line {
            BEGIN => inside = true,
            END => inside = false,
            // anchored to the clone's root: `/projects/<name>/<file>`
            pattern if inside => {
                if let Some(path) = pattern.strip_prefix('/') {
                    paths.insert(path.to_owned());
                }
            }
            _ => {}
        }
    }
    paths
}

/// Writes the block with exactly `paths`, leaving the person's lines as they were.
fn write_block(clone: &Path, text: &str, paths: &BTreeSet<String>) -> Result<(), String> {
    let mut out = String::new();
    let mut inside = false;
    for line in text.lines() {
        match line {
            BEGIN => inside = true,
            END => inside = false,
            _ if !inside => {
                out.push_str(line);
                out.push('\n');
            }
            _ => {}
        }
    }
    if !paths.is_empty() {
        out.push_str(BEGIN);
        out.push('\n');
        for path in paths {
            out.push('/');
            out.push_str(path);
            out.push('\n');
        }
        out.push_str(END);
        out.push('\n');
    }
    let file = exclude_file(clone);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&file, out).map_err(|error| error.to_string())
}

fn read(clone: &Path) -> Result<String, String> {
    match std::fs::read_to_string(exclude_file(clone)) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.to_string()),
    }
}

/// Gives a kept session to the team: its transcript and its directory of subagents and tool
/// results leave the block, and the next tick commits them. Returns what was released.
///
/// # Errors
///
/// When the exclude file cannot be read or written.
pub fn release(clone: &Path, session: &str) -> Result<Vec<String>, String> {
    let text = read(clone)?;
    let (released, kept): (BTreeSet<String>, BTreeSet<String>) =
        kept(&text).into_iter().partition(|path| {
            let file = path.rsplit('/').next().unwrap_or_default();
            file == format!("{session}.jsonl") || path.split('/').any(|part| part == session)
        });
    if !released.is_empty() {
        write_block(clone, &text, &kept)?;
    }
    Ok(released.into_iter().collect())
}

/// Keeps `paths` (relative to the clone) on this machine only. What else the file says — the
/// user's own patterns — stays as it was.
///
/// # Errors
///
/// When the exclude file cannot be read or written.
pub fn keep(clone: &Path, paths: &[String]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    let text = read(clone)?;
    let mut all = kept(&text);
    all.extend(paths.iter().cloned());
    write_block(clone, &text, &all)
}

/// Whether a path of the clone is kept on this machine only.
#[must_use]
pub fn is_local(clone: &Path, relative: &str) -> bool {
    std::fs::read_to_string(exclude_file(clone)).is_ok_and(|text| kept(&text).contains(relative))
}
