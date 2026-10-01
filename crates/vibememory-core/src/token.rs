//! The shape of the HTTPS token a cabinet issues: `vmt_<id>_<secret>`.
//!
//! Known here, in one place, because two sides need it and must never disagree: the memory server
//! that finds a token by its id, and the engine that must not carry one into the store — a token
//! in a synced file is a token in a git history and in its mirror, for good.

use std::collections::BTreeSet;

/// Every issued token starts with this.
pub const PREFIX: &str = "vmt_";

/// Characters of a public id: Crockford's base32, lowercase — no `i`, `l`, `o`, `u` to misread.
pub const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// Characters of a public id.
pub const ID_LENGTH: usize = 8;

/// How the cabinet and the host name a token publicly: this prefix and its id.
pub const PUBLIC_ID_PREFIX: &str = "tk_";

/// The bytes that tell an issued token: the prefix, the id and the `_` before the secret. A scan
/// that reads a text in parts keeps this many bytes less one from each part in front of the next.
pub const SHAPE_BYTES: usize = PREFIX.len() + ID_LENGTH + 1;

/// Whether `id` is a public id: eight characters of the alphabet.
#[must_use]
pub fn is_id(id: &str) -> bool {
    id.len() == ID_LENGTH && id.chars().all(|character| ID_ALPHABET.contains(character))
}

/// The ids of the issued tokens in `text`, in the order they appear: wherever the prefix, an id and
/// the `_` before the secret stand together. The legacy token has no shape to recognise — it is 64
/// hex characters, like any digest — so it is not looked for.
fn issued_ids(text: &[u8]) -> impl Iterator<Item = &str> {
    let prefix = PREFIX.as_bytes();
    text.windows(SHAPE_BYTES).filter_map(move |window| {
        if !window.starts_with(prefix) || window.last() != Some(&b'_') {
            return None;
        }
        window
            .get(prefix.len()..prefix.len() + ID_LENGTH)
            .and_then(|id| std::str::from_utf8(id).ok())
            .filter(|id| is_id(id))
    })
}

/// Whether `text` holds anything shaped like an issued token.
#[must_use]
pub fn holds_token(text: &[u8]) -> bool {
    issued_ids(text).next().is_some()
}

/// The public ids (`tk_<id>`) of the issued tokens `text` holds: what finds each in the cabinet to
/// revoke it, and nothing that opens anything.
#[must_use]
pub fn token_ids(text: &[u8]) -> BTreeSet<String> {
    issued_ids(text)
        .map(|id| format!("{PUBLIC_ID_PREFIX}{id}"))
        .collect()
}

/// What stands in a text in place of an issued token's secret after [`redact`].
pub const REDACTED: &str = "<token>";

/// `text` with every issued token cut out: the prefix, the id, the `_` and the secret after it give
/// way to [`REDACTED`]. The secret runs to the first whitespace, quote or backslash — where it ends
/// in a line of JSON, in a shell command and in prose. The same text always gives the same result,
/// so a log converted twice makes no change to commit.
#[must_use]
pub fn redact(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while let Some(found) = text.get(at..).and_then(|rest| rest.find(PREFIX)) {
        let start = at + found;
        let shape = bytes.get(start..start + SHAPE_BYTES);
        let issued = shape.is_some_and(|shape| {
            shape.last() == Some(&b'_')
                && shape
                    .get(PREFIX.len()..PREFIX.len() + ID_LENGTH)
                    .and_then(|id| std::str::from_utf8(id).ok())
                    .is_some_and(is_id)
        });
        if !issued {
            at = start + PREFIX.len();
            continue;
        }
        let secret = start + SHAPE_BYTES;
        let end = bytes
            .get(secret..)
            .and_then(|rest| {
                rest.iter().position(|byte| {
                    byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'\\')
                })
            })
            .map_or(bytes.len(), |length| secret + length);
        out.push_str(text.get(copied..start).unwrap_or_default());
        out.push_str(REDACTED);
        copied = end;
        at = end;
    }
    out.push_str(text.get(copied..).unwrap_or_default());
    out
}
