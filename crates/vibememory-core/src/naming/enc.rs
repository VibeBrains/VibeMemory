//! The project directory slug (`enc`) of Claude Code: `<config dir>/projects/<enc>/<sid>.jsonl`.
//!
//! The authoritative slug of a running session is read from the hook's `transcript_path`
//! ([`enc_from_transcript_path`]). [`encode_cwd`] reproduces the CLI formula for the one place
//! that has no transcript yet (the tick reconciler pre-creating links); it is a prediction, never
//! a decision.

use serde::{Deserialize, Serialize};
use typed_path::constants::windows::RESERVED_DEVICE_NAMES_STR;
use unicode_normalization::UnicodeNormalization;

use super::error::NamingError;
use super::path::{ParsedPath, PathSyntax};

/// Directory under the config dir that holds one directory per project.
pub const PROJECTS_DIR_NAME: &str = "projects";
/// Extension of a session transcript.
pub const TRANSCRIPT_EXTENSION: &str = "jsonl";
/// Slugs longer than this are truncated and suffixed with a hash (CLI 2.1.224+).
const ENC_MAX_LEN: usize = 200;
/// Every UTF-16 unit outside `[A-Za-z0-9]` becomes this character.
const SLUG_REPLACEMENT: char = '-';
/// Radix of the hash suffix (`Number.prototype.toString(36)`).
const SLUG_HASH_RADIX: u32 = 36;
/// Longest value of `CLAUDE_CODE_PROJECT_DIR_NAME` the CLI accepts (2.1.255).
const PROJECT_DIR_NAME_MAX_LEN: usize = 64;

/// The alphabet of a project directory name: the formula yields `[A-Za-z0-9-]` and
/// `CLAUDE_CODE_PROJECT_DIR_NAME` adds `_`.
fn is_slug_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// A project directory name exactly as the CLI computed it. Opaque: never parsed back into a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EncSlug(String);

impl EncSlug {
    /// Accepts exactly what the CLI can produce: one or more of `[A-Za-z0-9_-]`.
    pub fn parse(slug: &str) -> Result<Self, NamingError> {
        if !slug.is_empty() && slug.chars().all(is_slug_char) {
            Ok(Self(slug.to_owned()))
        } else {
            Err(NamingError::InvalidEncSlug {
                slug: slug.to_owned(),
            })
        }
    }

    /// The slug as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EncSlug {
    type Error = NamingError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<EncSlug> for String {
    fn from(value: EncSlug) -> Self {
        value.0
    }
}

/// The authoritative slug of a running session: `basename(dirname(transcript_path))`.
///
/// Validates the layout `<…>/projects/<enc>/<sid>.jsonl` in the given path syntax. Never
/// normalizes and never recomputes: the link must be created under the slug this CLI computed.
pub fn enc_from_transcript_path(
    transcript_path: &str,
    syntax: PathSyntax,
) -> Result<EncSlug, NamingError> {
    let malformed = || NamingError::MalformedTranscriptPath {
        path: transcript_path.to_owned(),
    };
    let path = ParsedPath::parse(transcript_path, syntax);
    if !path.is_anchored() {
        return Err(malformed());
    }
    let [.., projects, enc, file] = path.normals() else {
        return Err(malformed());
    };
    let stem = file
        .strip_suffix(TRANSCRIPT_EXTENSION)
        .and_then(|s| s.strip_suffix('.'));
    if projects != PROJECTS_DIR_NAME || stem.is_none_or(str::is_empty) {
        return Err(malformed());
    }
    EncSlug::parse(enc)
}

/// The value of `CLAUDE_CODE_PROJECT_DIR_NAME` the CLI actually uses (2.1.255): one to 64 of
/// `[A-Za-z0-9_-]` and not a reserved device name; anything else is silently ignored by the
/// CLI, which then uses the formula, so the engine ignores it too.
pub(crate) fn effective_project_dir_name(raw: &str) -> Option<&str> {
    let well_formed =
        (1..=PROJECT_DIR_NAME_MAX_LEN).contains(&raw.len()) && raw.chars().all(is_slug_char);
    let reserved = RESERVED_DEVICE_NAMES_STR
        .iter()
        .any(|device| raw.eq_ignore_ascii_case(device));
    (well_formed && !reserved).then_some(raw)
}

/// The CLI formula (2.1.232 / 2.1.255, transcribed from the binaries): NFC, then every UTF-16
/// unit outside `[A-Za-z0-9]` becomes `-`; a slug longer than 200 characters is cut to 200 and
/// suffixed with `-` and the base-36 magnitude of the JavaScript string hash of the full NFC
/// string.
///
/// The CLI feeds `realpath(cwd)` into this; resolving symlinks is I/O and stays with the caller.
/// A verbatim Windows path (`\\?\D:\…`) must never reach this function: its prefix would become
/// part of the slug.
pub fn encode_cwd(cwd: &str) -> Result<EncSlug, NamingError> {
    let nfc: String = cwd.nfc().collect();
    let units: Vec<u16> = nfc.encode_utf16().collect();
    let slug: String = units
        .iter()
        .map(|&unit| match char::from_u32(u32::from(unit)) {
            Some(c) if c.is_ascii_alphanumeric() => c,
            _ => SLUG_REPLACEMENT,
        })
        .collect();
    // The slug is pure ASCII, so its char count equals its UTF-16 length in the CLI.
    if slug.len() <= ENC_MAX_LEN {
        return EncSlug::parse(&slug);
    }
    let head: String = slug.chars().take(ENC_MAX_LEN).collect();
    let suffix = base36(js_string_hash(&units).unsigned_abs());
    EncSlug::parse(&format!("{head}{SLUG_REPLACEMENT}{suffix}"))
}

/// JavaScript `t = (t << 5) - t + charCodeAt(i) | 0` over UTF-16 units, as wrapping i32 arithmetic.
pub(crate) fn js_string_hash(units: &[u16]) -> i32 {
    units.iter().fold(0_i32, |t, &unit| {
        t.wrapping_shl(5)
            .wrapping_sub(t)
            .wrapping_add(i32::from(unit))
    })
}

/// JavaScript `Number.prototype.toString(36)` for a non-negative integer (lowercase digits).
pub(crate) fn base36(mut n: u32) -> String {
    if n == 0 {
        return "0".to_owned();
    }
    let mut digits = Vec::new();
    while n > 0 {
        digits.extend(char::from_digit(n % SLUG_HASH_RADIX, SLUG_HASH_RADIX));
        n /= SLUG_HASH_RADIX;
    }
    digits.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_wraps_like_javascript_int32() {
        // 201 × 'x' after '/' — the CLI computes -786901713 (node 24, 2026-09-02).
        let s = format!("/{}", "x".repeat(200));
        let units: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(js_string_hash(&units), -786_901_713);
        assert_eq!(base36(js_string_hash(&units).unsigned_abs()), "d0i18x");
    }

    #[test]
    fn magnitude_of_min_int_does_not_overflow() {
        // JavaScript: Math.abs(-2147483648).toString(36) === "zik0zk".
        assert_eq!(base36(i32::MIN.unsigned_abs()), "zik0zk");
    }

    #[test]
    fn base36_zero() {
        assert_eq!(base36(0), "0");
    }
}
