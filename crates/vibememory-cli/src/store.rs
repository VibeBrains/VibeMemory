//! Reading the store directory: which project names already exist.
//!
//! The core needs this list to keep one store per identity: a session whose name differs from an
//! existing one only in case or in Unicode form must reuse the existing spelling instead of
//! creating a second directory that APFS will merge and git will not.
//!
//! The list comes from two sources on purpose. `readdir` sees what is on this disk — including
//! directories not yet committed — but on a case-insensitive file system it cannot show two
//! spellings that collide, because only one of them exists there. The git tree keeps both, and it
//! is the tree the other machine will fetch.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use vibememory_core::naming::StoreName;

/// The subdirectory of the store that holds the projects.
pub const PROJECTS_DIR: &str = "projects";
/// What the tree of committed project directories is asked for.
const LS_TREE_REF: &str = "HEAD:projects";

/// A name found in the store that the engine cannot use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The name as it is written in the store.
    pub name: String,
    /// Why it is not a legal store name.
    pub reason: String,
}

/// What the store holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Existing {
    /// Legal names, one per identity key, sorted; the spelling is the one already in the store.
    pub names: Vec<StoreName>,
    /// Names that are not legal store names. They are reported by `doctor` and otherwise ignored:
    /// a directory somebody created by hand may not stop a session from starting.
    pub rejected: Vec<Rejected>,
}

/// Collects the project names of the store: the committed tree plus what is on disk.
///
/// Failures of either source are not failures of the whole: a store that has no commit yet has no
/// tree, and a store directory that does not exist yet has nothing to read. Both mean "nothing
/// known from here", never "stop".
#[must_use]
pub fn existing(store: &Path, timeout: Duration) -> Existing {
    let mut seen: BTreeMap<String, StoreName> = BTreeMap::new();
    let mut rejected: Vec<Rejected> = Vec::new();

    for name in committed_names(store, timeout)
        .into_iter()
        .chain(names_on_disk(&store.join(PROJECTS_DIR)))
    {
        match StoreName::parse(&name) {
            Ok(parsed) => {
                // The first spelling of an identity wins, and the tree is read first: what the
                // other machines already fetched outranks what this disk happens to show.
                seen.entry(parsed.key()).or_insert(parsed);
            }
            Err(error) => {
                let reason = error.to_string();
                if !rejected.iter().any(|item| item.name == name) {
                    rejected.push(Rejected { name, reason });
                }
            }
        }
    }

    Existing {
        names: seen.into_values().collect(),
        rejected,
    }
}

/// Names in the committed tree. An empty answer covers every failure: no repository, no commit,
/// no `projects` directory in it, or git not answering in time.
fn committed_names(store: &Path, timeout: Duration) -> Vec<String> {
    let mut command = Command::new("git");
    command
        .args(["ls-tree", "-d", "--name-only", LS_TREE_REF])
        .current_dir(store)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in crate::git::variables_to_remove(std::env::vars().map(|(name, _)| name)) {
        command.env_remove(name);
    }

    let Ok(output) = crate::git::run_with_timeout(command, timeout) else {
        return Vec::new();
    };
    let Some(stdout) = output else {
        return Vec::new();
    };
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Directory names under `projects` on this disk, symlinks included: a store directory reached
/// through a link is still a store directory.
fn names_on_disk(projects: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(projects) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        // A store directory reached through a symlink is still a store directory; a stray plain
        // file under `projects` is not one and must not become a name.
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
        })
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect()
}
