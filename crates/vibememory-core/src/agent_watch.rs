//! An agent without hooks of its own, watched by the tick: its log directory is looked at every run,
//! and a log written since the last look is handed to the agent's own wrapper, which turns it into
//! the format of `session put`.
//!
//! The engine never reads another agent's log itself: the wrapper is the one place that knows the
//! format, and a release of the engine does not wait on a release of somebody else's log.
//!
//! Here are the two decisions about it, apart from the file system: which logs are new since the
//! last look, and whether an agent has gone silent — logs keep being written, sessions stop coming.

use std::collections::BTreeMap;

/// How long logs may run ahead of the last `session put` before the agent counts as silent: seven
/// runs of the tick, enough for a slow wrapper and too little for a broken one to go unnoticed.
pub const SILENT_AFTER_SECONDS: u64 = 15 * 60;

/// The logs written since the last look, by name, oldest first: the order they were written in is
/// the order a person reads the history in.
///
/// `seen` is what the last look found, name and modification time; `found` is what is there now. A
/// log whose time moved either way counts as new: a clock set back still means the file changed.
#[must_use]
pub fn changed(seen: &BTreeMap<String, u64>, found: &BTreeMap<String, u64>) -> Vec<String> {
    let mut fresh: Vec<(&String, u64)> = found
        .iter()
        .filter(|(name, modified)| seen.get(*name) != Some(*modified))
        .map(|(name, modified)| (name, *modified))
        .collect();
    fresh.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(right.0)));
    fresh.into_iter().map(|(name, _)| name.clone()).collect()
}

/// Whether an agent has gone silent: its newest log is younger than the last delivery by more than
/// [`SILENT_AFTER_SECONDS`].
///
/// The last delivery is the last `session put` of the agent, or the moment it was registered when
/// there has been none: a log written before registration is history, not silence.
#[must_use]
pub fn is_silent(newest_log: u64, last_put: Option<u64>, registered: u64) -> bool {
    let delivered = last_put.map_or(registered, |put| put.max(registered));
    newest_log > delivered.saturating_add(SILENT_AFTER_SECONDS)
}
