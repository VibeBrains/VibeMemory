//! Two operations that may only happen when nobody is using the thing they touch: re-aiming a
//! link, and importing a real directory the CLI created before the engine got there.
//!
//! Both were deliberately left out of the hook. A hook sees one machine; these need to know that
//! no session anywhere is writing into the directory, and that answer lives in the store —
//! `machines/*/live.json`, written by every machine's own hooks. A pid on this machine proves
//! nothing about the laptop in the other room.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::hook::stop::Live;

/// Why an operation was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A session somewhere is working in this directory right now.
    SessionLive {
        /// Which machine reports it.
        machine: String,
        /// Which session.
        session: String,
    },
    /// There is nothing at the path to act on.
    NothingThere,
    /// The path holds something this operation does not handle.
    NotApplicable {
        /// What was found.
        found: String,
    },
}

impl Refusal {
    /// A sentence for the person who asked.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::SessionLive { machine, session } => format!(
                "session {session} is live on {machine} and is writing into this directory. \
                 Nothing was changed — finish that session, or wait for its machine to stop \
                 reporting it, and run this again."
            ),
            Self::NothingThere => "there is nothing at that path to work on".to_owned(),
            Self::NotApplicable { found } => format!("that path holds {found}"),
        }
    }
}

/// Every session any machine says it is running, with the machine that says so.
#[must_use]
pub fn live_everywhere(store: &Path) -> Vec<(String, String, String)> {
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Vec::new();
    };
    let mut live = Vec::new();
    for machine in machines.filter_map(Result::ok) {
        let name = machine.file_name().to_string_lossy().into_owned();
        let Ok(text) = std::fs::read_to_string(machine.path().join("live.json")) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Live>(&text) else {
            continue;
        };
        for (session, entry) in parsed.sessions {
            live.push((name.clone(), session, entry.cwd));
        }
    }
    live
}

/// The first live session working in `cwd`, on any machine.
fn live_in(store: &Path, cwd: &str) -> Option<(String, String)> {
    live_everywhere(store)
        .into_iter()
        .find(|(_, _, live_cwd)| live_cwd == cwd)
        .map(|(machine, session, _)| (machine, session))
}

/// Points an existing link at a different store directory.
///
/// The hook never does this: under a live session, replacing a link costs the session, and from a
/// hook there is no way to know whether one is running elsewhere. Here there is.
///
/// # Errors
///
/// A [`Refusal`] when the link may not be touched, or the text of a file-system failure.
pub fn relink(
    config_dir: &Path,
    store: &Path,
    enc: &str,
    name: &str,
    portable_cwd: &str,
) -> Result<PathBuf, Refusal> {
    let link = config_dir.join("projects").join(enc);
    let metadata = std::fs::symlink_metadata(&link).map_err(|_| Refusal::NothingThere)?;
    if !metadata.is_symlink() {
        return Err(Refusal::NotApplicable {
            found: "a real directory, not a link — import it first".to_owned(),
        });
    }
    if let Some((machine, session)) = live_in(store, portable_cwd) {
        return Err(Refusal::SessionLive { machine, session });
    }

    let target = store.join("projects").join(name);
    std::fs::create_dir_all(&target).map_err(|error| Refusal::NotApplicable {
        found: error.to_string(),
    })?;
    // Replaced, not edited: a symlink has no atomic re-aim, and the window between removing and
    // creating is why this is refused under a live session in the first place.
    crate::dir_link::remove(&link).map_err(|found| Refusal::NotApplicable { found })?;
    crate::dir_link::create(&target, &link).map_err(|found| Refusal::NotApplicable { found })?;
    Ok(target)
}

/// What an import moved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    /// Files copied into the store.
    pub copied: Vec<String>,
    /// Files already in the store under the same name, left as they were.
    pub kept: Vec<String>,
    /// Local versions that differed from the store's and were set aside instead of destroyed.
    pub quarantined: Vec<String>,
}

/// Copies a real `projects/<enc>` directory into the store and puts a link in its place.
///
/// This is the directory the CLI creates when a session starts before any link exists. Its
/// transcripts are on this machine only, and until they are copied they are one disk failure away
/// from gone.
///
/// # Errors
///
/// A [`Refusal`], or the text of a file-system failure.
pub fn import_real_directory(
    config_dir: &Path,
    store: &Path,
    enc: &str,
    name: &str,
    portable_cwd: &str,
    engine_dir: &Path,
    stamp: &str,
) -> Result<Imported, Refusal> {
    let real = config_dir.join("projects").join(enc);
    let metadata = std::fs::symlink_metadata(&real).map_err(|_| Refusal::NothingThere)?;
    if metadata.is_symlink() || !metadata.is_dir() {
        return Err(Refusal::NotApplicable {
            found: "a link, not a real directory".to_owned(),
        });
    }
    if let Some((machine, session)) = live_in(store, portable_cwd) {
        return Err(Refusal::SessionLive { machine, session });
    }

    let target = store.join("projects").join(name);
    let mut imported = Imported::default();
    copy_into(&real, &target, "", &mut imported, engine_dir, stamp)
        .map_err(|found| Refusal::NotApplicable { found })?;

    // The directory goes only after every file is in the store: a crash halfway must leave the
    // originals, not a hole.
    std::fs::remove_dir_all(&real).map_err(|error| Refusal::NotApplicable {
        found: error.to_string(),
    })?;
    crate::dir_link::create(&target, &real).map_err(|found| Refusal::NotApplicable { found })?;
    Ok(imported)
}

/// Copies a tree, never over a file the store already has: what is there arrived through a merge
/// and knows more than this machine's copy.
fn copy_into(
    from: &Path,
    to: &Path,
    prefix: &str,
    imported: &mut Imported,
    engine_dir: &Path,
    stamp: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let target = to.join(&name);
        if entry.path().is_dir() {
            copy_into(
                &entry.path(),
                &target,
                &relative,
                imported,
                engine_dir,
                stamp,
            )?;
        } else if target.exists() {
            // The store's version wins — it came through a merge and knows more. But the local
            // one is not simply dropped: the directory is deleted right after this, so a
            // discarded version is gone for ever. That is exactly how this project's own memory
            // index was lost on 2026-09-07. A differing local version goes to the quarantine.
            let ours = std::fs::read(entry.path()).unwrap_or_default();
            let theirs = std::fs::read(&target).unwrap_or_default();
            if ours != theirs {
                let saved = format!("{}-{stamp}", relative.replace('/', "-"));
                let path = crate::memory::quarantine(engine_dir, &saved, &ours)?;
                imported.quarantined.push(path.display().to_string());
            }
            imported.kept.push(relative);
        } else {
            std::fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
            imported.copied.push(relative);
        }
    }
    Ok(())
}

/// The sessions live anywhere, as a set of ids — for callers that only need to know "is it busy".
#[must_use]
pub fn live_session_ids(store: &Path) -> BTreeSet<String> {
    live_everywhere(store)
        .into_iter()
        .map(|(_, session, _)| session)
        .collect()
}
