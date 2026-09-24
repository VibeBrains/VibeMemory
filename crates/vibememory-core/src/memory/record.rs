//! What a memory is: an identity, a kind, a hook line and a body.
//!
//! Claude Code keeps memory as markdown files, one per fact, with a `MEMORY.md` index. Those
//! files are a projection here — the record is the thing that travels between machines and
//! agents — but the shape follows the format the model already writes, so nothing has to be
//! taught a new one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::error::MemoryError;

/// Longest identifier a record may carry; longer names make unwieldy file names and say nothing
/// more than their first words.
const MAX_ID_LEN: usize = 64;

/// The identity of a record: a short slug, stable for the life of the record, and also the name
/// of the file it is projected into.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RecordId(String);

impl RecordId {
    /// Accepts a lowercase kebab-case slug: what the memory format already uses for `name`, and
    /// what stays a legal file name on every platform the store is checked out on.
    pub fn parse(id: &str) -> Result<Self, MemoryError> {
        let shaped = !id.is_empty()
            && id.len() <= MAX_ID_LEN
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !id.starts_with('-')
            && !id.ends_with('-')
            && !id.contains("--");
        if shaped {
            Ok(Self(id.to_owned()))
        } else {
            Err(MemoryError::InvalidId { id: id.to_owned() })
        }
    }

    /// Builds an identifier from a human title: the same shape a person would have typed.
    #[must_use]
    pub fn from_title(title: &str) -> Option<Self> {
        let mut slug = String::with_capacity(title.len());
        for character in title.chars() {
            if character.is_ascii_alphanumeric() {
                slug.extend(character.to_lowercase());
            } else if !slug.ends_with('-') {
                slug.push('-');
            }
        }
        let slug = slug.trim_matches('-');
        let cut = slug
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= MAX_ID_LEN)
            .count();
        Self::parse(slug.get(..cut).unwrap_or_default().trim_end_matches('-')).ok()
    }

    /// The identifier as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for RecordId {
    type Error = MemoryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<RecordId> for String {
    fn from(value: RecordId) -> Self {
        value.0
    }
}

/// What a memory is about, in the vocabulary the memory format already uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordKind {
    /// Who the person is: role, expertise, preferences.
    User,
    /// Guidance the person gave on how to work, with its reason.
    Feedback,
    /// Ongoing work, goals and constraints not derivable from the repository.
    Project,
    /// A pointer to something outside: a URL, a dashboard, a ticket.
    Reference,
}

impl RecordKind {
    /// The word used in the file's frontmatter and in the journal.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
        }
    }

    /// Reads the word back; an unknown kind is an error rather than a silent default, because
    /// the kind decides how the memory is used.
    pub fn parse(word: &str) -> Result<Self, MemoryError> {
        match word {
            "user" => Ok(Self::User),
            "feedback" => Ok(Self::Feedback),
            "project" => Ok(Self::Project),
            "reference" => Ok(Self::Reference),
            other => Err(MemoryError::UnknownKind {
                kind: other.to_owned(),
            }),
        }
    }
}

/// Whether a memory can still be relied on.
///
/// Only two values, and the second one is the whole point: a fact that was true when it was
/// written and is not any more looks exactly like a fresh one, so a reader trusts it. The other
/// statuses a knowledge system usually carries are deliberately absent, because this model
/// already says the same things another way: a version with a child is superseded (`parent`),
/// a forgotten record is a `delete` event, and two machines disagreeing is a pair of rival
/// versions. Inventing fields that duplicate those would give the same fact two sources of truth.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum RecordStatus {
    /// Believed to hold. The default, and what every record written before this field existed is.
    #[default]
    Active,
    /// Was true, is not any more, or can no longer be trusted without checking. The record is
    /// kept: what it says still explains why things were done, and deleting it would lose that.
    Stale,
}

impl RecordStatus {
    /// The word used in the file's frontmatter and in the journal.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Stale => "stale",
        }
    }

    /// Whether this is the value that needs no writing down. Takes a reference because serde's
    /// `skip_serializing_if` hands it one.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Reads the word back. An unknown status is an error rather than a silent `active`: guessing
    /// here would quietly restore trust in a fact somebody marked as not to be trusted.
    ///
    /// # Errors
    ///
    /// [`MemoryError::UnknownStatus`] for a word this format does not define.
    pub fn parse(word: &str) -> Result<Self, MemoryError> {
        match word {
            "active" => Ok(Self::Active),
            "stale" => Ok(Self::Stale),
            other => Err(MemoryError::UnknownStatus {
                status: other.to_owned(),
            }),
        }
    }
}

/// One remembered fact — the thing that travels between machines and agents. The markdown file
/// under `memory/` is a projection of this, not the other way round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// Identity and file name.
    pub id: RecordId,
    /// What the memory is about.
    pub kind: RecordKind,
    /// Which project it belongs to. The store path says the same thing, but a record read over
    /// MCP arrives without a path, and memory is shared across projects there.
    pub project: String,
    /// One line saying what is inside: the hook the index shows and the model reads to decide
    /// whether the memory is relevant.
    pub description: String,
    /// The memory itself, as markdown.
    pub body: String,
    /// Other records this one points at, in the order they appear in the body.
    pub links: Vec<RecordId>,
    /// Whether the fact still holds. Absent in a journal written before this field existed, and
    /// left out of the line when it is `active`, so old journals and new ones stay comparable.
    #[serde(default, skip_serializing_if = "RecordStatus::is_default")]
    pub status: RecordStatus,
    /// Keys the CLI wrote into `metadata` that this format does not interpret — `node_type`,
    /// `originSessionId`, `modified` and whatever the next release adds.
    ///
    /// Carried, not understood. The projection is rewritten by the engine, and a field dropped on
    /// the way out is a field the CLI wrote and the engine quietly deleted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Which agent wrote this version — memory is shared between agents, so it has to say.
    pub agent: String,
    /// Which member of a team wrote this version, when a token or a machine key of a member
    /// wrote it. Absent on what the store's owner writes with their own engine, and on every line
    /// older than teams: a team shares one memory, and "which agent" alone does not say whose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// When the record was first written, in the writer's clock.
    pub created_at: String,
    /// When this version was written, in the writer's clock.
    pub updated_at: String,
}

impl Record {
    /// The human name to show. Derived, never stored: the real memory format has no title field,
    /// and a name is a way of showing a record rather than something it knows about itself.
    ///
    /// `promed-branch-names` becomes `Promed branch names` — the shape a person would have typed
    /// if asked for a heading, and stable, because it is a function of the identifier.
    #[must_use]
    pub fn title(&self) -> String {
        let mut out = String::with_capacity(self.id.as_str().len());
        for (index, word) in self.id.as_str().split('-').enumerate() {
            if index > 0 {
                out.push(' ');
            }
            let mut characters = word.chars();
            if index == 0
                && let Some(first) = characters.next()
            {
                out.extend(first.to_uppercase());
            }
            out.push_str(characters.as_str());
        }
        out
    }

    /// Rejects a record that could not survive the round trip through a file: a description
    /// spanning lines would break the index it is written into, and an empty body is not a memory.
    /// A record that holds an agent token is refused too, wherever the token sits in it.
    pub fn validate(&self) -> Result<(), MemoryError> {
        let one_line = |text: &str| !text.trim().is_empty() && !text.contains('\n');
        if !one_line(&self.description) {
            return Err(MemoryError::InvalidDescription {
                id: self.id.as_str().to_owned(),
            });
        }
        if self.body.trim().is_empty() {
            return Err(MemoryError::EmptyBody {
                id: self.id.as_str().to_owned(),
            });
        }
        // Every field at once, the metadata included: the check must not depend on where a token
        // was put.
        let whole = serde_json::to_vec(self).map_err(|error| MemoryError::Unserializable {
            id: self.id.as_str().to_owned(),
            reason: error.to_string(),
        })?;
        if crate::token::holds_token(&whole) {
            return Err(MemoryError::HoldsToken {
                id: self.id.as_str().to_owned(),
            });
        }
        Ok(())
    }
}
