//! `machines/<id>/links.json` — what one machine knows about its own links.
//!
//! Each machine writes only its own file and every machine reads all of them, so there is nothing
//! to merge: the reconciler of another machine reads this to learn which working directory a
//! store name belongs to, and creates the `projects/<enc>` link before any session needs it —
//! the fallback scans of both the CLI and Desktop are blind to symlinks, so a link that does not
//! exist beforehand is a link nobody will find.
//!
//! Two fields carry the whole caution of the file. `predicted` says the link was invented by a
//! reconciler from another machine's record rather than observed here, and `confirmedBy` holds
//! the `transcript_path` that later proved it right. A predicted link is never deleted and never
//! re-aimed: it costs nothing when wrong, while removing a link under a live session costs the
//! session.

use serde::{Deserialize, Serialize};

use crate::naming::{NamingError, PathSyntax, StoreName};

/// Where the record came from, which decides how much it may be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkSource {
    /// A session started here and the CLI told us its `transcript_path`: the encoding is not our
    /// guess but the CLI's own answer.
    Observed,
    /// The reconciler built it from another machine's record and the local roots.
    Reconciled,
    /// `install` or `migrate` created it from the state of this machine.
    Installed,
}

/// One link of one machine: which encoded directory of `~/.claude/projects` points at which store
/// name, and which working directory that is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkRecord {
    /// The name of the directory under `~/.claude/projects`, exactly as the CLI writes it.
    pub enc: String,
    /// The store directory it points at, as text; compare through [`LinkRecord::name`].
    pub name: String,
    /// The working directory in portable form, `{ROOT}/rel` — a local path here would mean
    /// nothing on the machine that reads it.
    pub cwd: String,
    /// The syntax the local path is written in on the machine that owns this record. Kept so a
    /// reader can tell `D:/a/b` from a POSIX path without guessing from the string.
    pub syntax: PathSyntax,
    /// How the record came to be.
    pub source: LinkSource,
    /// The link was invented rather than observed, and its `enc` may be wrong.
    #[serde(default)]
    pub predicted: bool,
    /// The `transcript_path` that proved a predicted link right, if one ever did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_by: Option<String>,
}

impl LinkRecord {
    /// The store name as a parsed value.
    ///
    /// # Errors
    ///
    /// [`NamingError`] when the text is not a legal store name — a record written by a newer
    /// version, or a file edited by hand. The caller warns through `doctor` and skips the record
    /// rather than failing the whole file: one bad line may not cost every other link.
    pub fn name(&self) -> Result<StoreName, NamingError> {
        StoreName::parse(&self.name)
    }

    /// Whether this record and another name the same store directory. Comparison goes through
    /// [`StoreName::key`], because two names differing only in case are one directory on APFS and
    /// two on ext4.
    #[must_use]
    pub fn same_name_as(&self, other: &Self) -> bool {
        match (self.name(), other.name()) {
            (Ok(ours), Ok(theirs)) => ours.key() == theirs.key(),
            // Unparseable names are compared as written: the reconciler must still notice that
            // two records disagree, even when neither can be understood.
            _ => self.name == other.name,
        }
    }

    /// Whether a predicted link has been proven right by an observation of this machine.
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmed_by.is_some()
    }
}

/// The file itself: the links of one machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinksFile {
    /// Format version, so a newer machine can refuse to guess at an older file instead of
    /// silently mis-reading it.
    pub version: u32,
    /// The records, in no significant order.
    pub links: Vec<LinkRecord>,
}

/// The version this build writes.
pub const LINKS_VERSION: u32 = 1;
