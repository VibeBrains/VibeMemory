//! One glob dialect for `ignoreCwd` and `nameOverrides`: a literal path matches exactly, a
//! subtree needs an explicit `/**`. Patterns use `/` only and are canonicalized like working
//! directories (NFC, `.`/`..` resolved, no trailing separator except on roots). Case follows the
//! file system: a pattern in Windows syntax never minds it, a POSIX one minds it only where the
//! host's file system does. A subtree pattern `dir/**` takes `dir` itself too.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::error::NamingError;
use super::path::{ParsedPath, PathSyntax};

/// Characters that make a path component a glob rather than a literal.
const GLOB_META: &[char] = &['*', '?', '[', ']', '{', '}'];

/// The tail that makes a pattern a subtree.
const SUBTREE: &str = "/**";

/// Whether letter case tells two paths apart, as the file system the paths live on decides.
///
/// APFS and NTFS keep the case a name was written in but find the file under any case, so a
/// pattern typed in another case names the same directory and has to match it. ext4 and most
/// Linux file systems tell `Work` from `work`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaseRule {
    /// `Work` and `work` are two directories.
    Sensitive,
    /// `Work` and `work` are one directory.
    Insensitive,
}

impl CaseRule {
    /// The rule of the file systems this build runs on: insensitive on macOS and Windows.
    pub const HOST: Self = if cfg!(any(target_os = "macos", windows)) {
        Self::Insensitive
    } else {
        Self::Sensitive
    };
}

/// A compiled, validated list of patterns in configuration order, each carrying a value.
#[derive(Debug, Clone)]
pub(crate) struct PathRules<T> {
    set: GlobSet,
    /// The entry each glob of `set` belongs to: a subtree pattern compiles to two globs.
    owners: Vec<usize>,
    entries: Vec<(String, T)>,
}

impl<T> Default for PathRules<T> {
    fn default() -> Self {
        Self {
            set: GlobSet::empty(),
            owners: Vec::new(),
            entries: Vec::new(),
        }
    }
}

impl<T> PathRules<T> {
    /// Validates and compiles `(pattern, value)` pairs; the first problem is the error.
    pub(crate) fn compile(
        entries: Vec<(String, T)>,
        field: &'static str,
        case: CaseRule,
    ) -> Result<Self, NamingError> {
        let invalid = |reason: String| NamingError::ConfigInvalid { field, reason };
        let mut builder = GlobSetBuilder::new();
        let mut owners = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (index, (pattern, _)) in entries.iter().enumerate() {
            if pattern.trim().is_empty() {
                return Err(invalid("empty pattern".to_owned()));
            }
            if pattern.contains('\\') {
                return Err(invalid(format!(
                    "pattern {pattern:?} contains a backslash; write paths with `/` only"
                )));
            }
            let syntax = PathSyntax::derive(pattern);
            let parsed = ParsedPath::parse(pattern, syntax);
            if !parsed.is_anchored() {
                return Err(invalid(format!(
                    "pattern {pattern:?} is not an absolute path (`/…`, `D:/…` or `//server/share/…`)"
                )));
            }
            let identity = parsed.identity();
            if seen.contains(&identity) {
                return Err(invalid(format!("pattern {pattern:?} is listed twice")));
            }
            seen.push(identity);
            let insensitive = syntax == PathSyntax::Windows || case == CaseRule::Insensitive;
            let rendered = parsed.render();
            // `dir/**` is the directory and everything under it: a session in the project's root
            // is the commonest case, and the glob alone takes only what lies below
            let root = rendered
                .strip_suffix(SUBTREE)
                .filter(|root| !root.is_empty() && !root.ends_with('/'));
            for text in std::iter::once(rendered.as_str()).chain(root) {
                let glob = GlobBuilder::new(text)
                    .literal_separator(true)
                    .backslash_escape(false)
                    .case_insensitive(insensitive)
                    .build()
                    .map_err(|e| invalid(format!("pattern {pattern:?}: {e}")))?;
                builder.add(glob);
                owners.push(index);
            }
        }
        let set = builder.build().map_err(|e| invalid(e.to_string()))?;
        Ok(Self {
            set,
            owners,
            entries,
        })
    }

    /// The matching `(pattern, value)` pairs, in configuration order.
    pub(crate) fn matches<'a>(
        &'a self,
        rendered_path: &str,
    ) -> impl Iterator<Item = (&'a str, &'a T)> {
        let mut hit: Vec<usize> = self
            .set
            .matches(rendered_path)
            .into_iter()
            .filter_map(|glob| self.owners.get(glob).copied())
            .collect();
        hit.dedup();
        hit.into_iter()
            .filter_map(move |i| self.entries.get(i).map(|(p, v)| (p.as_str(), v)))
    }

    /// The patterns with their values, in configuration order.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&str, &T)> {
        self.entries
            .iter()
            .map(|(pattern, value)| (pattern.as_str(), value))
    }

    /// The part of a pattern before its first glob component: the directory it is anchored at.
    pub(crate) fn literal_prefix(pattern: &str) -> String {
        let parsed = ParsedPath::parse(pattern, PathSyntax::derive(pattern));
        let rendered = parsed.render();
        let normals = parsed.normals();
        let Some(first_glob) = normals.iter().position(|c| c.contains(GLOB_META)) else {
            return rendered;
        };
        // the rendered form ends with the normals joined by `/`: cut it before the first glob one
        let tail: usize = normals
            .iter()
            .skip(first_glob)
            .map(|component| component.len() + 1)
            .sum();
        rendered
            .get(..rendered.len().saturating_sub(tail))
            .filter(|prefix| !prefix.is_empty())
            .unwrap_or("/")
            .to_owned()
    }

    /// `true` when at least one component of the pattern is a plain literal, so the pattern
    /// cannot capture a whole volume (`/`, `/**`, `D:/**`, `//server/share/**`, `/./**`).
    pub(crate) fn has_literal_component(pattern: &str) -> bool {
        ParsedPath::parse(pattern, PathSyntax::derive(pattern))
            .normals()
            .iter()
            .any(|c| !c.contains(GLOB_META))
    }
}
