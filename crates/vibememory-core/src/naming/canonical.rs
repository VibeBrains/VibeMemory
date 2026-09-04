//! One canonical form of a working directory, computed once and before anything else.
//!
//! The same directory reaches the engine written several ways: the CLI reports cwd, a hook reads
//! it from stdin, git prints it from `--path-format`, and on Windows the shell may hand over a
//! verbatim path. If each of them is canonicalized at its own call site, the encoded directory
//! name, the record in `links.json` and the store name eventually disagree — and a disagreement
//! there means a second link to the same project, or none.
//!
//! The rule is therefore: canonicalize once, at the edge, then pass the result everywhere. This
//! function is the lexical half of it — the half that has no file system in it. The caller does
//! the other half first: `realpath` on macOS; on Windows nothing at all, because the CLI keys its
//! directories by Node's `fs.realpathSync`, which leaves junctions and `subst` drives as they
//! are. Resolving them here would produce a name the CLI never writes.

use std::borrow::Cow;

use super::PathSyntax;
use super::path::ParsedPath;

/// The Windows verbatim prefix, which disables path parsing in the OS.
const VERBATIM: &str = r"\\?\";
/// Its UNC form: `\\?\UNC\server\share` is `\\server\share`.
const VERBATIM_UNC: &str = r"\\?\UNC\";

/// Canonical text of a working directory: `/` separators, NFC, `.` and `..` resolved, no
/// trailing separator, no verbatim prefix.
///
/// `syntax` is the syntax of the machine the path came from — passed in, never guessed, because a
/// POSIX path and a drive-relative Windows path can look alike.
///
/// Verbatim prefixes are stripped rather than kept: `\\?\D:\Projects` and `D:\Projects` are one
/// directory, but as text they produce different encoded names and fail every comparison against
/// what the CLI wrote.
#[must_use]
pub fn canonical_cwd(raw: &str, syntax: PathSyntax) -> String {
    let stripped = match syntax {
        PathSyntax::Posix => Cow::Borrowed(raw),
        PathSyntax::Windows => strip_verbatim(raw),
    };
    ParsedPath::parse(&stripped, syntax).render()
}

/// `\\?\UNC\server\share` → `\\server\share`; `\\?\D:\a` → `D:\a`; anything else unchanged.
fn strip_verbatim(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix(VERBATIM_UNC) {
        // The UNC form loses its two leading backslashes together with the prefix; without them
        // the parser would read `server` as an ordinary component and the share would vanish.
        return Cow::Owned(format!(r"\\{rest}"));
    }
    Cow::Borrowed(path.strip_prefix(VERBATIM).unwrap_or(path))
}
