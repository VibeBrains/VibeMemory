//! `vibememory session put`: a session of another agent, handed over as a finished file, lands in
//! the store the way a Claude Code session does after `hook stop`.
//!
//! The file is checked first and refused whole on the first line that breaks the format
//! (`vibememory_core::foreign`): a wrong field found at the door is a message to the agent that
//! wrote it, the same field found a week later is a search that quietly finds nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vibememory_core::foreign::{self, AGENTS_DIR, Checked, Problem};
use vibememory_core::naming::slug::is_slug;
use vibememory_core::naming::{IgnoreReason, Resolution};

use crate::config::Config;
use crate::hook::stop;
use crate::install::Layout;
use crate::stores::StoreOf;

/// What `session put` was asked to do.
pub struct Request<'a> {
    /// The agent the session is of: a directory in every store, so a slug.
    pub agent: &'a str,
    /// The session id: the file name, so a slug.
    pub session: &'a str,
    /// The session's working directory, canonical: it decides the store and the project.
    pub cwd: &'a str,
    /// The same directory as other machines read it, for the liveness mark.
    pub portable_cwd: &'a str,
    /// The file the agent wrote.
    pub source: &'a Path,
    /// The session is over: it leaves this machine's list of live sessions.
    pub ended: bool,
    /// Now, by this machine's clock.
    pub stamp: &'a str,
}

/// Where a session went and what became of it.
#[derive(Debug)]
pub struct Placed {
    /// The store it went into.
    pub store: StoreOf,
    /// Its project in that store.
    pub project: String,
    /// Its path inside the clone.
    pub relative: String,
    /// What the file holds.
    pub checked: Checked,
    /// Whether a commit was made; an unchanged file makes none.
    pub committed: bool,
    /// Agent tokens found in the file: then it stays on this machine and is not committed.
    pub held: Vec<String>,
}

/// Places one session.
///
/// # Errors
///
/// A sentence for the agent's log: a name that is not a slug, a file that breaks the format, a
/// directory no store takes, or git refusing.
pub fn put(layout: &Layout, config: &Config, request: &Request<'_>) -> Result<Placed, String> {
    for (what, name) in [("agent", request.agent), ("session id", request.session)] {
        if !is_slug(name) {
            return Err(format!(
                "{what} {:?} is not a name: lowercase latin letters, digits and '-', not at either \
                 end, at most 63",
                vibememory_core::terminal::printable(name)
            ));
        }
    }
    let bytes = std::fs::read(request.source)
        .map_err(|error| format!("{}: {error}", request.source.display()))?;
    let checked = foreign::check(&bytes).map_err(|refusal| {
        let problem = describe(&refusal.problem);
        if refusal.line == 0 {
            format!("{}: {problem}", request.source.display())
        } else {
            format!(
                "{} line {}: {problem}",
                request.source.display(),
                refusal.line
            )
        }
    })?;

    let syntax = crate::hook::session_start::host_syntax();
    let store = crate::stores::for_cwd(layout, config, request.cwd, syntax)?;
    let project = match crate::project::resolve(&store.clone, &config.naming, request.cwd) {
        Ok(Resolution::Named { name, .. }) => String::from(name),
        Ok(Resolution::Ignored { reason }) => {
            return Err(match reason {
                IgnoreReason::Pattern(pattern) => format!(
                    "{} is left alone by ignoreCwd {pattern}: its sessions stay on this machine",
                    request.cwd
                ),
                IgnoreReason::ProjectDirName(name) => format!(
                    "{} is left alone: CLAUDE_CODE_PROJECT_DIR_NAME is {name}",
                    request.cwd
                ),
            });
        }
        Err(error) => return Err(format!("{}: {error}", request.cwd)),
    };

    let relative = foreign::session_path(&project, Some(request.agent), request.session);
    let target = store.clone.join(&relative);
    place(&store.state_dir, &target, &bytes)?;

    let kept = crate::held::Kept::read(&store.state_dir);
    let stopped = stop::commit_snapshot(&store.clone, &target, &relative, request.stamp, &kept)?;
    if stopped.held.is_empty() {
        crate::held::release(&store.state_dir, &relative)?;
    } else {
        crate::held::hold_found(
            &store.state_dir,
            &relative,
            stopped.held.clone(),
            request.stamp,
        )?;
    }
    // No process of the agent is known here: the mark lapses on its own an hour after the last put,
    // unless `--end` clears it first.
    stop::record_progress(
        &store.clone,
        &store.machine_id,
        request.session,
        request.portable_cwd,
        &target,
        request.stamp,
        || None,
    )?;
    if request.ended {
        stop::record_end(&store.clone, &store.machine_id, request.session)?;
    }
    Ok(Placed {
        project,
        relative,
        checked,
        committed: stopped.committed,
        held: stopped.held.into_iter().collect(),
        store,
    })
}

/// Writes the file whole or not at all. The copy is made outside the clone and renamed in: the tick
/// commits whatever it finds under `projects/`, and a half-written file must never be among it.
fn place(state_dir: &Path, target: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("{} has no directory", target.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    std::fs::create_dir_all(state_dir)
        .map_err(|error| format!("{}: {error}", state_dir.display()))?;
    let temporary = state_dir.join(format!("session-put-{}.jsonl.tmp", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, target).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", target.display())
    })
}

/// A refusal in words.
fn describe(problem: &Problem) -> String {
    match problem {
        Problem::Empty => "holds no record".to_owned(),
        Problem::NoFinalNewline => {
            "the last line has no newline after it: the file was cut in the middle of a record"
                .to_owned()
        }
        Problem::NotText => "is not UTF-8 text".to_owned(),
        Problem::BlankLine => "an empty line between records".to_owned(),
        Problem::NotAnObject => "not a JSON object".to_owned(),
        Problem::Missing(field) => format!("{field} is missing or is not a non-empty string"),
        Problem::Timestamp => {
            "timestamp is not UTC in the form 2026-09-30T12:00:00Z (a fraction may follow the \
             seconds)"
                .to_owned()
        }
        Problem::DuplicateUuid => "uuid repeats an earlier line".to_owned(),
        Problem::Content => "message.content is neither a string nor a list of blocks".to_owned(),
    }
}

/// What the store holds of one agent's sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Delivered {
    /// Sessions in the store.
    pub sessions: usize,
    /// When the newest of them was last written, seconds since the epoch.
    pub newest: u64,
}

/// The sessions of other agents in one clone, by agent: what `status` shows, so a wrapper that
/// stopped handing sessions over shows up as a date that no longer moves.
#[must_use]
pub fn delivered(clone: &Path) -> BTreeMap<String, Delivered> {
    let mut found: BTreeMap<String, Delivered> = BTreeMap::new();
    for project in entries(&clone.join("projects")) {
        for agent in entries(&project.join(AGENTS_DIR)) {
            let Some(name) = agent
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
            else {
                continue;
            };
            for session in entries(&agent) {
                if session
                    .extension()
                    .is_none_or(|extension| extension != "jsonl")
                {
                    continue;
                }
                let modified = std::fs::metadata(&session)
                    .and_then(|data| data.modified())
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_secs());
                let entry = found.entry(name.clone()).or_default();
                entry.sessions += 1;
                entry.newest = entry.newest.max(modified);
            }
        }
    }
    found
}

/// The entries of a directory, or none when it is not there.
fn entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}
