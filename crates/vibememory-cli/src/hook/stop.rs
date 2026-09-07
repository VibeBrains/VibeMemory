//! `Stop`: commit what the session has written, without ever handing git the live file.
//!
//! The transcript is being appended to while this runs. `git add` would stage whatever the file
//! happens to hold at the moment it is read — including half of a record that the CLI is still
//! writing — and that half-record would then travel to every machine. So the engine reads the
//! bytes itself, cuts them at the last newline (`snapshot_boundary`), and puts *that* into the
//! object database with `hash-object -w --stdin`. The file is never staged, and `git add -A` is
//! never used at all: it would sweep in everything else the directory happens to contain.
//!
//! What is committed is therefore always a whole number of records, and never fresher than the
//! moment the hook ran — which is exactly what another machine can safely merge.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vibememory_core::merge::jsonl::{snapshot_boundary, survey};

use crate::git;

/// Mode bits of an ordinary file in a git tree.
const BLOB_MODE: &str = "100644";
/// How long any one git command may take before the hook gives up on it.
const TIMEOUT: Duration = Duration::from_secs(20);

/// What one machine says about the sessions it is running.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    /// Session id → when it was last seen alive and where it is working.
    pub sessions: BTreeMap<String, LiveSession>,
}

/// One live session of this machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    /// ISO-8601 UTC, from the caller: the core has no clock and the engine keeps it that way.
    pub at: String,
    /// The working directory, portable form when a root covers it.
    pub cwd: String,
}

/// How far each session's transcript has got, so another machine can tell at a glance whether its
/// copy is behind.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tails {
    /// Session id → the tail of its transcript.
    pub sessions: BTreeMap<String, Tail>,
}

/// The tail of one transcript.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Tail {
    /// The `uuid` of the last record that has one.
    pub last_uuid: Option<String>,
    /// How many complete lines the file holds.
    pub lines: usize,
    /// The leaf a reader would resume from.
    pub resume_leaf: Option<String>,
    /// When this was measured.
    pub at: String,
}

/// What one `Stop` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stopped {
    /// The blob written for the snapshot, when there was anything to write.
    pub blob: Option<String>,
    /// Bytes left out because the last record was still being written.
    pub truncated: usize,
    /// Whether a commit was made. A session that added nothing since the last stop makes none.
    pub committed: bool,
}

/// Puts the current contents of `transcript` into the store's index and commits, if anything
/// changed.
///
/// `relative` is the path inside the store, `/`-separated. `stamp` is the moment, from the
/// caller.
///
/// # Errors
///
/// The text of what went wrong. Every caller of this is a hook, so the text is for a log and for
/// `additionalContext`, never for an exit code.
pub fn commit_snapshot(
    store: &Path,
    transcript: &Path,
    relative: &str,
    stamp: &str,
) -> Result<Stopped, String> {
    let bytes = match std::fs::read(transcript) {
        Ok(bytes) => bytes,
        // No transcript yet is not a failure: a session can stop before writing anything.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Stopped::default()),
        Err(error) => return Err(format!("{}: {error}", transcript.display())),
    };
    let boundary = snapshot_boundary(&bytes);
    let truncated = bytes.len() - boundary;
    let whole = bytes.get(..boundary).unwrap_or_default();
    if whole.is_empty() {
        return Ok(Stopped {
            blob: None,
            truncated,
            committed: false,
        });
    }

    // The bytes go straight into the object database. The live file is never staged.
    let blob = git::run_with_input(
        git::command(store, &["hash-object", "-w", "--stdin"]),
        whole,
        TIMEOUT,
    )?
    .ok_or_else(|| "git refused to write the snapshot".to_owned())?;

    let cacheinfo = format!("{BLOB_MODE},{blob},{relative}");
    git::run_with_timeout(
        git::command(store, &["update-index", "--add", "--cacheinfo", &cacheinfo]),
        TIMEOUT,
    )?
    .ok_or_else(|| format!("git refused to index {relative}"))?;

    let committed = if index_differs_from_head(store)? {
        let message = format!("vibememory: {relative} at {stamp}");
        git::run_with_timeout(
            git::command(store, &["commit", "--quiet", "-m", &message]),
            TIMEOUT,
        )?
        .ok_or_else(|| "git refused to commit".to_owned())?;
        true
    } else {
        false
    };

    Ok(Stopped {
        blob: Some(blob),
        truncated,
        committed,
    })
}

/// Whether the index holds anything HEAD does not.
///
/// Dirtiness is measured against the index, never against the working tree: the working tree holds
/// live files that are none of the engine's business.
fn index_differs_from_head(store: &Path) -> Result<bool, String> {
    // No HEAD yet: the very first commit of a fresh store always has something to say.
    if git::run_with_timeout(
        git::command(store, &["rev-parse", "--verify", "HEAD"]),
        TIMEOUT,
    )?
    .is_none()
    {
        return Ok(true);
    }
    let quiet = git::run_with_timeout(
        git::command(store, &["diff-index", "--quiet", "--cached", "HEAD", "--"]),
        TIMEOUT,
    )?;
    // `--quiet` exits 1 when there are differences, which the runner reports as a refusal.
    Ok(quiet.is_none())
}

/// Records that this session exists, before it has written anything.
///
/// `Stop` and `SessionEnd` also record liveness, but they fire at the end of a turn: a session
/// that has only just started is invisible to every machine until then, and the tick — which
/// copies real directories into the store and re-aims links — would treat its working directory
/// as idle. Measured the hard way: the tick moved a directory out from under a session that had
/// not finished its first turn.
///
/// # Errors
///
/// The text of what went wrong.
pub fn record_live(
    store: &Path,
    machine_id: &str,
    session_id: &str,
    cwd: &str,
    stamp: &str,
) -> Result<(), String> {
    let dir = store.join("machines").join(machine_id);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join("live.json");
    let mut live: Live = read_json(&path)?;
    live.sessions.insert(
        session_id.to_owned(),
        LiveSession {
            at: stamp.to_owned(),
            cwd: cwd.to_owned(),
        },
    );
    write_json(&path, &live)
}

/// Records that this session is alive, and how far its transcript has got.
///
/// Both files are per-machine and rewritten whole: nobody else writes them, so there is nothing
/// to merge and no conflict to have.
///
/// # Errors
///
/// The text of what went wrong.
pub fn record_progress(
    store: &Path,
    machine_id: &str,
    session_id: &str,
    cwd: &str,
    transcript: &Path,
    stamp: &str,
) -> Result<(), String> {
    let dir = store.join("machines").join(machine_id);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;

    let mut live: Live = read_json(&dir.join("live.json"))?;
    live.sessions.insert(
        session_id.to_owned(),
        LiveSession {
            at: stamp.to_owned(),
            cwd: cwd.to_owned(),
        },
    );
    write_json(&dir.join("live.json"), &live)?;

    let bytes = std::fs::read(transcript).unwrap_or_default();
    let found = survey(&bytes);
    let mut tails: Tails = read_json(&dir.join("tails.json"))?;
    tails.sessions.insert(
        session_id.to_owned(),
        Tail {
            last_uuid: found.last_uuid,
            lines: found.lines,
            resume_leaf: found.resume_leaf,
            at: stamp.to_owned(),
        },
    );
    write_json(&dir.join("tails.json"), &tails)
}

/// How long after a push the next one waits. A session writes many stops in a burst; pushing on
/// each would spend more time in git than in the session.
pub const PUSH_DEBOUNCE: Duration = Duration::from_secs(20);

/// When this machine last pushed, kept outside the store: it is nobody else's business.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PushState {
    /// Seconds since the epoch of the last push that was attempted.
    pub last_push: u64,
}

/// Pushes the store, unless the last push was too recent.
///
/// `now` is seconds since the epoch, from the caller. A failed push is not an error worth telling
/// the session about: the tick pushes again in two minutes, and the commit is already safe on
/// this disk.
///
/// # Errors
///
/// Only the state file failing to be written, which would make the debounce meaningless.
pub fn push_if_due(
    store: &Path,
    engine_dir: &Path,
    now: u64,
    debounce: Duration,
) -> Result<bool, String> {
    let path = engine_dir.join("push-state.json");
    let state: PushState = read_json(&path)?;
    if now.saturating_sub(state.last_push) < debounce.as_secs() {
        return Ok(false);
    }
    write_json(&path, &PushState { last_push: now })?;
    // The result is deliberately ignored: no remote, no network, a rejected push — all of them
    // mean "later", and none of them may reach the session.
    let _ = git::run_with_timeout(git::command(store, &["push", "--quiet"]), TIMEOUT);
    Ok(true)
}

/// Marks a session as no longer running on this machine.
///
/// `SessionEnd` is not a nicety: measured on 2.1.258, a session whose turn failed produces
/// `SessionEnd` and **no** `Stop` at all. Commit on `Stop` alone and every session that ended in
/// an error stays on one machine for ever.
///
/// # Errors
///
/// The text of what went wrong.
pub fn record_end(store: &Path, machine_id: &str, session_id: &str) -> Result<(), String> {
    let path = store.join("machines").join(machine_id).join("live.json");
    let mut live: Live = read_json(&path)?;
    if live.sessions.remove(session_id).is_none() {
        return Ok(());
    }
    write_json(&path, &live)
}

/// Reads a per-machine file, treating both "not there" and "unreadable" as empty: these files are
/// a convenience for other machines, and losing one may not stop a session.
fn read_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or_default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes a per-machine file whole, through a temporary file, so a reader never sees half of one.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}
