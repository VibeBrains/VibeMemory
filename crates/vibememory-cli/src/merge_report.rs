//! Where a merge report goes after the merge.
//!
//! The driver produces numbers; three different readers need three different parts of them, and
//! sending everything to all three would train each of them to ignore the lot.
//!
//! * The **log** gets every merge, one line, so a machine's history can be reconstructed later.
//! * The **session** gets only what changes what a person would do next: a fork it may be reading
//!   the wrong branch of, records the reader will not load, a record silently collapsed by
//!   Desktop's own deduplication.
//! * **`doctor`** gets what suggests something is wrong with the format or the setup — dropped
//!   lines, resurrected ones, opaque lines, torn tails.

use std::collections::BTreeSet;
use std::path::Path;

use vibememory_core::merge::jsonl::MergeReport;

/// The name of the log the tick and the hooks append to.
pub const LOG_FILE: &str = "merge.log";

/// One report, split by who needs to hear it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Routed {
    /// One line for the log, always.
    pub log: String,
    /// Sentences for `additionalContext`, when the session should know.
    pub session: Vec<String>,
    /// Warnings for `doctor`, when something looks wrong rather than merely notable.
    pub doctor: Vec<String>,
}

/// Splits a report between the log, the session and `doctor`.
///
/// `duplicate_message_ids` are records that arrived carrying a `message.id` another record already
/// had. Measured on 2.1.258 and 2.1.232: the reader collapses those, so one of the two branches
/// disappears for it even though the file holds both. The merge is right and the file is right —
/// but only a person can decide which of the two to keep, so this is the one thing worth
/// interrupting them about.
#[must_use]
pub fn route(path: &str, report: &MergeReport, duplicate_message_ids: &BTreeSet<String>) -> Routed {
    let mut routed = Routed {
        log: format!(
            "{path}: +{} ours, +{} theirs, dropped {}/{}, resurrected {}/{}, opaque {}, \
             conflicts {}, before boundary {}",
            report.ours.added,
            report.theirs.added,
            report.ours.dropped,
            report.theirs.dropped,
            report.ours.resurrected,
            report.theirs.resurrected,
            report.opaque,
            report.conflicts.len(),
            report.before_boundary,
        ),
        ..Routed::default()
    };

    if let Some(fork) = report.fork {
        let visible = fork.visible.map_or_else(
            || "neither branch is singled out".to_owned(),
            |side| format!("the branch from {side:?} is the one it will show"),
        );
        routed.session.push(format!(
            "VibeMemory: {path} was continued on two machines and both branches are kept; \
             {visible}. The other branch is in the file and can be reached with /rewind."
        ));
    }
    if report.before_boundary > 0 {
        routed.session.push(format!(
            "VibeMemory: {} record(s) from the other machine landed before the last compaction \
             boundary of {path}. They are in the file, but the reader will not load them.",
            report.before_boundary
        ));
    }
    if !duplicate_message_ids.is_empty() {
        routed.session.push(format!(
            "VibeMemory: {path} holds {} record(s) that share a message id with another record. \
             The file is correct, but Claude Code collapses records with the same message id, so \
             it will show only one of them.",
            duplicate_message_ids.len()
        ));
    }

    for (side, counts) in [("ours", &report.ours), ("theirs", &report.theirs)] {
        if counts.dropped > 0 {
            routed.doctor.push(format!(
                "{path}: {} line(s) of {side} were dropped as deletions",
                counts.dropped
            ));
        }
        if counts.resurrected > 0 {
            routed.doctor.push(format!(
                "{path}: {} deletion(s) of {side} were not honoured — the lines are ancestors of \
                 surviving records",
                counts.resurrected
            ));
        }
        if counts.truncated > 0 {
            routed.doctor.push(format!(
                "{path}: {} byte(s) of {side} were a torn tail and were left out",
                counts.truncated
            ));
        }
        if counts.absent {
            routed.doctor.push(format!(
                "{path}: {side} shares no record with the base — it was treated as a different \
                 file, not as a deletion of everything"
            ));
        }
    }
    if report.opaque > 0 {
        routed.doctor.push(format!(
            "{path}: {} line(s) are not JSON objects. They are carried through untouched, but a \
             growing count means the transcript format has moved",
            report.opaque
        ));
    }
    if !report.conflicts.is_empty() {
        routed.doctor.push(format!(
            "{path}: {} record(s) differ on both sides with no base to arbitrate; both are kept",
            report.conflicts.len()
        ));
    }

    routed
}

/// Appends one line to the engine's merge log.
///
/// # Errors
///
/// The text of what went wrong. A log that cannot be written is worth reporting and nothing more:
/// the merge itself already happened.
pub fn append_to_log(engine_dir: &Path, stamp: &str, line: &str) -> Result<(), String> {
    use std::io::Write;

    let dir = engine_dir.join("log");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(dir.join(LOG_FILE))
        .map_err(|error| error.to_string())?;
    writeln!(file, "{stamp} {line}").map_err(|error| error.to_string())
}

/// Records that share a `message.id` with an earlier one in the same file.
///
/// The id lives inside `message`, which the merge never looks at: union keys by `uuid`, and two
/// records with different uuids are two records. The reader disagrees, so this is checked here.
#[must_use]
pub fn duplicate_message_ids(bytes: &[u8]) -> BTreeSet<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut duplicated = BTreeSet::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        let Some(id) = value
            .get("message")
            .and_then(|message| message.get("id"))
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        if !seen.insert(id.to_owned()) {
            duplicated.insert(id.to_owned());
        }
    }
    duplicated
}

/// The file where notes wait for the next session to read them.
pub const PENDING_FILE: &str = "pending-notes.json";

/// Keeps the session-facing notes until somebody starts a session.
///
/// A merge happens in the tick, with nobody watching. `additionalContext` only exists inside a
/// session, so the note has to wait somewhere for one — otherwise the one thing a person needed
/// to know is said to an empty room.
///
/// # Errors
///
/// The text of what went wrong.
pub fn save_pending(engine_dir: &Path, routed: &Routed) -> Result<(), String> {
    if routed.session.is_empty() {
        return Ok(());
    }
    let path = engine_dir.join(PENDING_FILE);
    let mut waiting = read_pending(engine_dir);
    for note in &routed.session {
        if !waiting.contains(note) {
            waiting.push(note.clone());
        }
    }
    std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&waiting).map_err(|error| error.to_string())?;
    std::fs::write(&path, text.as_bytes()).map_err(|error| error.to_string())
}

/// Adds one note to what is waiting for a session, without a merge behind it.
///
/// The tick has things to say that no merge produced — a backup that stopped following the host,
/// for one — and they wait in the same place, so a session hears them all at once.
///
/// # Errors
///
/// The text of what went wrong.
pub fn add_pending(engine_dir: &Path, note: &str) -> Result<(), String> {
    let mut waiting = read_pending(engine_dir);
    if waiting.iter().any(|existing| existing == note) {
        return Ok(());
    }
    waiting.push(note.to_owned());
    std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&waiting).map_err(|error| error.to_string())?;
    std::fs::write(engine_dir.join(PENDING_FILE), text.as_bytes())
        .map_err(|error| error.to_string())
}

/// Notes waiting to be said to a session.
#[must_use]
pub fn read_pending(engine_dir: &Path) -> Vec<String> {
    std::fs::read_to_string(engine_dir.join(PENDING_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Forgets them, once a session has been told.
///
/// # Errors
///
/// The text of what went wrong.
pub fn clear_pending(engine_dir: &Path) -> Result<(), String> {
    match std::fs::remove_file(engine_dir.join(PENDING_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
