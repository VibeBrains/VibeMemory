//! The log of `DeepSeek` Harness (DSH), read by the engine itself: `session agent add --preset dsh`.
//!
//! Every other agent converts its own log, as `foreign` says, and for them that stays the rule. DSH
//! is the exception because without it every person on DSH writes the same converter: a log of zstd
//! frames, records of its own, tokens of ours caught in it. The reasons are in
//! `docs/knowledge/design/foreignSessions.md`.
//!
//! The log is `session.v<N>.jsonl.zstd` in a directory of its own per session: a sequence of zstd
//! frames, each a batch of JSON lines. The first record is the header — session id, working
//! directory, the format's version. A version this engine was not written against is refused by
//! name: a reader that guessed would skip sessions without a word.
//!
//! The translation is the one DSH's own wrapper used, so a session handed over by it and one read
//! here land as the same records: a message of the person becomes `user`, a message of the model
//! `assistant`, everything else keeps its type and its `data`.

use std::collections::BTreeSet;
use std::io::Read;

use serde_json::{Value, json};

/// The version of the log this engine reads.
pub const LOG_VERSION: u64 = 4;

/// The name of a log starts with this, the version follows.
const LOG_PREFIX: &str = "session.v";

/// The name of a log ends with this.
const LOG_SUFFIX: &str = ".jsonl.zstd";

/// What a log turned into: the session as `session put` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The session id, from the header.
    pub id: String,
    /// The working directory, from the header, as DSH wrote it.
    pub cwd: String,
    /// The session in the format of `foreign::check`: a line per record, each ending in a newline.
    pub file: String,
    /// Records in the file.
    pub records: usize,
}

/// Why a log gives no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Nothing is written yet, or only the header: a session that has just begun. Not a failure —
    /// there is nothing to hand over yet.
    Empty,
    /// The decompressed text is not UTF-8.
    NotText,
    /// The first record is not a header with a session id and a working directory.
    NoHeader,
    /// The header names a version of the format this engine does not know.
    UnknownVersion(u64),
    /// A record that is not JSON, before the last line: the log is damaged, not merely being written.
    Record {
        /// The line, counting from 1 in the decompressed text.
        line: usize,
    },
}

/// Whether a file name is a DSH log: `session.v<digits>.jsonl.zstd`. A version this engine does not
/// read is a log all the same — reading it gives the refusal that says to update, where skipping it
/// would leave the sessions behind without a word.
#[must_use]
pub fn is_log_name(name: &str) -> bool {
    name.strip_prefix(LOG_PREFIX)
        .and_then(|rest| rest.strip_suffix(LOG_SUFFIX))
        .is_some_and(|version| {
            !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Turns a log into a session.
///
/// The last frame of a log that is being written may be cut; it is dropped, and the next look
/// takes it whole. So is a last line cut in the middle.
///
/// # Errors
///
/// [`Problem`]: nothing to hand over yet, or a log this engine cannot read and why.
pub fn convert(raw: &[u8]) -> Result<Session, Problem> {
    let text = String::from_utf8(frames(raw)).map_err(|_| Problem::NotText)?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A text that ends in a newline leaves an empty piece after it; one that does not was cut in
    // the middle of its last line.
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
    let Some((header, body)) = records.split_first() else {
        return Err(Problem::Empty);
    };
    if header.get("type").and_then(Value::as_str) != Some("session") {
        return Err(Problem::NoHeader);
    }
    let version = header.get("version").and_then(Value::as_u64);
    if version != Some(LOG_VERSION) {
        return Err(version.map_or(Problem::NoHeader, Problem::UnknownVersion));
    }
    let text_of = |name| {
        header
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let (Some(id), Some(cwd)) = (text_of("id"), text_of("cwd")) else {
        return Err(Problem::NoHeader);
    };
    let mut used = BTreeSet::new();
    let mut file = String::new();
    let mut count = 0;
    for (index, record) in body.iter().enumerate() {
        let Some(line) = translate(record, &id, index, &mut used) else {
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

/// Every whole frame of the log, decompressed and joined. A frame that does not decompress ends the
/// reading: in a live log that is the last one, still being written.
fn frames(raw: &[u8]) -> Vec<u8> {
    let mut rest = raw;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let Ok(mut decoder) = ruzstd::decoding::StreamingDecoder::new(&mut rest) else {
            break;
        };
        let mut frame = Vec::new();
        if decoder.read_to_end(&mut frame).is_err() {
            break;
        }
        out.extend_from_slice(&frame);
    }
    out
}

/// One record of the log as a line of the session, or nothing for a record without a type or a
/// time: the merge cannot place such a line, and DSH writes none.
///
/// `index` is the record's place after the header, the id of last resort when the record has no
/// `seq`; `used` keeps the ids already given, so no two lines share one.
fn translate(
    record: &Value,
    session: &str,
    index: usize,
    used: &mut BTreeSet<String>,
) -> Option<String> {
    let millis = record.get("time").and_then(Value::as_f64)?;
    if !millis.is_finite() {
        return None;
    }
    // Milliseconds are whole in every log seen; a fraction is cut as JavaScript's `Date` cuts it.
    #[allow(clippy::cast_possible_truncation)]
    let timestamp = crate::utc::iso8601_millis(millis.trunc() as i64);
    let kind = record
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| !kind.is_empty())?;
    let data = record.get("data");
    let seq = record
        .get("seq")
        .and_then(Value::as_u64)
        .map_or_else(|| format!("i{index}"), |seq| seq.to_string());
    let fallback = format!("{session}-{seq}");
    let own_id = |path: &[&str]| {
        path.iter()
            .try_fold(data?, |value, key| value.get(key))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    let (line, own) = match kind {
        "user/message" => (
            json!({
                "type": "user",
                "timestamp": timestamp,
                "message": {
                    "role": "user",
                    "content": data.and_then(|data| data.get("content")).cloned().unwrap_or_else(|| json!("")),
                },
            }),
            own_id(&["id"]),
        ),
        "assistant/message" => (
            json!({
                "type": "assistant",
                "timestamp": timestamp,
                "message": {
                    "role": "assistant",
                    "content": data
                        .and_then(|data| data.get("message"))
                        .and_then(|message| message.get("content"))
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                },
            }),
            own_id(&["message", "id"]),
        ),
        _ => (
            json!({
                "type": kind,
                "timestamp": timestamp,
                "data": data.cloned().unwrap_or(Value::Null),
            }),
            None,
        ),
    };
    let uuid = [own, Some(fallback.clone())]
        .into_iter()
        .flatten()
        .find(|candidate| !used.contains(candidate))
        .unwrap_or_else(|| format!("{fallback}-{index}"));
    used.insert(uuid.clone());
    let Value::Object(mut fields) = line else {
        return None;
    };
    fields.insert("uuid".to_owned(), Value::String(uuid));
    serde_json::to_string(&Value::Object(fields)).ok()
}
