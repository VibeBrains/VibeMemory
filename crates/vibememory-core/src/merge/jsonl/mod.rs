//! Three-way union of JSONL transcripts.
//!
//! Both callers use the same function: the git merge driver passes `%O %A %B`, and copy-import
//! passes an empty base — which is literally what git itself passes when the file was created on
//! both sides. Nothing is re-serialized: every byte of the result comes from an input.
//!
//! The rules, and why:
//!
//! - **Identity.** A record is its `uuid`; anything else (state lines, `journal.jsonl`, lines
//!   that are not JSON objects) is its bytes, and the k-th copy matches the k-th copy on the
//!   other side. Claude Code rewrites its whole state block whenever it has appended about
//!   32 KiB, so byte-identical state lines repeat dozens of times and their count carries no
//!   meaning: the reader folds them by `type` and keeps the last one.
//! - **The base decides membership, never order.** A line present in the base and on one side
//!   only was deleted by the other side, and the deletion is honoured — Claude Code deletes
//!   records itself (tombstones, and compaction when its feature gate is on), and git applies a
//!   one-sided deletion without ever calling this driver. The exception is a line that is the
//!   parent of a surviving record: dropping it would orphan the other machine's branch, so it
//!   comes back and is reported.
//! - **Order.** `ours` is the spine, in its own order. Lines only `theirs` has are inserted after
//!   the last record both sides share, in timestamp order; state lines never anchor anything,
//!   because both machines write byte-identical copies of them. Timestamps are compared as
//!   bytes: Claude Code writes `Date.toISOString()`, fixed width, and compares them as strings
//!   itself.
//! - **Nothing is thrown away** except the bytes after the last `\n`, which are half a record
//!   caught mid-write. A complete line that does not parse is kept as it is.

use std::collections::{HashMap, HashSet};

mod line;
mod report;

use line::{Id, Key, LINE_END, Line, Side as ParsedSide, read_side};
pub use report::{Fork, MergeReport, Side, SideReport};

/// The merged file and what the merge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutput {
    /// The bytes to write: every line terminated by `\n`, empty only when both inputs are.
    pub bytes: Vec<u8>,
    /// Counts and codes for the log, `doctor` and `additionalContext`.
    pub report: MergeReport,
}

/// The only way a merge can fail: the algorithm broke its own contract. There is no failure mode
/// in the data — every input has a safe, deterministic result — so the caller treats this as the
/// abort signal it already owes git (`git merge --abort`, never a commit with `MERGE_HEAD`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MergeError {
    /// The result would have lost, duplicated or invented a line.
    #[error("merge postcondition violated: {detail}")]
    PostconditionViolated {
        /// Which invariant failed.
        detail: &'static str,
    },
}

impl MergeError {
    /// Stable camelCase code of the variant.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::PostconditionViolated { .. } => "postconditionViolated",
        }
    }
}

/// Offset just after the last `\n` (0 when there is none): where a snapshot of a live transcript
/// ends. Claude Code calls a file torn by exactly this rule (last byte is not `\n`), and the Stop
/// hook commits `bytes[..snapshot_boundary(bytes)]` so half a record never reaches the store.
#[must_use]
pub fn snapshot_boundary(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .rposition(|byte| *byte == LINE_END)
        .map_or(0, |index| index + 1)
}

/// Where an emitted line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Ours,
    Theirs,
    /// Both sides have this line: a shared record, or the two halves of a kept-both pair.
    Both,
}

impl Origin {
    /// The side to report for the tail of the file: a shared line is attributed to `ours`, the
    /// spine.
    fn side(self) -> Side {
        match self {
            Self::Theirs => Side::Theirs,
            Self::Ours | Self::Both => Side::Ours,
        }
    }
}

struct Emitted<'a> {
    line: &'a Line<'a>,
    bytes: &'a [u8],
    origin: Origin,
}

/// What one transcript looks like from outside, without merging anything.
///
/// The tick writes this into `tails.json` so other machines can tell at a glance whether their
/// copy is behind, and the freshness gate of `UserPromptSubmit` compares it against the store
/// before letting a prompt through. Both need the leaf a reader would resume from, and that rule
/// is not theirs to re-derive — it is the same one the merge report uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSurvey {
    /// Complete lines: everything up to the last `\n`.
    pub lines: usize,
    /// Bytes after the last `\n`, which a snapshot must not carry.
    pub truncated: usize,
    /// The `uuid` of the last line that has one — the tail another machine appends after.
    pub last_uuid: Option<String>,
    /// The leaf a reader resumes from: the target of the last `last-prompt` line after the last
    /// compaction boundary. `None` when nothing names one and the reader falls back to the tail.
    pub resume_leaf: Option<String>,
    /// Lines that are not JSON objects. They are carried through merges untouched, but a growing
    /// count means the format moved and `doctor` should say so.
    pub opaque: usize,
}

/// Reads a transcript without merging it.
#[must_use]
pub fn survey(bytes: &[u8]) -> TranscriptSurvey {
    let side = line::read_side(bytes);
    TranscriptSurvey {
        lines: side.lines.len(),
        truncated: side.truncated,
        last_uuid: side
            .lines
            .iter()
            .rev()
            .find_map(|line| line.uuid())
            .map(str::to_owned),
        resume_leaf: line::resume_leaf(&side.lines, |line| line).map(str::to_owned),
        opaque: side.lines.iter().filter(|line| line.opaque).count(),
    }
}

/// Merges two versions of a JSONL file, using `base` to tell additions from deletions.
///
/// `base` may be empty: git passes an empty `%O` when the file was added on both sides, and
/// copy-import has no common ancestor at all.
pub fn merge_jsonl(base: &[u8], ours: &[u8], theirs: &[u8]) -> Result<MergeOutput, MergeError> {
    let base_side = read_side(base);
    let our_side = read_side(ours);
    let their_side = read_side(theirs);

    let base_keys: HashMap<Key<'_>, &[u8]> = index_bytes(&base_side);
    let our_keys: HashMap<Key<'_>, &Line<'_>> = index_lines(&our_side);
    let their_keys: HashMap<Key<'_>, &Line<'_>> = index_lines(&their_side);

    // A side that shares no record with the base is not a side that deleted everything: it is a
    // different file (a fresh transcript written at the same path after a relink). Then the base
    // has nothing to say and the merge is a plain union.
    let base_has_records = base_keys.keys().any(|(id, _)| matches!(id, Id::Uuid(_)));
    let our_absent = base_has_records && !shares_record(&base_keys, &our_keys);
    let their_absent = base_has_records && !shares_record(&base_keys, &their_keys);
    let arbitrating_base = if our_absent || their_absent {
        HashMap::new()
    } else {
        base_keys
    };

    let membership = decide_membership(
        &arbitrating_base,
        &our_side,
        &their_side,
        &our_keys,
        &their_keys,
    );

    let mut report = MergeReport {
        base_truncated: base_side.truncated,
        ours: SideReport {
            truncated: our_side.truncated,
            absent: our_absent,
            ..SideReport::default()
        },
        theirs: SideReport {
            truncated: their_side.truncated,
            absent: their_absent,
            ..SideReport::default()
        },
        ..MergeReport::default()
    };
    report.ours.dropped = membership.dropped_ours;
    report.ours.resurrected = membership.resurrected_ours;
    report.theirs.dropped = membership.dropped_theirs;
    report.theirs.resurrected = membership.resurrected_theirs;

    let mut conflicts: Vec<String> = Vec::new();
    let mut conflict_keys: HashSet<Key<'_>> = HashSet::new();
    let spine = build_spine(
        &our_side,
        &their_keys,
        &arbitrating_base,
        &membership.kept,
        &mut conflicts,
        &mut conflict_keys,
    );
    let their_added: Vec<&Line<'_>> = their_side
        .lines
        .iter()
        .filter(|line| membership.kept.contains(&line.key) && !our_keys.contains_key(&line.key))
        .collect();

    // "Added" means a new line, not one that came back: a resurrected ancestor is old work, and
    // counting it as new would announce a fork where neither machine took a turn.
    let our_added: Vec<&Line<'_>> = our_side
        .lines
        .iter()
        .filter(|line| membership.kept.contains(&line.key) && !their_keys.contains_key(&line.key))
        .filter(|line| !arbitrating_base.contains_key(&line.key))
        .collect();
    let their_new: Vec<&&Line<'_>> = their_added
        .iter()
        .filter(|line| !arbitrating_base.contains_key(&line.key))
        .collect();
    report.ours.added = our_added.len();
    report.ours.added_uuid = our_added
        .iter()
        .filter(|line| line.uuid().is_some())
        .count();
    report.theirs.added = their_new.len();
    report.theirs.added_uuid = their_new
        .iter()
        .filter(|line| line.uuid().is_some())
        .count();

    let emitted = weave(spine, &their_added);

    let mut bytes = Vec::with_capacity(ours.len() + theirs.len());
    for item in &emitted {
        bytes.extend_from_slice(item.bytes);
        bytes.push(LINE_END);
    }

    report.conflicts = conflicts;
    report.opaque = emitted.iter().filter(|item| item.line.opaque).count();
    report.before_boundary = count_before_boundary(&emitted);
    // A torn tail is dropped, so the result differs from the file on disk even when every line
    // survived: the caller must write it, or the half-record stays.
    report.result_equals_ours = our_side.truncated == 0 && bytes == ours;
    report.fork = fork(&emitted, &report);

    let input_lines: HashSet<&[u8]> = our_side
        .lines
        .iter()
        .chain(their_side.lines.iter())
        .map(|line| line.bytes)
        .collect();
    verify(&bytes, &membership.kept, &conflict_keys, &input_lines)?;
    Ok(MergeOutput { bytes, report })
}

fn index_lines<'a>(side: &'a ParsedSide<'a>) -> HashMap<Key<'a>, &'a Line<'a>> {
    side.lines
        .iter()
        .map(|line| (line.key.clone(), line))
        .collect()
}

fn index_bytes<'a>(side: &'a ParsedSide<'a>) -> HashMap<Key<'a>, &'a [u8]> {
    side.lines
        .iter()
        .map(|line| (line.key.clone(), line.bytes))
        .collect()
}

fn shares_record(base: &HashMap<Key<'_>, &[u8]>, side: &HashMap<Key<'_>, &Line<'_>>) -> bool {
    side.keys()
        .any(|key| matches!(key.0, Id::Uuid(_)) && base.contains_key(key))
}

struct Membership<'a> {
    kept: HashSet<Key<'a>>,
    dropped_ours: usize,
    dropped_theirs: usize,
    resurrected_ours: usize,
    resurrected_theirs: usize,
}

/// Classic three-way membership, plus the rule that an ancestor of a surviving record is never
/// dropped: a machine that compacted or retracted a record must not orphan the branch the other
/// machine built on it.
fn decide_membership<'a>(
    base: &HashMap<Key<'a>, &[u8]>,
    our_side: &'a ParsedSide<'a>,
    their_side: &'a ParsedSide<'a>,
    our_keys: &HashMap<Key<'a>, &'a Line<'a>>,
    their_keys: &HashMap<Key<'a>, &'a Line<'a>>,
) -> Membership<'a> {
    let mut kept: HashSet<Key<'a>> = HashSet::new();
    let mut deleted: Vec<(Key<'a>, Side)> = Vec::new();
    for (key, _) in our_keys.iter().chain(their_keys.iter()) {
        let in_ours = our_keys.contains_key(key);
        let in_theirs = their_keys.contains_key(key);
        let in_base = base.contains_key(key);
        // Only records can be deleted. A line keyed by its bytes is a state line (or an opaque
        // one): both machines rewrite the whole state block on their own schedule, so a missing
        // copy means "this machine wrote the block fewer times", not "this machine deleted it".
        // Their count is therefore the larger of the two, exactly as the rules promise.
        let deletable = matches!(key.0, Id::Uuid(_));
        if in_ours && in_theirs || !in_base || !deletable {
            kept.insert(key.clone());
        } else {
            let side = if in_ours { Side::Ours } else { Side::Theirs };
            deleted.push((key.clone(), side));
        }
    }

    let mut resurrected_ours = 0;
    let mut resurrected_theirs = 0;
    loop {
        let needed: HashSet<&str> = our_side
            .lines
            .iter()
            .chain(their_side.lines.iter())
            .filter(|line| kept.contains(&line.key))
            .filter_map(|line| line.parent_uuid.as_deref())
            .collect();
        let mut grew = false;
        deleted.retain(|(key, side)| {
            let Id::Uuid(uuid) = &key.0 else { return true };
            if !needed.contains(uuid.as_str()) {
                return true;
            }
            kept.insert(key.clone());
            match side {
                Side::Ours => resurrected_ours += 1,
                Side::Theirs => resurrected_theirs += 1,
            }
            grew = true;
            false
        });
        if !grew {
            break;
        }
    }

    Membership {
        kept,
        dropped_ours: deleted
            .iter()
            .filter(|(_, side)| *side == Side::Ours)
            .count(),
        dropped_theirs: deleted
            .iter()
            .filter(|(_, side)| *side == Side::Theirs)
            .count(),
        resurrected_ours,
        resurrected_theirs,
    }
}

/// The lines of `ours` that survive, with the bytes of shared records decided by the base. When
/// both sides rewrote the same record and the base cannot say which is newer, both lines are
/// kept: the reader folds records by `uuid` anyway, and dropping one would lose bytes that exist
/// nowhere else in copy-import.
fn build_spine<'a>(
    our_side: &'a ParsedSide<'a>,
    their_keys: &HashMap<Key<'a>, &'a Line<'a>>,
    base: &HashMap<Key<'a>, &[u8]>,
    kept: &HashSet<Key<'a>>,
    conflicts: &mut Vec<String>,
    conflict_keys: &mut HashSet<Key<'a>>,
) -> Vec<Emitted<'a>> {
    let mut spine = Vec::with_capacity(our_side.lines.len());
    for line in &our_side.lines {
        if !kept.contains(&line.key) {
            continue;
        }
        let Some(theirs) = their_keys.get(&line.key) else {
            spine.push(Emitted {
                line,
                bytes: line.bytes,
                origin: Origin::Ours,
            });
            continue;
        };
        if line.bytes == theirs.bytes {
            spine.push(Emitted {
                line,
                bytes: line.bytes,
                origin: Origin::Both,
            });
            continue;
        }
        match base.get(&line.key) {
            Some(base_bytes) if *base_bytes == theirs.bytes => {
                spine.push(Emitted {
                    line,
                    bytes: line.bytes,
                    origin: Origin::Both,
                });
            }
            Some(base_bytes) if *base_bytes == line.bytes => {
                spine.push(Emitted {
                    line,
                    bytes: theirs.bytes,
                    origin: Origin::Both,
                });
            }
            _ => {
                // Only a record can conflict: a line keyed by its bytes has equal bytes whenever
                // it has an equal key, so it never reaches this arm.
                if let Some(uuid) = line.uuid() {
                    conflicts.push(uuid.to_owned());
                    conflict_keys.insert(line.key.clone());
                }
                spine.push(Emitted {
                    line,
                    bytes: line.bytes,
                    origin: Origin::Both,
                });
                spine.push(Emitted {
                    line: theirs,
                    bytes: theirs.bytes,
                    origin: Origin::Both,
                });
            }
        }
    }
    spine
}

/// Inserts the lines only `theirs` has, never before the last record both sides share: no clock
/// may push a line ahead of one it already followed on its own machine. After that point the two
/// tails are interleaved by timestamp, so the file ends with the branch that was worked on last.
///
/// Position is not causality — that lives in `parentUuid`, and the reader rebuilds the chain from
/// it — so a resurrected ancestor may well come back after its own child. That is allowed; what
/// is not allowed is losing it.
fn weave<'a>(spine: Vec<Emitted<'a>>, their_added: &[&'a Line<'a>]) -> Vec<Emitted<'a>> {
    let cut = spine
        .iter()
        .rposition(|item| item.line.uuid().is_some() && item.origin == Origin::Both)
        .map_or(0, |index| index + 1);
    let mut emitted: Vec<Emitted<'a>> = Vec::with_capacity(spine.len() + their_added.len());
    let mut rest = spine.into_iter();
    for _ in 0..cut {
        if let Some(item) = rest.next() {
            emitted.push(item);
        }
    }
    let mut tail = rest.peekable();
    let mut incoming = their_added.iter().peekable();
    loop {
        match (tail.peek(), incoming.peek()) {
            (Some(ours), Some(theirs)) => {
                if theirs.at < ours.line.at {
                    if let Some(line) = incoming.next() {
                        emitted.push(Emitted {
                            line,
                            bytes: line.bytes,
                            origin: Origin::Theirs,
                        });
                    }
                } else if let Some(item) = tail.next() {
                    emitted.push(item);
                }
            }
            (Some(_), None) => {
                if let Some(item) = tail.next() {
                    emitted.push(item);
                }
            }
            (None, Some(_)) => {
                if let Some(line) = incoming.next() {
                    emitted.push(Emitted {
                        line,
                        bytes: line.bytes,
                        origin: Origin::Theirs,
                    });
                }
            }
            (None, None) => break,
        }
    }
    emitted
}

/// Records that ended up before the last compaction boundary of the result: above 5 MiB the
/// reader starts there and never loads what precedes it, so the caller must not promise those
/// lines are reachable. Whose boundary it is does not matter — the reader reads the file, not
/// its history — and neither does which machine added the line.
fn count_before_boundary(emitted: &[Emitted<'_>]) -> usize {
    let Some(boundary) = last_boundary(emitted) else {
        return 0;
    };
    emitted
        .get(..boundary)
        .unwrap_or_default()
        .iter()
        .filter(|item| item.origin != Origin::Both && item.line.uuid().is_some())
        .count()
}

/// Position of the last compaction boundary in the result.
fn last_boundary(emitted: &[Emitted<'_>]) -> Option<usize> {
    emitted
        .iter()
        .rposition(|item| item.line.is_compact_boundary())
}

/// Which branch a reader will show, when both machines added records.
fn fork(emitted: &[Emitted<'_>], report: &MergeReport) -> Option<Fork> {
    if report.ours.added_uuid == 0 || report.theirs.added_uuid == 0 {
        return None;
    }
    let tail_side = emitted
        .iter()
        .rev()
        .find(|item| item.line.uuid().is_some())
        .map_or(Side::Ours, |item| item.origin.side());
    let named_leaf = line::resume_leaf(emitted, |item| item.line);
    let visible = match named_leaf {
        // No `last-prompt` at all: the reader falls back to the last record of the file.
        None => Some(tail_side),
        Some(leaf) => emitted
            .iter()
            .find(|item| item.line.uuid() == Some(leaf))
            .and_then(|item| match item.origin {
                // The leaf is shared, or it is named but absent: neither branch is singled out,
                // and the caller must not guess one.
                Origin::Both => None,
                origin => Some(origin.side()),
            }),
    };
    Some(Fork { tail_side, visible })
}

/// Every line that passed the membership rule appears in the produced bytes exactly once — twice
/// for a record both sides rewrote — and nothing else appears at all.
///
/// The count is per identity, not per copy: the result renumbers copies of the same identity from
/// scratch, so a kept-both pair shows up as the first and second copy of one `uuid`. The check
/// reads the bytes that will be written rather than the plan that produced them, so it also
/// covers the assembly. It runs in production: the merged file goes straight to every machine,
/// and a silent loss there is unrecoverable.
fn verify(
    bytes: &[u8],
    kept: &HashSet<Key<'_>>,
    conflict_keys: &HashSet<Key<'_>>,
    input_lines: &HashSet<&[u8]>,
) -> Result<(), MergeError> {
    if !(bytes.is_empty() || bytes.ends_with(&[LINE_END])) {
        return Err(MergeError::PostconditionViolated {
            detail: "result does not end with a newline",
        });
    }
    let result = read_side(bytes);
    if result.truncated != 0 {
        return Err(MergeError::PostconditionViolated {
            detail: "result ends mid-line",
        });
    }

    let mut expected: HashMap<&Id<'_>, usize> = HashMap::new();
    for (id, _) in kept.iter().chain(conflict_keys.iter()) {
        *expected.entry(id).or_insert(0) += 1;
    }
    let mut actual: HashMap<&Id<'_>, usize> = HashMap::new();
    for line in &result.lines {
        if !input_lines.contains(line.bytes) {
            return Err(MergeError::PostconditionViolated {
                detail: "line is not from an input",
            });
        }
        *actual.entry(&line.key.0).or_insert(0) += 1;
    }
    if actual != expected {
        return Err(MergeError::PostconditionViolated {
            detail: "line lost, duplicated or invented",
        });
    }
    Ok(())
}
