//! The shape of the HTTPS token a cabinet issues: `vmt_<id>_<secret>`.
//!
//! Known here, in one place, because two sides need it and must never disagree: the memory server
//! that finds a token by its id, and the engine that must not carry one into the store — a token
//! in a synced file is a token in a git history and in its mirror, for good.

/// Every issued token starts with this.
pub const PREFIX: &str = "vmt_";

/// Characters of a public id: Crockford's base32, lowercase — no `i`, `l`, `o`, `u` to misread.
pub const ID_ALPHABET: &str = "0123456789abcdefghjkmnpqrstvwxyz";

/// Characters of a public id.
pub const ID_LENGTH: usize = 8;

/// Whether `id` is a public id: eight characters of the alphabet.
#[must_use]
pub fn is_id(id: &str) -> bool {
    id.len() == ID_LENGTH && id.chars().all(|character| ID_ALPHABET.contains(character))
}

/// Whether `text` holds anything shaped like an issued token: the prefix, an id, and the `_`
/// before the secret. The legacy token has no shape to recognise — it is 64 hex characters, like
/// any digest — so it is not looked for.
#[must_use]
pub fn holds_token(text: &[u8]) -> bool {
    let prefix = PREFIX.as_bytes();
    text.windows(prefix.len() + ID_LENGTH + 1).any(|window| {
        window.starts_with(prefix)
            && window.last() == Some(&b'_')
            && window
                .get(prefix.len()..prefix.len() + ID_LENGTH)
                .and_then(|id| std::str::from_utf8(id).ok())
                .is_some_and(is_id)
    })
}
