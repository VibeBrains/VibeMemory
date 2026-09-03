//! The name of a project store: one directory under `<store>/projects/` that must be valid on
//! macOS, on Windows (NTFS) and inside a git tree checked out on both.

use serde::{Deserialize, Serialize};
use typed_path::constants::windows::{DISALLOWED_FILENAME_CHARS, RESERVED_DEVICE_NAMES_STR};
use unicode_normalization::UnicodeNormalization;

use super::error::NamingError;
use crate::MAX_FILE_NAME_BYTES;

/// Name of the git directory; a store named like this would break the store repository.
pub const GIT_DIR_NAME: &str = ".git";
/// Reserved device names Windows documents in addition to the ASCII ones typed-path ships
/// (`COM¹`…`LPT³`, superscript digits). Not verified on a Windows machine.
const RESERVED_DEVICE_NAMES_SUPERSCRIPT: &[&str] = &[
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// A validated store directory name. Compared byte-for-byte; use [`StoreName::key`] for identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StoreName(String);

impl StoreName {
    /// Validates one path component for macOS, Windows and git.
    pub fn parse(name: &str) -> Result<Self, NamingError> {
        match Self::violation(name) {
            None => Ok(Self(name.to_owned())),
            Some(reason) => Err(NamingError::InvalidStoreName {
                name: name.to_owned(),
                reason,
            }),
        }
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Identity key: names equal under NFC and simple case folding are one store, because git
    /// keeps them apart in the tree while APFS and NTFS put them into one directory. Folding is
    /// per character: `str::to_lowercase` applies the contextual final-sigma rule, file systems
    /// do not.
    #[must_use]
    pub fn key(&self) -> String {
        self.0.nfc().flat_map(char::to_lowercase).collect()
    }

    fn violation(name: &str) -> Option<&'static str> {
        if name.is_empty() {
            return Some("empty");
        }
        if name.len() > MAX_FILE_NAME_BYTES {
            return Some("longer than the file-name limit of APFS and NTFS");
        }
        if name == "." || name == ".." {
            return Some("`.` and `..` are not names");
        }
        if name.eq_ignore_ascii_case(GIT_DIR_NAME) {
            return Some("`.git` would break the store repository");
        }
        if name.chars().any(|c| DISALLOWED_FILENAME_CHARS.contains(&c)) {
            return Some("contains a character Windows forbids in file names");
        }
        if name.chars().any(char::is_control) {
            return Some("contains a control character");
        }
        if name.ends_with('.') || name.ends_with(' ') {
            return Some("ends with a dot or a space");
        }
        if Self::is_reserved_device_name(name) {
            return Some("is a reserved device name on Windows");
        }
        None
    }

    /// Windows reserves `CON`, `AUX.txt`, `com1 .x`…: the stem before the first dot, case-insensitive.
    fn is_reserved_device_name(name: &str) -> bool {
        let stem = name.split('.').next().unwrap_or(name).trim_end();
        let upper = stem.to_uppercase();
        RESERVED_DEVICE_NAMES_STR
            .iter()
            .chain(RESERVED_DEVICE_NAMES_SUPERSCRIPT)
            .any(|reserved| upper == *reserved)
    }
}

impl TryFrom<String> for StoreName {
    type Error = NamingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<StoreName> for String {
    fn from(value: StoreName) -> Self {
        value.0
    }
}
