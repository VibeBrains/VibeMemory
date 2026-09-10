//! One line of a JSONL file: its bytes, its identity and the timestamp it sorts by.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

pub(crate) use crate::jsonl::{LINE_END, line_body};

/// `type` of the state line that names the leaf a reader resumes from.
pub(crate) const LAST_PROMPT_TYPE: &str = "last-prompt";
/// `type` and `subtype` of the record that starts a compaction window.
pub(crate) const SYSTEM_TYPE: &str = "system";
/// Everything before it is invisible to the reader of a file above 5 MiB.
pub(crate) const COMPACT_BOUNDARY_SUBTYPE: &str = "compact_boundary";

/// The top-level fields a merge looks at. Every field is `Value` so that a wrong JSON type
/// (a numeric `uuid`, a null `timestamp`) leaves the line usable instead of opaque; unknown
/// fields are skipped without allocating.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Head {
    #[serde(default)]
    uuid: Option<Value>,
    #[serde(default)]
    timestamp: Option<Value>,
    #[serde(default)]
    parent_uuid: Option<Value>,
    #[serde(default)]
    leaf_uuid: Option<Value>,
    #[serde(default)]
    r#type: Option<Value>,
    #[serde(default)]
    subtype: Option<Value>,
}

/// Identity of a line: its `uuid` when it has one, its bytes otherwise. Byte identity is what
/// makes state lines (`custom-title`, `bridge-session`, …) and opaque lines mergeable at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Id<'a> {
    /// A record: identified by `uuid`, whatever else changed.
    Uuid(String),
    /// Anything else: identified by its exact bytes.
    Bytes(&'a [u8]),
}

/// Identity plus the ordinal of that identity within its own input: the k-th copy of a state
/// line matches the k-th copy on the other side.
pub(crate) type Key<'a> = (Id<'a>, usize);

/// A line together with everything the merge needs to place it.
#[derive(Debug, Clone)]
pub(crate) struct Line<'a> {
    /// The bytes of the line exactly as they were read, without the terminator. What goes back
    /// out: a merge rewrites line endings for nobody.
    pub(crate) bytes: &'a [u8],
    /// The same line without the carriage return git may have put before the terminator. What
    /// identity and every comparison are made of, so that one conversion cannot turn a line into
    /// two. See `crate::jsonl`.
    pub(crate) body: &'a [u8],
    /// What makes this line the same line on the other side.
    pub(crate) key: Key<'a>,
    /// Own `timestamp`, inherited from the previous line when absent.
    pub(crate) at: String,
    /// `parentUuid`, when it is a string: used to protect ancestors from deletion.
    pub(crate) parent_uuid: Option<String>,
    /// `leafUuid`, when it is a string: the leaf a `last-prompt` line points at.
    pub(crate) leaf_uuid: Option<String>,
    /// `type`, when it is a string.
    pub(crate) kind: Option<String>,
    /// `subtype`, when it is a string.
    pub(crate) subtype: Option<String>,
    /// The line is not a JSON object: kept byte for byte, identified by its bytes.
    pub(crate) opaque: bool,
}

impl Line<'_> {
    /// The `uuid` of this line, when it has one.
    pub(crate) fn uuid(&self) -> Option<&str> {
        match &self.key.0 {
            Id::Uuid(uuid) => Some(uuid),
            Id::Bytes(_) => None,
        }
    }

    /// The leaf a `last-prompt` line points at.
    pub(crate) fn leaf_target(&self) -> Option<&str> {
        if self.kind.as_deref() == Some(LAST_PROMPT_TYPE) {
            self.leaf_uuid.as_deref()
        } else {
            None
        }
    }

    /// The record that starts a compaction window.
    pub(crate) fn is_compact_boundary(&self) -> bool {
        self.kind.as_deref() == Some(SYSTEM_TYPE)
            && self.subtype.as_deref() == Some(COMPACT_BOUNDARY_SUBTYPE)
    }
}

/// The lines of one input, with the torn tail removed.
#[derive(Debug, Default)]
pub(crate) struct Side<'a> {
    /// Complete lines, in file order.
    pub(crate) lines: Vec<Line<'a>>,
    /// Bytes after the last `\n`, dropped.
    pub(crate) truncated: usize,
}

/// The leaf a reader resumes from, by the one rule both the merge report and `tails.json` obey.
///
/// Claude Code resumes from the leaf named by the last `last-prompt` line — not from the last
/// record and not from the newest timestamp. A compaction boundary clears that leaf, so only the
/// part of the file after the last boundary may name it. The rule lives here alone: written twice,
/// it would drift, and the two places that need it are the merge report the user reads and the
/// freshness gate that blocks a prompt.
///
/// `items` is any slice whose elements carry a line — the merged view holds lines with their
/// origin, a survey holds them bare — and `line` names that field.
pub(crate) fn resume_leaf<'a, T>(
    items: &'a [T],
    line: impl Fn(&'a T) -> &'a Line<'a>,
) -> Option<&'a str> {
    let after_boundary = items
        .iter()
        .rposition(|item| line(item).is_compact_boundary())
        .map_or(0, |index| index + 1);
    items
        .get(after_boundary..)
        .unwrap_or_default()
        .iter()
        .rev()
        .find_map(|item| line(item).leaf_target())
}

/// Splits an input into complete lines and reads the head of each.
///
/// Only the bytes up to the last `\n` are lines: a snapshot of a live transcript can catch a
/// record mid-write, and half a record must never become a line of its own.
pub(crate) fn read_side(bytes: &[u8]) -> Side<'_> {
    let boundary = super::snapshot_boundary(bytes);
    let body = bytes.get(..boundary).unwrap_or_default();
    let truncated = bytes.len() - boundary;
    // `body` is empty or ends with the separator; dropping that separator leaves exactly the
    // lines, so an empty line (two separators in a row) is preserved.
    let pieces: Vec<&[u8]> = match body.strip_suffix(&[LINE_END]) {
        Some(joined) => joined.split(|byte| *byte == LINE_END).collect(),
        None => Vec::new(),
    };

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(pieces.len());
    let mut ordinals: HashMap<Id<'_>, usize> = HashMap::new();
    let mut at = String::new();
    for raw in pieces {
        let body = line_body(raw);
        let head = parse_head(body);
        let uuid = head
            .as_ref()
            .and_then(|head| string(head.uuid.as_ref()))
            .filter(|uuid| !uuid.is_empty());
        let id = uuid.map_or(Id::Bytes(body), Id::Uuid);
        let ordinal = ordinals.entry(id.clone()).or_insert(0);
        *ordinal += 1;
        let key = (id, *ordinal);
        if let Some(own) = head
            .as_ref()
            .and_then(|head| string(head.timestamp.as_ref()))
        {
            at = own;
        }
        lines.push(Line {
            bytes: raw,
            body,
            key,
            at: at.clone(),
            parent_uuid: head
                .as_ref()
                .and_then(|head| string(head.parent_uuid.as_ref())),
            leaf_uuid: head
                .as_ref()
                .and_then(|head| string(head.leaf_uuid.as_ref())),
            kind: head.as_ref().and_then(|head| string(head.r#type.as_ref())),
            subtype: head.as_ref().and_then(|head| string(head.subtype.as_ref())),
            opaque: head.is_none(),
        });
    }
    Side { lines, truncated }
}

/// Reads the head of a line, or `None` when the line is not a JSON object.
///
/// The `{` check is not cosmetic: serde deserializes a struct from a JSON array positionally,
/// so `["a", "b"]` would otherwise be read as a record whose `uuid` is `"a"`.
fn parse_head(line: &[u8]) -> Option<Head> {
    let first = line.iter().find(|byte| !byte.is_ascii_whitespace())?;
    if *first != b'{' {
        return None;
    }
    serde_json::from_slice::<Head>(line).ok()
}

fn string(value: Option<&Value>) -> Option<String> {
    Some(value?.as_str()?.to_owned())
}
