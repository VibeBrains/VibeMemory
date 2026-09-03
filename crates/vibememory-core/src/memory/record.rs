//! What a memory is: an identity, a kind, a hook line and a body.
//!
//! Claude Code keeps memory as markdown files, one per fact, with a `MEMORY.md` index. Those
//! files are a projection here — the record is the thing that travels between machines and
//! agents — but the shape follows the format the model already writes, so nothing has to be
//! taught a new one.

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
    /// Human name, shown in the index.
    pub title: String,
    /// One line saying what is inside: the hook the index shows and the model reads to decide
    /// whether the memory is relevant.
    pub description: String,
    /// The memory itself, as markdown.
    pub body: String,
    /// Other records this one points at, in the order they appear in the body.
    pub links: Vec<RecordId>,
    /// Which agent wrote this version — memory is shared between agents, so it has to say.
    pub agent: String,
    /// When the record was first written, in the writer's clock.
    pub created_at: String,
    /// When this version was written, in the writer's clock.
    pub updated_at: String,
}

impl Record {
    /// Rejects a record that could not survive the round trip through a file: a title or a
    /// description spanning lines would break the index they are written into, and an empty body
    /// is not a memory.
    pub fn validate(&self) -> Result<(), MemoryError> {
        let one_line = |text: &str| !text.trim().is_empty() && !text.contains('\n');
        if !one_line(&self.title) {
            return Err(MemoryError::InvalidTitle {
                id: self.id.as_str().to_owned(),
            });
        }
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
        Ok(())
    }
}
