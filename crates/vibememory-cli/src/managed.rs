//! Managed copies: `CLAUDE.md` and `settings.json`, kept the same on every machine.
//!
//! Each of them exists twice — in `~/.claude`, where the CLI reads it, and in `<store>/config/`,
//! where the other machines can see it — and the two drift apart the moment somebody edits either.
//! Reconciling them is a three-way merge on whole files: the base is the content both sides agreed
//! on at the last sync, remembered as a hash in the engine directory. A side that still matches
//! the base has not moved; the other side then wins, whole. Both moved is a conflict, and a
//! conflict is not resolved here: both files stay as they are, this machine's version is set aside
//! in the quarantine so that nothing is lost while a person decides, and `doctor` names the file.
//!
//! For `settings.json` "the same" means the same JSON once this machine's own hook commands are
//! taken out: they name this machine's binary and engine directory and must never reach another
//! machine, so what the store holds is the shared part, and what comes back gets the hooks put on
//! top again.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::install::{Layout, MANAGED_FILES, for_the_store, nothing_to_share, write_hooks};

/// Where the last-synced hashes live, beside the engine's other small states.
pub const STATE_FILE: &str = "managed-state.json";

/// What both copies of each file agreed on at the last sync.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedState {
    /// File name → hash of the shared form the copies last agreed on.
    #[serde(default)]
    pub synced: BTreeMap<String, String>,
}

impl ManagedState {
    /// Reads the state; a machine without one has never synced anything.
    #[must_use]
    pub fn read(engine_dir: &Path) -> Self {
        std::fs::read_to_string(engine_dir.join(STATE_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the state.
    ///
    /// # Errors
    ///
    /// The text of what went wrong.
    pub fn write(&self, engine_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(engine_dir.join(STATE_FILE), text).map_err(|error| error.to_string())
    }
}

/// What the three hashes say has to happen. Pure: the decision, not the files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Neither side has the file, or this machine's copy holds nothing worth sharing.
    Nothing,
    /// The copies agree; only the base may need recording.
    Same,
    /// Only the store has it, or only the store moved: the store's version comes to the machine.
    Pull,
    /// Only the machine has it, or only the machine moved: the machine's version goes to the store.
    Push,
    /// Both moved since the last sync — or there is no record of one and they differ.
    Conflict,
    /// This machine's copy holds an issued token: it stays on this machine, whatever the hashes
    /// say. Pushed, the token would be in the store's history and its mirror for good.
    Withheld,
}

/// Why a managed copy holding a token is not shared, and what to do instead: a client is given its
/// token in `~/.claude.json`, which the engine never copies.
#[must_use]
pub fn withheld_reason(name: &str) -> String {
    format!(
        "{name} holds a vibememory token (vmt_…), so it is not copied to the store; register the \
         server with the line `vibememory connect` prints (`claude mcp add-json --scope user`, the \
         token read from its file on every connection) — that goes to ~/.claude.json, which the \
         engine never copies — and take the token out of {name}"
    )
}

/// Decides from the hashes of the shared form on each side and the last-synced base.
///
/// `None` means "no file" (or, for the base, "never synced"). A missing base with differing copies
/// is a conflict, not a guess: the first sync of two machines that were set up separately is
/// exactly the moment a wrong guess would overwrite a settings file somebody spent an hour on.
#[must_use]
pub fn verdict(local: Option<&str>, store: Option<&str>, base: Option<&str>) -> Verdict {
    match (local, store) {
        (None, None) => Verdict::Nothing,
        (Some(_), None) => Verdict::Push,
        (None, Some(_)) => Verdict::Pull,
        (Some(here), Some(there)) if here == there => Verdict::Same,
        (Some(here), Some(there)) => match base {
            Some(base) if base == here => Verdict::Pull,
            Some(base) if base == there => Verdict::Push,
            _ => Verdict::Conflict,
        },
    }
}

/// What one reconciliation of all managed files did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Files whose store copy was replaced by this machine's.
    pub pushed: Vec<String>,
    /// Files whose local copy was replaced by the store's.
    pub pulled: Vec<String>,
    /// Files both sides changed; left as they are, this machine's version in the quarantine.
    pub conflicting: Vec<String>,
    /// Files that hold a token and stay on this machine.
    pub withheld: Vec<String>,
}

/// Reconciles every managed file, recording the new base for each one that ends up agreed.
///
/// The store is passed, not derived from the engine directory: `Layout::store()` names where an
/// installed machine keeps it, and a reconciliation aimed at a path that merely should hold the
/// store would push into an empty directory and report agreement with it — seen in the test
/// harness, where the store lives beside the engine directory rather than under it.
///
/// # Errors
///
/// The text of what went wrong with the file system; a failure on one file stops the run so that
/// the state file never claims a sync that did not happen.
pub fn reconcile(layout: &Layout, store: &Path, stamp: &str) -> Result<Reconciled, String> {
    let mut state = ManagedState::read(&layout.engine_dir);
    let mut done = Reconciled::default();
    for name in MANAGED_FILES {
        match reconcile_one(layout, store, name, &mut state, stamp)? {
            Verdict::Push => done.pushed.push((*name).to_owned()),
            Verdict::Pull => done.pulled.push((*name).to_owned()),
            Verdict::Conflict => done.conflicting.push((*name).to_owned()),
            Verdict::Withheld => done.withheld.push((*name).to_owned()),
            Verdict::Same | Verdict::Nothing => {}
        }
    }
    state.write(&layout.engine_dir)?;
    Ok(done)
}

/// Reconciles one managed file and updates `state` for it; the caller writes the state.
///
/// # Errors
///
/// The text of what went wrong with the file system.
pub fn reconcile_one(
    layout: &Layout,
    store: &Path,
    name: &str,
    state: &mut ManagedState,
    stamp: &str,
) -> Result<Verdict, String> {
    let local_path = layout.config_dir.join(name);
    let store_path = store.join("config").join(name);

    let local = read_optional(&local_path)?;
    // A settings file holding nothing but this machine's own hooks has nothing to share; treating
    // it as content would push an empty object to every machine.
    let local = local.filter(|bytes| !nothing_to_share(&local_path, bytes));
    let local_shared = local
        .as_deref()
        .map(|bytes| for_the_store(&local_path, bytes));
    // Before any hash: nothing is written anywhere and the base stays where it was, so the file
    // is reconciled as usual the moment the token is taken out.
    if local_shared
        .as_deref()
        .is_some_and(vibememory_core::token::holds_token)
    {
        return Ok(Verdict::Withheld);
    }
    let store_shared = read_optional(&store_path)?.map(|bytes| for_the_store(&store_path, &bytes));

    let local_hash = local_shared.as_deref().map(crate::sha256::hex);
    let store_hash = store_shared.as_deref().map(crate::sha256::hex);
    let base = state.synced.get(name).cloned();

    let decision = verdict(
        local_hash.as_deref(),
        store_hash.as_deref(),
        base.as_deref(),
    );
    match decision {
        Verdict::Nothing => {
            state.synced.remove(name);
        }
        Verdict::Same => {
            if let Some(hash) = local_hash {
                state.synced.insert(name.to_owned(), hash);
            }
        }
        Verdict::Pull => {
            let Some(bytes) = store_shared else {
                return Ok(Verdict::Nothing);
            };
            write_whole(&local_path, &bytes)?;
            // The shared form has no hook commands; this machine's go back on top, or the very
            // next session would run without the engine.
            if name == "settings.json" {
                write_hooks(layout)?;
            }
            if let Some(hash) = store_hash {
                state.synced.insert(name.to_owned(), hash);
            }
        }
        Verdict::Push => {
            let Some(bytes) = local_shared else {
                return Ok(Verdict::Nothing);
            };
            write_whole(&store_path, &bytes)?;
            if let Some(hash) = local_hash {
                state.synced.insert(name.to_owned(), hash);
            }
        }
        Verdict::Conflict => {
            // Neither file is touched. This machine's version is set aside so that whatever the
            // person decides, nothing has been lost in the meantime; the helper recognises a
            // version it already holds, so a tick every two minutes does not pile up copies.
            if let Some(bytes) = local_shared {
                crate::memory::quarantine(&layout.engine_dir, &format!("{name}-{stamp}"), &bytes)?;
            }
        }
        // Decided before the hashes, above; `verdict` never answers it.
        Verdict::Withheld => {}
    }
    Ok(decision)
}

/// The file's bytes, or `None` when it is not there.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes a file whole, through a temporary name: the CLI may read these while we write them.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("vibememory.tmp");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}
