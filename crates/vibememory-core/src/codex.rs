//! The log of Codex, read by the engine itself: `session agent add --preset codex`.
//!
//! Codex writes a session as `rollout-<time>-<id>.jsonl` under `~/.codex/sessions/<year>/<month>/<day>/`, one JSON
//! record a line: `{timestamp, type, payload}`. The first `session_meta` is the session's own header — its id and
//! working directory; a forked session carries its parent's header after its own, and the parent's history with it.
//!
//! The conversation is read from the events Codex shows a person: `event_msg` of `user_message` and `agent_message`.
//! The same words come again as `response_item` messages, wrapped with the instructions Codex sends the model, so
//! those are left out: a session read twice over would answer every search twice.

use serde_json::{Value, json};

pub use crate::dsh::Session;

/// The name of a log starts with this.
const LOG_PREFIX: &str = "rollout-";

/// The name of a log ends with this.
const LOG_SUFFIX: &str = ".jsonl";

/// Why a log gives no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Only the header, or not even that: a session that has just begun. Not a failure — there is nothing to hand
    /// over yet.
    Empty,
    /// The text is not UTF-8.
    NotText,
    /// No `session_meta` with a session id and a working directory.
    NoHeader,
    /// A record that is not JSON, before the last line: the log is damaged, not merely being written.
    Record {
        /// The line, counting from 1.
        line: usize,
    },
}

/// Whether a file name is a Codex log: `rollout-<anything>.jsonl`.
#[must_use]
pub fn is_log_name(name: &str) -> bool {
    name.strip_prefix(LOG_PREFIX)
        .and_then(|rest| rest.strip_suffix(LOG_SUFFIX))
        .is_some_and(|middle| !middle.is_empty())
}

/// Turns a log into a session. A last line cut in the middle is dropped: the next look takes it whole.
///
/// # Errors
///
/// [`Problem`]: nothing to hand over yet, or a log this engine cannot read and why.
pub fn convert(raw: &[u8]) -> Result<Session, Problem> {
    let text = std::str::from_utf8(raw).map_err(|_| Problem::NotText)?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    let cut = lines.pop().filter(|last| !last.trim().is_empty());
    let mut records = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record =
            serde_json::from_str::<Value>(line).map_err(|_| Problem::Record { line: index + 1 })?;
        records.push(record);
    }
    if let Some(record) = cut.and_then(|line| serde_json::from_str::<Value>(line).ok()) {
        records.push(record);
    }
    let header = records
        .iter()
        .find(|record| record.get("type").and_then(Value::as_str) == Some("session_meta"))
        .and_then(|record| record.get("payload"));
    let field = |name| {
        header
            .and_then(|header| header.get(name))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let (Some(id), Some(cwd)) = (field("id"), field("cwd")) else {
        return Err(if records.is_empty() {
            Problem::Empty
        } else {
            Problem::NoHeader
        });
    };
    let mut file = String::new();
    let mut count = 0;
    for (index, record) in records.iter().enumerate() {
        let Some(line) = translate(record, &id, index) else {
            continue;
        };
        file.push_str(&crate::token::redact(&line));
        file.push('\n');
        count += 1;
    }
    if count == 0 {
        return Err(Problem::Empty);
    }
    Ok(Session {
        id,
        cwd,
        file,
        records: count,
    })
}

/// One event of the conversation as a line of the session; everything else gives nothing.
fn translate(record: &Value, session: &str, index: usize) -> Option<String> {
    if record.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let timestamp = record
        .get("timestamp")
        .and_then(Value::as_str)
        .filter(|time| !time.is_empty())?;
    let payload = record.get("payload")?;
    let text = payload
        .get("message")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())?;
    let line = match payload.get("type").and_then(Value::as_str)? {
        "user_message" => json!({
            "type": "user",
            "timestamp": timestamp,
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        }),
        "agent_message" => json!({
            "type": "assistant",
            "timestamp": timestamp,
            "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
        }),
        _ => return None,
    };
    // the log has no ids of its own for these events: the line's place in the log names it, and stays the same
    // however often the growing log is read
    let uuid = format!("{session}-{index}");
    let Value::Object(mut fields) = line else {
        return None;
    };
    fields.insert("uuid".to_owned(), Value::String(uuid));
    serde_json::to_string(&Value::Object(fields)).ok()
}
