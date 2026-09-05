//! `forget <sid>` — asking every machine to drop a session.
//!
//! Deleting the file here and pushing would meet the other machine's copy as a tree conflict
//! (`DU`/`UD`), which no merge driver is even asked about: git resolves paths before it resolves
//! contents. The tick would then have to choose between aborting for ever and resurrecting the
//! session it was told to forget.
//!
//! So a forget is not a deletion but a statement, written into this machine's outbox — a file
//! only this machine writes, so there is nothing to conflict with. Every machine reads it on its
//! next tick and removes the session locally, the same way it learned about it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// The outbox file that holds them.
pub const FORGOTTEN_FILE: &str = "forgotten.json";

/// What one machine asks the others to forget.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Forgotten {
    /// Session id → when the request was made and what it covers.
    pub sessions: BTreeMap<String, Tombstone>,
}

/// One request to forget a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tombstone {
    /// When it was asked for, from the caller's clock.
    pub at: String,
    /// The store path of the transcript, so a machine that has it under another project name can
    /// still find it.
    pub path: String,
}

/// Records that a session is to be forgotten everywhere.
///
/// Nothing is deleted here: the tick of each machine, this one included, does the removal after
/// reading this file. That keeps deletion on the same path as every other change — one machine
/// writes, the rest read — instead of turning it into a merge nobody can resolve.
///
/// # Errors
///
/// The text of what went wrong.
pub fn forget(
    store: &Path,
    machine_id: &str,
    session_id: &str,
    path: &str,
    stamp: &str,
) -> Result<(), String> {
    let dir = store.join("machines").join(machine_id);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let file = dir.join(FORGOTTEN_FILE);

    let mut forgotten: Forgotten = match std::fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Forgotten::default(),
        Err(error) => return Err(format!("{}: {error}", file.display())),
    };
    // A repeated forget keeps the first request: the moment it was asked for is what another
    // machine compares against, and moving it forward would make an old tombstone look new.
    forgotten
        .sessions
        .entry(session_id.to_owned())
        .or_insert_with(|| Tombstone {
            at: stamp.to_owned(),
            path: path.to_owned(),
        });

    let text = serde_json::to_string_pretty(&forgotten).map_err(|error| error.to_string())?;
    let temporary = file.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &file).map_err(|error| error.to_string())
}

/// Reads what one machine asks to forget. Anything unreadable is nothing: a broken outbox file
/// may not turn into a deletion.
#[must_use]
pub fn read(store: &Path, machine_id: &str) -> Forgotten {
    let file = store.join("machines").join(machine_id).join(FORGOTTEN_FILE);
    std::fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
