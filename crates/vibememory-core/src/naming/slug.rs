//! The names that become directories and arguments on both ends: a team slug, a member's handle,
//! a machine, an agent.

/// Longest slug: one DNS label.
pub const MAX_SLUG_LENGTH: usize = 63;

/// `^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$`. The host makes a directory and a git or ssh argument
/// of such a name, and the engine on a member's machine makes a directory of it: a leading hyphen
/// would turn it into an option, and anything beyond the alphabet — a slash, `..` — into a path.
#[must_use]
pub fn is_slug(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=MAX_SLUG_LENGTH).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
}
