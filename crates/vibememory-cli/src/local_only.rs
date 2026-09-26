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
    let file = exclude_file(clone);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let mut all = kept(&text);
    all.extend(paths.iter().cloned());
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
    out.push_str(BEGIN);
    out.push('\n');
    for path in &all {
        out.push('/');
        out.push_str(path);
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(&file, out).map_err(|error| error.to_string())
}

/// Whether a path of the clone is kept on this machine only.
#[must_use]
pub fn is_local(clone: &Path, relative: &str) -> bool {
    std::fs::read_to_string(exclude_file(clone)).is_ok_and(|text| kept(&text).contains(relative))
}
