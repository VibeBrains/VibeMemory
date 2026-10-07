//! Two rails around the tick: a cap on how much one run may delete, and a back-off after a run
//! keeps failing.
//!
//! Both are decisions, not actions — the numbers go in, the verdict comes out, and the tick does
//! the work. That keeps them testable without a store, a clock or a network.
//!
//! Neither is our invention: Claude Code's own memory sync holds mass deletions and pauses after
//! repeated failures, and both earned their place there
//! ([knowledge/claudeCode/nativeMemory.md](../../../docs/knowledge/claudeCode/nativeMemory.md)).
//! What is ours is where the line falls — see the two comments below, they are the whole design.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// How many transcripts one tick may remove before it stops and asks. Ten is more than a normal
/// day of forgetting single sessions and far less than a store-wide accident.
pub const DEFAULT_MAX_DELETIONS_PER_TICK: usize = 10;

/// Failing runs in a row before the store cycle is paused. Two could be one flaky network moment;
/// three is a pattern.
const FAILURES_BEFORE_PAUSE: u32 = 3;

/// The longest back-off, counted in runs skipped. At the scheduled two minutes a run, fifteen is
/// half an hour — long enough to stop hammering a dead host, short enough that a machine which
/// comes back is not left behind for a working day.
const MAX_RUNS_SKIPPED: u32 = 15;

/// Where the state lives, beside the engine's other small states.
pub const STATE_FILE: &str = "tick-state.json";

/// What the tick remembers between runs.
///
/// Runs, not wall time: the tick is scheduled every two minutes, so counting runs needs no clock
/// and no date arithmetic on strings, and a machine that was asleep does not wake up owing a
/// back-off it already served.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TickState {
    /// Runs that failed in a row; zero once one succeeds.
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Store cycles still to be skipped before trying again.
    #[serde(default)]
    pub runs_to_skip: u32,
    /// Deletions held back by the cap and still waiting for a person, from the last run that held
    /// them. Kept so `doctor` can say so without recounting the tombstones itself.
    #[serde(default)]
    pub deletions_held: usize,
    /// Directories the last run left alone, with the reason and what they hold.
    ///
    /// Two jobs at once: the tick reports only what is *not* already here, so a permanently
    /// ignored directory is named once rather than every two minutes; and `status` answers from
    /// this list instead of scanning again, so the answer costs nothing and matches what the
    /// engine actually saw.
    #[serde(default)]
    pub ignored: Vec<crate::tick::IgnoredDirectory>,
    /// The host refused this store's pushes for a reason of the team's — out of room, unpaid, the
    /// member removed, a path the team's store may not hold. Pushes wait; everything else goes on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<StorePause>,
    /// Why the last run that tried failed, in one line; `None` once a run works.
    /// `doctor` says it under the count of failing runs: a count alone does not say what to do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<String>,
}

/// One line of a failure for a person: the first line that says something.
/// git puts the cause first (`Permission denied (publickey)`, `Could not resolve hostname`) and its
/// Generic advice after it.
#[must_use]
pub fn failure_line(problem: &str) -> String {
    let mut lines = problem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    // "fetch …: " leads into git's own words, which start on the same line
    match lines.next() {
        Some(cause) if first.ends_with(':') => format!("{first} {cause}"),
        _ => first.to_owned(),
    }
}

/// A pause of a store's pushes, on the host's word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorePause {
    /// The host's code: `quota`, `readOnly`, `pathDenied`, …
    pub code: String,
    /// What the host said after it: the paths or the detail.
    pub lines: Vec<String>,
    /// When the pause began.
    pub since: String,
    /// When the host is asked again. An hour, not a day: a payment or a bigger plan is made in the
    /// cabinet at once, and the team should not wait until tomorrow to see its sessions go again.
    pub recheck_at: String,
}

impl TickState {
    /// Reads the state; a machine without one has never failed.
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

    /// Whether this run may talk to the store's remote at all.
    ///
    /// Only the remote. Everything the tick does locally — putting back a transcript that
    /// vanished, refreshing the heartbeat, projecting memory — runs on every tick regardless: a
    /// rail that switched off the work which *saves* data would be worse than the failure it is
    /// backing off from.
    #[must_use]
    pub fn store_cycle_allowed(&self) -> bool {
        self.runs_to_skip == 0
    }

    /// Whether this run may push: no pause, or its recheck is due. `stamp` and `recheck_at` are
    /// written by the same clock in one format, so they compare as text.
    #[must_use]
    pub fn push_due(&self, stamp: &str) -> bool {
        self.pause
            .as_ref()
            .is_none_or(|pause| stamp >= pause.recheck_at.as_str())
    }

    /// The state after a run, and whether the caller should announce a recovery.
    ///
    /// A recovery is announced exactly once, on the first run that works after failures: a sync
    /// that silently comes back is indistinguishable from one still broken, and the person who
    /// saw the first complaint deserves the second sentence.
    #[must_use]
    pub fn after_run(&self, failed: bool) -> (Self, bool) {
        if !self.store_cycle_allowed() {
            // A skipped run is neither a success nor a failure: it did not try.
            return (
                Self {
                    runs_to_skip: self.runs_to_skip.saturating_sub(1),
                    ..self.clone()
                },
                false,
            );
        }
        if failed {
            let failures = self.consecutive_failures.saturating_add(1);
            let skip = if failures >= FAILURES_BEFORE_PAUSE {
                // Doubling from one run, capped: 1, 2, 4, 8, 15, 15, …
                let doubled = 1u32
                    .checked_shl(failures - FAILURES_BEFORE_PAUSE)
                    .unwrap_or(MAX_RUNS_SKIPPED);
                doubled.min(MAX_RUNS_SKIPPED)
            } else {
                0
            };
            return (
                Self {
                    consecutive_failures: failures,
                    runs_to_skip: skip,
                    deletions_held: self.deletions_held,
                    ignored: self.ignored.clone(),
                    pause: self.pause.clone(),
                    last_failure: self.last_failure.clone(),
                },
                false,
            );
        }
        let recovered = self.consecutive_failures >= FAILURES_BEFORE_PAUSE;
        (
            Self {
                consecutive_failures: 0,
                runs_to_skip: 0,
                deletions_held: self.deletions_held,
                ignored: self.ignored.clone(),
                pause: self.pause.clone(),
                last_failure: None,
            },
            recovered,
        )
    }
}

/// What a run is allowed to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deletions {
    /// Go ahead: this many is within the cap, or the person released the run.
    Allowed,
    /// Refuse, and say how many were asked for.
    Held {
        /// How many transcripts the tombstones named.
        wanted: usize,
        /// The cap that was in force.
        cap: usize,
    },
}

/// Whether a run may carry out the deletions its tombstones ask for.
///
/// All or nothing, deliberately. Deleting the first ten of a hundred would leave the store in a
/// state nobody chose — half a project forgotten — and the next run would take ten more, so the
/// cap would slow an accident down instead of stopping it.
#[must_use]
pub fn deletions(wanted: usize, cap: usize, released: bool) -> Deletions {
    if released || wanted <= cap {
        Deletions::Allowed
    } else {
        Deletions::Held { wanted, cap }
    }
}
