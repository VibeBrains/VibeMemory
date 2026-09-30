//! Sessions of agents other than Claude Code, handed to the engine as a finished file.
//!
//! Claude Code writes its transcripts into the store itself, through a link; another agent keeps
//! its log in a format of its own, and the engine does not read that format: it belongs to the
//! other product and changes with it, and a reader that falls behind skips sessions without a word.
//! The agent converts its log to lines this module accepts, and `vibememory session put` places
//! them. The format is described for people in `docs/manuals/foreignSessionSpec.md`.
//!
//! Such a session lives beside Claude Code's, one directory down —
//! `projects/<project>/agents/<agent>/<session>.jsonl` — and never among them: Claude Code lists
//! every `*.jsonl` at the top of a project as its own session, and a line it cannot resume would
//! reach its `/resume` list.

use std::collections::BTreeSet;

use serde_json::Value;

/// The directory of a project that holds the sessions of other agents, one directory per agent.
pub const AGENTS_DIR: &str = "agents";

/// The agent whose sessions sit at the top of a project directory.
pub const CLAUDE_CODE: &str = "claude-code";

/// Where a session lives, relative to the root of a store.
///
/// `agent` is `None` for Claude Code's own sessions.
#[must_use]
pub fn session_path(project: &str, agent: Option<&str>, session: &str) -> String {
    match agent {
        Some(agent) => format!("projects/{project}/{AGENTS_DIR}/{agent}/{session}.jsonl"),
        None => format!("projects/{project}/{session}.jsonl"),
    }
}

/// What a file that passed holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checked {
    /// Records in the file.
    pub lines: usize,
    /// Records the history search reads: `user` and `assistant` with words in a string or in a
    /// block of type `text`.
    pub spoken: usize,
}

/// Why a file was refused. `line` counts from 1; it is 0 for what concerns the file as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The line the problem is on.
    pub line: usize,
    /// What is wrong with it.
    pub problem: Problem,
}

/// What is wrong with a line, or with the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The file holds no record at all.
    Empty,
    /// The last line has no newline after it: a file cut in the middle of a record.
    NoFinalNewline,
    /// Not UTF-8.
    NotText,
    /// An empty line between records.
    BlankLine,
    /// Not a JSON object.
    NotAnObject,
    /// A field the engine needs is missing, or is not a non-empty string.
    Missing(&'static str),
    /// `timestamp` is not UTC in the form `2026-09-30T12:00:00Z`, with or without a fraction.
    Timestamp,
    /// `uuid` repeats one of an earlier line.
    DuplicateUuid,
    /// A `user` or `assistant` record whose `message.content` is neither a string nor a list of
    /// blocks.
    Content,
}

/// Checks a whole session file.
///
/// Every line is a JSON object with a non-empty `type`, a `uuid` unique in the file and a UTC
/// `timestamp`. The merge needs the last two: without `uuid` an edited line survives beside its
/// old version on the other machine, and without `timestamp` lines from two machines cannot be
/// put in order. A `user` or `assistant` record also needs `message.content`, which is what the
/// history search reads.
///
/// # Errors
///
/// The first line that breaks a rule, and which rule.
pub fn check(bytes: &[u8]) -> Result<Checked, Refusal> {
    let whole = |problem| Refusal { line: 0, problem };
    let text = std::str::from_utf8(bytes).map_err(|_| whole(Problem::NotText))?;
    if text.is_empty() {
        return Err(whole(Problem::Empty));
    }
    let Some(body) = text.strip_suffix('\n') else {
        return Err(whole(Problem::NoFinalNewline));
    };
    let mut checked = Checked::default();
    let mut seen = BTreeSet::new();
    for (index, line) in body.split('\n').enumerate() {
        let at = |problem| Refusal {
            line: index + 1,
            problem,
        };
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() {
            return Err(at(Problem::BlankLine));
        }
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line) else {
            return Err(at(Problem::NotAnObject));
        };
        let field = |name: &'static str| {
            record
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| at(Problem::Missing(name)))
        };
        let kind = field("type")?;
        let uuid = field("uuid")?;
        if !is_utc_timestamp(field("timestamp")?) {
            return Err(at(Problem::Timestamp));
        }
        if !seen.insert(uuid.to_owned()) {
            return Err(at(Problem::DuplicateUuid));
        }
        if kind == "user" || kind == "assistant" {
            let content = record
                .get("message")
                .and_then(|message| message.get("content"))
                .ok_or_else(|| at(Problem::Missing("message.content")))?;
            match content {
                Value::String(said) => {
                    if !said.trim().is_empty() {
                        checked.spoken += 1;
                    }
                }
                Value::Array(blocks) => {
                    if blocks
                        .iter()
                        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|block| block.get("text").and_then(Value::as_str))
                        .any(|said| !said.trim().is_empty())
                    {
                        checked.spoken += 1;
                    }
                }
                _ => return Err(at(Problem::Content)),
            }
        }
        checked.lines += 1;
    }
    Ok(checked)
}

/// `YYYY-MM-DDTHH:MM:SS`, an optional fraction, then `Z`. UTC only: the merge orders lines by
/// comparing these strings, and two offsets would compare as text, not as time.
fn is_utc_timestamp(stamp: &str) -> bool {
    let Some(rest) = stamp.strip_suffix('Z') else {
        return false;
    };
    let (seconds, fraction) = rest.split_once('.').unwrap_or((rest, "0"));
    let shape = seconds
        .bytes()
        .enumerate()
        .all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            10 => byte == b'T',
            13 | 16 => byte == b':',
            _ => byte.is_ascii_digit(),
        });
    shape
        && seconds.len() == 19
        && !fraction.is_empty()
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}
