//! The merge drivers git calls, as subcommands of this binary.
//!
//! Git hands a driver three files (`%O` the base, `%A` ours, `%B` theirs) and expects the result
//! in `%A`. Two rules bound what happens here, and both are about the store never holding
//! something nobody wrote:
//!
//! * the result is written to `%A` **only** when the merge produced one — a driver that fails
//!   must leave the file exactly as git gave it, so the caller can abort cleanly;
//! * exit 1 means "this merge cannot be done", and the caller is expected to `git merge --abort`.
//!   Conflict markers are never written: neither driver can produce them.

use std::path::Path;

use vibememory_core::merge::jsonl::{MergeReport, merge_jsonl};
use vibememory_core::merge::keep_both::{KeepBothInput, KeepBothOutcome, keep_both};

/// Which driver git asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// Transcripts and journals: union by record.
    Jsonl,
    /// Everything else in the store: whole-file choice, with the loser set aside.
    KeepBoth,
}

impl Driver {
    /// The name git knows it by, as written in `.gitattributes`.
    #[must_use]
    pub fn git_name(self) -> &'static str {
        match self {
            Self::Jsonl => "vibememory-jsonl",
            Self::KeepBoth => "vibememory-keepboth",
        }
    }

    /// Reads the name given on the command line.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "jsonl" => Some(Self::Jsonl),
            "keepboth" => Some(Self::KeepBoth),
            _ => None,
        }
    }
}

/// What a driver run produced, for the log and for `doctor`.
#[derive(Debug, Clone)]
pub enum DriverOutcome {
    /// A transcript was merged.
    Jsonl(Box<MergeReport>),
    /// A whole file was chosen; the loser, if any, still has to be quarantined by the caller.
    KeepBoth(Box<KeepBothOutcome>),
}

/// Runs one driver over the three files git provided.
///
/// `path` is the file's path inside the store (`%P`), which `keep_both` needs to tell memory from
/// anything else. `stamp` names this machine and moment for a quarantined version.
///
/// # Errors
///
/// A sentence for the log. `%A` is left untouched whenever this returns an error — that is the
/// whole contract: git may then abort with the working tree exactly as it was.
pub fn run(
    driver: Driver,
    base_path: &Path,
    ours_path: &Path,
    theirs_path: &Path,
    path: &str,
    stamp: &str,
) -> Result<DriverOutcome, String> {
    let base = read(base_path)?;
    let ours = read(ours_path)?;
    let theirs = read(theirs_path)?;

    match driver {
        Driver::Jsonl => {
            let merged = merge_jsonl(&base, &ours, &theirs).map_err(|error| error.to_string())?;
            write_result(ours_path, &merged.bytes)?;
            Ok(DriverOutcome::Jsonl(Box::new(merged.report)))
        }
        Driver::KeepBoth => {
            let outcome = keep_both(&KeepBothInput {
                path,
                base: &base,
                ours: &ours,
                theirs: &theirs,
                stamp,
            });
            write_result(ours_path, &outcome.bytes)?;
            Ok(DriverOutcome::KeepBoth(Box::new(outcome)))
        }
    }
}

/// Reads one of git's three inputs. A missing file is empty: git passes an empty `%O` when the
/// file was added on both sides.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes the merged bytes over `%A`, through a temporary file in the same directory so that a
/// crash halfway cannot leave half a transcript where git expects a whole one.
fn write_result(ours_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = ours_path.with_extension("vibememory-merge");
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, ours_path).map_err(|error| error.to_string())
}
