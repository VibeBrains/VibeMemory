//! Why a memory could not be read or written. Every variant carries a stable camelCase code: the
//! fixtures assert on it and the CLI quotes it, so the wording may change but the code may not.

/// A memory that cannot be stored or projected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MemoryError {
    /// The identifier is not a short kebab-case slug.
    #[error("memory id must be lowercase kebab-case, 1 to 64 characters: {id:?}")]
    InvalidId {
        /// The offending identifier.
        id: String,
    },
    /// The kind is not one the format knows.
    #[error("unknown memory kind: {kind:?}")]
    UnknownKind {
        /// The offending word.
        kind: String,
    },
    /// The human name is missing or spans lines.
    #[error("memory {id:?} needs a one-line title")]
    InvalidTitle {
        /// The record.
        id: String,
    },
    /// The one-line summary is missing or spans lines.
    #[error("memory {id:?} needs a one-line description")]
    InvalidDescription {
        /// The record.
        id: String,
    },
    /// The record has no text.
    #[error("memory {id:?} has an empty body")]
    EmptyBody {
        /// The record.
        id: String,
    },
    /// A record could not be written as a journal line.
    #[error("memory {id:?} cannot be written to the journal: {reason}")]
    Unserializable {
        /// The record.
        id: String,
        /// What serde said.
        reason: String,
    },
    /// A projected file has no frontmatter, so nothing says what the memory is.
    #[error("{path}: a memory document starts with a `---` frontmatter block")]
    MissingFrontmatter {
        /// The document.
        path: String,
    },
    /// The frontmatter lacks a field the record cannot do without.
    #[error("{path}: frontmatter has no {field}")]
    MissingField {
        /// The document.
        path: String,
        /// Which field.
        field: &'static str,
    },
}

impl MemoryError {
    /// Stable camelCase code of the variant.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidId { .. } => "invalidId",
            Self::UnknownKind { .. } => "unknownKind",
            Self::InvalidTitle { .. } => "invalidTitle",
            Self::InvalidDescription { .. } => "invalidDescription",
            Self::EmptyBody { .. } => "emptyBody",
            Self::Unserializable { .. } => "unserializable",
            Self::MissingFrontmatter { .. } => "missingFrontmatter",
            Self::MissingField { .. } => "missingField",
        }
    }
}
