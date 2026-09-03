//! Translating a working directory between machines.
//!
//! A descriptor written on Windows says `D:\Projects\VibeCode\VibeIDE`; the same project on the
//! Mac is `/Volumes/Storage/Projects/VibeCode/VibeIDE`. Desktop itself does not translate — the
//! store holds 23 descriptors with `D:\…` and the Mac simply reports `folderExists: false` for
//! them — so the engine translates on the way out and on the way in, through named roots the
//! owner declares in `config.json`.
//!
//! A path outside every root is not translated and therefore does not leave the machine: a root
//! is the owner saying "this directory means the same thing on my other machine", and without
//! that statement no rewriting is honest.

use std::collections::BTreeMap;

use crate::naming::path::{ParsedPath, PathSyntax};

/// The placeholder that opens a portable path: `{PROJECTS}/VibeCode/VibeIDE`.
const OPEN: char = '{';
/// And closes it.
const CLOSE: char = '}';

/// The roots of one machine: name → absolute local path, from `config.json`.
#[derive(Debug, Clone)]
pub struct Roots {
    entries: BTreeMap<String, String>,
    syntax: PathSyntax,
}

/// Why a path could not be translated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootError {
    /// No declared root contains it: the owner has not said what it means elsewhere.
    Outside {
        /// The path, as it was given.
        path: String,
    },
    /// The portable form names a root this machine does not declare.
    UnknownRoot {
        /// The name inside the braces.
        name: String,
    },
    /// The text does not open with `{NAME}`.
    NotPortable {
        /// The text, as it was given.
        text: String,
    },
}

impl Roots {
    /// Builds the roots of this machine. `syntax` is the syntax of the host, not derived from the
    /// strings: an empty root list still has to render local paths in the host's form.
    #[must_use]
    pub fn new(entries: BTreeMap<String, String>, syntax: PathSyntax) -> Self {
        Self { entries, syntax }
    }

    /// Rewrites a local path as `{NAME}/rel`.
    ///
    /// The longest matching root wins, so nested roots (`/Users/x/Projects` inside `/Users/x`)
    /// name the most specific meaning rather than an arbitrary one. Ties on length are broken by
    /// name so that two machines never disagree about which of two identical roots was used.
    ///
    /// # Errors
    ///
    /// [`RootError::Outside`] when no root contains the path.
    pub fn to_portable(&self, path: &str) -> Result<String, RootError> {
        let parsed = ParsedPath::parse(path, PathSyntax::derive(path));
        let mut best: Option<(&str, usize)> = None;
        for (name, root) in &self.entries {
            let root = ParsedPath::parse(root, PathSyntax::derive(root));
            let Some(depth) = contains(&root, &parsed) else {
                continue;
            };
            if best.is_none_or(|(_, best_depth)| depth > best_depth) {
                best = Some((name.as_str(), depth));
            }
        }
        let Some((name, depth)) = best else {
            return Err(RootError::Outside {
                path: path.to_owned(),
            });
        };
        let rest = parsed.normals().get(depth..).unwrap_or_default().join("/");
        let mut portable = format!("{OPEN}{name}{CLOSE}");
        if !rest.is_empty() {
            portable.push('/');
            portable.push_str(&rest);
        }
        Ok(portable)
    }

    /// Rewrites `{NAME}/rel` as a path of this machine, in the host's syntax.
    ///
    /// # Errors
    ///
    /// [`RootError::NotPortable`] when the text does not open with a root name;
    /// [`RootError::UnknownRoot`] when this machine does not declare that root.
    pub fn to_local(&self, portable: &str) -> Result<String, RootError> {
        let not_portable = || RootError::NotPortable {
            text: portable.to_owned(),
        };
        let rest = portable.strip_prefix(OPEN).ok_or_else(not_portable)?;
        let (name, rest) = rest.split_once(CLOSE).ok_or_else(not_portable)?;
        if name.is_empty() {
            return Err(not_portable());
        }
        let root = self
            .entries
            .get(name)
            .ok_or_else(|| RootError::UnknownRoot {
                name: name.to_owned(),
            })?;
        let mut local = ParsedPath::parse(root, self.syntax);
        for component in rest.split(['/', '\\']).filter(|part| !part.is_empty()) {
            local = local.join(component);
        }
        Ok(local.render())
    }
}

/// How many leading components of `path` the root occupies, or `None` when the root is not a
/// prefix of it. A path equal to the root translates to the bare `{NAME}`.
fn contains(root: &ParsedPath, path: &ParsedPath) -> Option<usize> {
    if !root.is_anchored() || !path.is_anchored() {
        return None;
    }
    let depth = root.normals().len();
    if path.normals().len() < depth {
        return None;
    }
    let mut head = path.clone();
    head.truncate_normals(path.normals().len() - depth);
    root.same_location(&head).then_some(depth)
}
