//! Hand-offs: the notes an agent leaves in `memory/sessions/` for whoever continues a flow.
//!
//! The rule is the engine's, repeated here because this server reads the files itself: only
//! `*.md`, only the frontmatter, and only a line reading `status: open` in it — the same words
//! deep in the prose are prose. The engine reads them to tell a session what is in progress; this
//! module reads the same files to hand the whole note to an agent that can act on it.

use std::path::Path;

/// The line the frontmatter of an open hand-off carries, exactly as the engine tests it: trimmed,
/// so `  status: open` under `metadata:` counts and a line in the body does not.
const OPEN: &str = "status: open";

/// The fence a frontmatter is written between.
const FENCE: &str = "---";

/// One hand-off whose frontmatter says it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    /// The file's stem, which is the name the engine reports an open hand-off by.
    pub name: String,
    /// `description` of the frontmatter, its quotes taken off; `None` when it carries none.
    pub description: Option<String>,
    /// `status` of the frontmatter, as written.
    pub status: Option<String>,
    /// `updated` of the frontmatter, as written.
    pub updated: Option<String>,
    /// The file's size in bytes.
    pub size: u64,
    /// Everything the file holds after its frontmatter.
    pub body: String,
}

/// The open hand-offs of a `memory/sessions` directory, by name.
///
/// A directory that is not there is no hand-offs rather than an error, and a file that cannot be
/// read is passed over: the engine reads them the same way, and one unreadable note must not hide
/// the rest of them.
#[must_use]
pub fn open_in(sessions: &Path) -> Vec<Handoff> {
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return Vec::new();
    };
    let mut open = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some((front, body)) = split(&text) else {
            continue;
        };
        if !front.lines().any(|line| line.trim() == OPEN) {
            continue;
        }
        let Some(name) = path.file_stem() else {
            continue;
        };
        open.push(Handoff {
            name: name.to_string_lossy().into_owned(),
            description: field(front, "description"),
            status: field(front, "status"),
            updated: field(front, "updated"),
            size: entry.metadata().map_or(0, |data| data.len()),
            body: body.trim().to_owned(),
        });
    }
    // Sorted here rather than trusted to the filesystem: the engine sorts for the same reason —
    // ext4 hands names back in hash order, and one store read on two machines would otherwise
    // answer differently.
    open.sort_by(|left, right| left.name.cmp(&right.name));
    open
}

/// The frontmatter of a note and the body after it: what lies between its first two `---`, split
/// as the engine splits it. A file without both fences is no hand-off.
fn split(text: &str) -> Option<(&str, &str)> {
    let first = text.find(FENCE)? + FENCE.len();
    let second = text.get(first..)?.find(FENCE)? + first;
    Some((text.get(first..second)?, text.get(second + FENCE.len()..)?))
}

/// One frontmatter field: the first line naming it, its value trimmed and its quotes taken off.
/// Nesting is not read — the fields an agent writes here are flat, and only the marker decides
/// whether the note is open at all.
fn field(front: &str, name: &str) -> Option<String> {
    front.lines().find_map(|line| {
        let value = line.trim().strip_prefix(name)?.strip_prefix(':')?.trim();
        (!value.is_empty()).then(|| value.trim_matches('"').to_owned())
    })
}
