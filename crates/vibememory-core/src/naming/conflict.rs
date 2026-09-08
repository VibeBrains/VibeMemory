//! Conflict copies: the second file a cloud client leaves when two machines wrote one name.
//!
//! `OneDrive` names them `<stem>-<MACHINE>.<ext>`, and the machine name is the only thing that
//! separates a copy from a file whose own name happens to contain a dash — which most of them do,
//! since every id here is a UUID. Nothing in the name says where the id ends, so the split is a
//! guess with two guards: the suffix must carry an upper-case letter (`GPD-WIN-MAX2`,
//! `MacMini` — hostnames as the clients write them), and the caller must find the original beside
//! it. Only the first guard belongs here; the second one needs a disk and stays with the caller.

/// A file name split into the original it copies and the machine that made the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictCopy {
    /// The name the original would have, extension included.
    pub original: String,
    /// The machine name carried in the suffix.
    pub machine: String,
}

/// Every way `name` could be read as a conflict copy, longest original first.
///
/// More than one split can look plausible — `local_02ea2773-529f-4432-8fd1-605dcd6197cd-MacMini`
/// splits at any of its dashes — so the answer is an ordered list of candidates rather than one
/// verdict. Longest original first, because the id is what precedes the suffix and a UUID is
/// longer than any of its prefixes: the caller takes the first candidate whose original it can
/// actually find, and a caller with no disk to consult has no business deciding at all.
///
/// A name with no upper-case letter after any dash yields nothing: `part-two.json` is a file,
/// not a copy.
#[must_use]
pub fn conflict_copies(name: &str) -> Vec<ConflictCopy> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, Some(ext)),
        _ => (name, None),
    };
    let mut found = Vec::new();
    for (index, _) in stem.match_indices('-') {
        let base = &stem[..index];
        let suffix = &stem[index + 1..];
        // An empty base names no file, and a suffix without an upper-case letter is part of the
        // id rather than a hostname: cloud clients write the machine name as the machine has it.
        if base.is_empty() || !suffix.chars().any(|c| c.is_ascii_uppercase()) {
            continue;
        }
        found.push(ConflictCopy {
            original: extension.map_or_else(|| base.to_owned(), |ext| format!("{base}.{ext}")),
            machine: suffix.to_owned(),
        });
    }
    // Longest original first: `…-605dcd6197cd` before `…-4432`, so the caller tries the reading
    // that keeps the whole id before the ones that cut into it.
    found.reverse();
    found
}
