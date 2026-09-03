//! What a merge did, in numbers and codes. The CLI logs this as one line and builds its
//! `additionalContext` from it; phrases are assembled there, never here.

use serde::Serialize;

/// Which version of the file a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Side {
    /// The version being overwritten: git `%A`, the store copy in copy-import.
    Ours,
    /// The incoming version: git `%B`, the real `projects/<enc>` file in copy-import.
    Theirs,
}

/// Counts for one input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SideReport {
    /// Bytes after the last `\n` — a torn tail, dropped.
    pub truncated: usize,
    /// Lines present only on this side.
    pub added: usize,
    /// Of those, lines carrying a `uuid` (conversation, not state).
    pub added_uuid: usize,
    /// Lines this side kept but the other deleted relative to the base: dropped.
    pub dropped: usize,
    /// Deletions not honoured because the line is an ancestor of a surviving record.
    pub resurrected: usize,
    /// The side shares no `uuid` with a non-empty base: treated as a different file, not as
    /// a deletion of everything.
    pub absent: bool,
}

/// Which branch a reader will show after a fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fork {
    /// The side of the last `uuid` line of the result.
    pub tail_side: Side,
    /// The side holding the leaf Claude Code resumes from: the `leafUuid` of the last
    /// `last-prompt` line. `None` when that leaf sits in the common prefix — neither branch is
    /// singled out, and the fresh turns of both machines stay out of the conversation.
    pub visible: Option<Side>,
}

/// The outcome of one merge.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    /// Bytes after the last `\n` of the base, dropped.
    pub base_truncated: usize,
    /// Counts for `ours`.
    pub ours: SideReport,
    /// Counts for `theirs`.
    pub theirs: SideReport,
    /// Uuids whose bytes differ on both sides with no base to arbitrate: both lines are kept.
    pub conflicts: Vec<String>,
    /// Result lines that are not a JSON object: kept byte for byte.
    pub opaque: usize,
    /// Lines added from `theirs` that landed before the last `compact_boundary` of `ours`: in a
    /// file above 5 MiB the reader never loads them.
    pub before_boundary: usize,
    /// The result is byte-identical to `ours`: the caller may skip the write.
    pub result_equals_ours: bool,
    /// Set when both sides added records.
    pub fork: Option<Fork>,
}
