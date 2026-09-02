//! Paths as data. The core is compiled on one platform but reasons about paths written on
//! another (a Windows-created worktree pointer read on macOS, a cwd from another machine's
//! `links.json`), so every path carries its syntax explicitly instead of relying on `std::path`.

use serde::{Deserialize, Serialize};
use typed_path::{
    Utf8TypedComponent, Utf8TypedPath, Utf8UnixComponent, Utf8WindowsComponent, Utf8WindowsPrefix,
};
use unicode_normalization::UnicodeNormalization;

/// Which path syntax a string is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathSyntax {
    /// `/a/b` — macOS, Linux.
    Posix,
    /// `D:\a\b`, `D:/a/b`, `\\server\share\a` — Windows.
    Windows,
}

impl PathSyntax {
    /// Heuristic for strings written by another machine (`gitdir:` pointers, config patterns):
    /// a drive letter, a UNC / verbatim prefix or a leading backslash means Windows, anything
    /// else is POSIX. Local inputs (cwd, `transcript_path`) take the syntax of the host from the
    /// CLI instead.
    #[must_use]
    pub fn derive(path: &str) -> Self {
        if Utf8TypedPath::derive(path).is_windows() {
            Self::Windows
        } else {
            Self::Posix
        }
    }
}

/// A path split into its meaningful parts: an optional Windows prefix, whether it is anchored
/// to a root, and the normal components with `.` and `..` resolved lexically. Components are
/// NFC-normalized: git prints directory names in their on-disk form (NFD on APFS for names
/// created that way) while the CLI reports cwd in NFC, and both name one directory. Trailing
/// and repeated separators carry no meaning.
#[derive(Debug, Clone)]
pub(crate) struct ParsedPath {
    syntax: PathSyntax,
    prefix: Option<String>,
    anchored: bool,
    normals: Vec<String>,
}

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// Simple case folding, character by character: `str::to_lowercase` applies the contextual
/// final-sigma rule, which file systems do not.
fn folded(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

impl ParsedPath {
    pub(crate) fn parse(path: &str, syntax: PathSyntax) -> Self {
        let mut parsed = Self {
            syntax,
            prefix: None,
            anchored: false,
            normals: Vec::new(),
        };
        parsed.push(path);
        parsed
    }

    /// Appends `path` component by component in this path's syntax; a root or a prefix in
    /// `path` replaces what came before, as `PathBuf::push` does.
    fn push(&mut self, path: &str) {
        let typed = match self.syntax {
            PathSyntax::Posix => Utf8TypedPath::unix(path),
            PathSyntax::Windows => Utf8TypedPath::windows(path),
        };
        for component in typed.components() {
            match component {
                Utf8TypedComponent::Unix(c) => match c {
                    Utf8UnixComponent::RootDir => {
                        self.anchored = true;
                        self.normals.clear();
                    }
                    Utf8UnixComponent::CurDir => {}
                    Utf8UnixComponent::ParentDir => {
                        self.normals.pop();
                    }
                    Utf8UnixComponent::Normal(n) => self.normals.push(nfc(n)),
                },
                Utf8TypedComponent::Windows(c) => match c {
                    Utf8WindowsComponent::Prefix(p) => {
                        // A drive letter alone (`D:foo`) is drive-relative; every other prefix
                        // (UNC, verbatim, device) names a root by itself.
                        self.anchored = !matches!(p.kind(), Utf8WindowsPrefix::Disk(_));
                        self.prefix = Some(nfc(&p.as_str().replace('\\', "/")));
                        self.normals.clear();
                    }
                    Utf8WindowsComponent::RootDir => {
                        self.anchored = true;
                        self.normals.clear();
                    }
                    Utf8WindowsComponent::CurDir => {}
                    Utf8WindowsComponent::ParentDir => {
                        self.normals.pop();
                    }
                    Utf8WindowsComponent::Normal(n) => self.normals.push(nfc(n)),
                },
            }
        }
    }

    pub(crate) fn syntax(&self) -> PathSyntax {
        self.syntax
    }

    /// Absolute in its own syntax: has a root, a UNC/verbatim prefix, or both.
    pub(crate) fn is_anchored(&self) -> bool {
        self.anchored
    }

    /// The normal components after lexical resolution, in order.
    pub(crate) fn normals(&self) -> &[String] {
        &self.normals
    }

    pub(crate) fn last_normal(&self) -> Option<&str> {
        self.normals.last().map(String::as_str)
    }

    /// The same path without its last normal component.
    pub(crate) fn parent(&self) -> Self {
        let mut parent = self.clone();
        parent.normals.pop();
        parent
    }

    /// Drops the last `count` normal components.
    pub(crate) fn truncate_normals(&mut self, count: usize) {
        let keep = self.normals.len().saturating_sub(count);
        self.normals.truncate(keep);
    }

    /// Joins `other` lexically; an anchored `other` replaces `self` (its syntax is derived from
    /// the string, as it may have been written on another machine).
    pub(crate) fn join(&self, other: &str) -> Self {
        let candidate = Self::parse(other, PathSyntax::derive(other));
        if candidate.is_anchored() {
            return candidate;
        }
        let mut joined = self.clone();
        joined.push(other);
        joined
    }

    /// Canonical text with `/` separators: `D:/a/b`, `//server/share/a`, `/a/b`; roots keep
    /// their trailing separator (`/`, `D:/`, `//server/share/`), a drive-relative path has none
    /// (`D:foo`). Used for glob matching, comparisons and messages; never fed back to the file
    /// system.
    pub(crate) fn render(&self) -> String {
        let mut out = self.prefix.clone().unwrap_or_default();
        if self.anchored {
            out.push('/');
        }
        out.push_str(&self.normals.join("/"));
        out
    }

    /// Same location in the same syntax; Windows compares without regard to case.
    pub(crate) fn same_location(&self, other: &Self) -> bool {
        if self.syntax != other.syntax || self.anchored != other.anchored {
            return false;
        }
        let eq = |a: &str, b: &str| match self.syntax {
            PathSyntax::Posix => a == b,
            PathSyntax::Windows => folded(a) == folded(b),
        };
        let same_prefix = match (&self.prefix, &other.prefix) {
            (None, None) => true,
            (Some(a), Some(b)) => eq(a, b),
            _ => false,
        };
        same_prefix
            && self.normals.len() == other.normals.len()
            && self
                .normals
                .iter()
                .zip(&other.normals)
                .all(|(a, b)| eq(a, b))
    }

    /// Text used to detect duplicate patterns: rendered, and case-folded for Windows.
    pub(crate) fn identity(&self) -> String {
        match self.syntax {
            PathSyntax::Posix => self.render(),
            PathSyntax::Windows => folded(&self.render()),
        }
    }
}
