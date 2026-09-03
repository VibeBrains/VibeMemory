//! Merging everything in the store that is not a transcript: memory documents first of all, and
//! with them the side files of a session — `custom-title.json`, `workflows/*.json`,
//! `tool-results/*` including images.
//!
//! There is nothing to merge inside such a file: it has no records to key by, and a text merge
//! would either invent a document neither machine wrote or leave conflict markers that the next
//! commit would carry to every machine. So the file is chosen whole, and when both machines wrote
//! it the loser is not overwritten but set aside: memory is the one shared artefact both machines
//! legitimately edit every session, and "the newer one wins" loses the other half silently.
//!
//! The bytes are never interpreted: CRLF, images, invalid UTF-8 and empty files all pass through
//! untouched.

use serde::Serialize;

/// What the driver was asked to merge. `stamp` names the machine and the moment; the core has no
/// clock and no machine identity of its own, so the caller supplies both.
#[derive(Debug, Clone, Copy)]
pub struct KeepBothInput<'a> {
    /// Path of the file inside the store, `/`-separated (git hands it to the driver as `%P`).
    pub path: &'a str,
    /// The common ancestor; empty when the file was created on both sides.
    pub base: &'a [u8],
    /// The version being overwritten (git `%A`).
    pub ours: &'a [u8],
    /// The incoming version (git `%B`).
    pub theirs: &'a [u8],
    /// Goes into the quarantined file's name, e.g. `mac-main-2026-09-03T10-15-00Z`.
    pub stamp: &'a str,
}

/// The merged file and, when both sides wrote it, the version that was set aside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepBothOutcome {
    /// The bytes to write: always exactly one of the inputs.
    pub bytes: Vec<u8>,
    /// The other version, to be written into the quarantine directory under this name.
    pub quarantined: Option<Quarantined>,
    /// What happened, for the log and for `additionalContext`.
    pub report: KeepBothReport,
}

/// A version that was not written into the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quarantined {
    /// One path component: safe on macOS and Windows, unique per path and stamp.
    pub name: String,
    /// The bytes to save under that name.
    pub bytes: Vec<u8>,
}

/// How the file was decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    /// Both sides hold the same bytes.
    Identical,
    /// Only this machine changed the file.
    TookOurs,
    /// Only the other machine changed the file.
    TookTheirs,
    /// Both changed it: ours stays in place, theirs goes to quarantine.
    KeptBoth,
}

/// Whether the file is memory — the only kind whose conflict the owner has to resolve by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    /// Somewhere under a `memory/` directory of a project.
    Memory,
    /// A side file of a session: a conflict here is worth a log line, not a nudge.
    Other,
}

/// What the merge did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeepBothReport {
    /// Which version won.
    pub resolution: Resolution,
    /// Memory or a side file.
    pub kind: FileKind,
    /// Size of our version.
    pub ours_bytes: usize,
    /// Size of the incoming version.
    pub theirs_bytes: usize,
}

/// Directory name that marks a project's memory.
const MEMORY_DIR_NAME: &str = "memory";
/// Longest file name accepted by APFS and NTFS, in bytes of UTF-8.
const MAX_NAME_BYTES: usize = 255;
/// Every other character of a path becomes this one in a quarantined file's name.
const NAME_REPLACEMENT: char = '-';

/// Chooses the version to keep and, when both machines wrote the file, names the one set aside.
#[must_use]
pub fn keep_both(input: &KeepBothInput<'_>) -> KeepBothOutcome {
    let kind = if is_memory(input.path) {
        FileKind::Memory
    } else {
        FileKind::Other
    };
    let resolution = if input.ours == input.theirs {
        Resolution::Identical
    } else if input.theirs == input.base {
        Resolution::TookOurs
    } else if input.ours == input.base {
        Resolution::TookTheirs
    } else {
        Resolution::KeptBoth
    };
    let bytes = match resolution {
        Resolution::TookTheirs => input.theirs.to_vec(),
        _ => input.ours.to_vec(),
    };
    let quarantined = (resolution == Resolution::KeptBoth).then(|| Quarantined {
        name: quarantine_name(input.path, input.stamp),
        bytes: input.theirs.to_vec(),
    });
    KeepBothOutcome {
        bytes,
        quarantined,
        report: KeepBothReport {
            resolution,
            kind,
            ours_bytes: input.ours.len(),
            theirs_bytes: input.theirs.len(),
        },
    }
}

/// A path under a `memory/` directory: `projects/<name>/memory/MEMORY.md` and anything below it.
/// The segment has to be a directory, so a file merely named `memory` is a side file like any
/// other. Two edges stay: a project literally named `memory`, and a memory directory moved by
/// `autoMemoryDirectory` — both only change whether the owner is nudged, never what is kept.
fn is_memory(path: &str) -> bool {
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segment == MEMORY_DIR_NAME && segments.peek().is_some() {
            return true;
        }
    }
    false
}

/// The name a quarantined version is saved under: the whole path with every character outside
/// `[A-Za-z0-9._-]` replaced, the stamp appended, the extension kept so the file still opens in
/// an editor. The stamp makes collisions between machines and moments impossible — and, because
/// it is always there, the name can never end up being a reserved device name on Windows.
fn quarantine_name(path: &str, stamp: &str) -> String {
    let (stem, extension) = split_extension(path);
    let stamp = sanitize(stamp);
    let suffix = match extension {
        Some(extension) => format!("{NAME_REPLACEMENT}{stamp}.{}", sanitize(extension)),
        None => format!("{NAME_REPLACEMENT}{stamp}"),
    };
    let stem = sanitize(stem);
    // The tail of a path says what the file is, so a name too long for the file system loses its
    // head, not its name. Sanitizing first makes every character one byte, so the cut is simple.
    let head_budget = MAX_NAME_BYTES.saturating_sub(suffix.len());
    let head = stem
        .get(stem.len().saturating_sub(head_budget)..)
        .unwrap_or_default();
    format!("{head}{suffix}")
}

/// Splits off the extension of the last path segment: `a/b.md` → (`a/b`, `md`), `a/.keep` →
/// (`a/.keep`, none), `a/b` → (`a/b`, none).
fn split_extension(path: &str) -> (&str, Option<&str>) {
    let segment_start = path.rfind('/').map_or(0, |index| index + 1);
    let segment = path.get(segment_start..).unwrap_or_default();
    match segment.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < segment.len() => {
            let cut = segment_start + dot;
            (path.get(..cut).unwrap_or_default(), path.get(cut + 1..))
        }
        _ => (path, None),
    }
}

/// Keeps what is safe in a file name on both platforms and replaces everything else, so no
/// separator, no forbidden character and no control byte can survive into the name.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                NAME_REPLACEMENT
            }
        })
        .collect()
}
