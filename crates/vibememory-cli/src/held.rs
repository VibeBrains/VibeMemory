//! Session files kept on this machine because they hold an agent token.
//!
//! A transcript is the agent's raw output: a settings file a tool read comes back into it whole,
//! and so does a token a person pasted. Committed, it would sit in the store's history, on the host
//! and in its mirror for good — in a team's store, in front of every member. So no session file
//! that holds a token of a cabinet is committed: it stays in the store's working tree on this
//! machine, the next session hears of it once, and `doctor` names it with the token's public id for
//! as long as it is held. Only the token's public id is ever written down — never the secret.
//!
//! Which files are a session's own is `vibememory_core::export::is_session_file`; which text holds
//! a token is `vibememory_core::token`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use vibememory_core::export::is_session_file;
use vibememory_core::token::token_ids;

/// What is held, in the engine's own directory: the list names files of this machine's store.
const HELD_FILE: &str = "held.json";

/// The session files this machine keeps back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    /// Store path → why it is kept back.
    pub files: BTreeMap<String, HeldFile>,
}

/// One file kept back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeldFile {
    /// When it was first kept back.
    pub since: String,
    /// The public ids (`tk_<id>`) of the tokens it holds.
    pub tokens: BTreeSet<String>,
    /// The file as it was when last read: an unchanged file is not read again.
    pub seen: Option<Seen>,
}

/// Size and modification time of a file, enough to tell that it has not changed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    /// Bytes.
    pub size: u64,
    /// Nanoseconds since the epoch.
    pub modified: u128,
}

impl Seen {
    /// The file at `path` as it is now; `None` when it cannot be told.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            size: metadata.len(),
            modified,
        })
    }
}

impl Held {
    /// What is held; a machine that never held anything has an empty list.
    #[must_use]
    pub fn read(engine_dir: &Path) -> Self {
        std::fs::read_to_string(engine_dir.join(HELD_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write(&self, engine_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(engine_dir.join(HELD_FILE), text).map_err(|error| error.to_string())
    }
}

/// The sentence a session hears once, when a file is first kept back.
fn note(path: &str, tokens: &BTreeSet<String>) -> String {
    let named: Vec<&str> = tokens.iter().map(String::as_str).collect();
    format!(
        "VibeMemory: {path} stays on this machine and is not synced: it holds agent token {}. \
         Revoke it in the cabinet and connect the agent again; `vibememory doctor` lists what is \
         held.",
        named.join(", ")
    )
}

/// Records that `path` is kept back for `tokens`, seen as `seen`. A file newly held is announced
/// to the next session.
fn hold(
    engine_dir: &Path,
    held: &mut Held,
    path: &str,
    tokens: BTreeSet<String>,
    seen: Option<Seen>,
    stamp: &str,
) -> Result<(), String> {
    let since = held
        .files
        .get(path)
        .map_or_else(|| stamp.to_owned(), |file| file.since.clone());
    let fresh = held
        .files
        .get(path)
        .is_none_or(|file| file.tokens != tokens);
    if fresh {
        crate::merge_report::add_pending(engine_dir, &note(path, &tokens))?;
    }
    held.files.insert(
        path.to_owned(),
        HeldFile {
            since,
            tokens,
            seen,
        },
    );
    Ok(())
}

/// Of `paths` — store-relative, `/`-separated, about to be committed — the ones that may be: a
/// session file that holds a token is kept back and recorded, one that no longer does is released.
/// Other files pass as they are.
///
/// # Errors
///
/// A file that cannot be read, or a list that cannot be written.
pub fn screen(
    engine_dir: &Path,
    store: &Path,
    paths: &[String],
    stamp: &str,
) -> Result<Vec<String>, String> {
    let mut held = Held::read(engine_dir);
    let before = held.clone();
    let mut passed = Vec::new();
    for path in paths {
        if !is_session_file(path) {
            passed.push(path.clone());
            continue;
        }
        let file = store.join(path);
        let seen = Seen::of(&file);
        if seen.is_some() && held.files.get(path).and_then(|held| held.seen) == seen {
            continue;
        }
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            // A file gone since it was listed has nothing to commit and nothing to hold.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                held.files.remove(path);
                passed.push(path.clone());
                continue;
            }
            Err(error) => return Err(format!("{}: {error}", file.display())),
        };
        let tokens = token_ids(&bytes);
        if tokens.is_empty() {
            held.files.remove(path);
            passed.push(path.clone());
        } else {
            hold(engine_dir, &mut held, path, tokens, seen, stamp)?;
        }
    }
    if held != before {
        held.write(engine_dir)?;
    }
    Ok(passed)
}

/// Records what the `Stop` hook found in a live transcript it did not commit: it read the bytes
/// itself, cut at the last newline, and they hold `tokens`.
///
/// # Errors
///
/// The list cannot be written.
pub fn hold_snapshot(
    engine_dir: &Path,
    path: &str,
    tokens: BTreeSet<String>,
    stamp: &str,
) -> Result<(), String> {
    let mut held = Held::read(engine_dir);
    let before = held.clone();
    // The live file keeps growing: the tick reads it again rather than trust a size of a moment.
    hold(engine_dir, &mut held, path, tokens, None, stamp)?;
    if held != before {
        held.write(engine_dir)?;
    }
    Ok(())
}

/// Forgets `path`: the `Stop` hook committed it, so whatever held it is gone.
///
/// # Errors
///
/// The list cannot be written.
pub fn release(engine_dir: &Path, path: &str) -> Result<(), String> {
    let mut held = Held::read(engine_dir);
    if held.files.remove(path).is_some() {
        held.write(engine_dir)?;
    }
    Ok(())
}
