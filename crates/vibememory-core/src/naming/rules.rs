//! One glob dialect for `ignoreCwd` and `nameOverrides`: a literal path matches exactly, a
//! subtree needs an explicit `/**`. Patterns use `/` only and are canonicalized like working
//! directories (NFC, `.`/`..` resolved, no trailing separator except on roots); a pattern in
//! Windows syntax matches without regard to case.

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use super::error::NamingError;
use super::path::{ParsedPath, PathSyntax};

/// Characters that make a path component a glob rather than a literal.
const GLOB_META: &[char] = &['*', '?', '[', ']', '{', '}'];

/// A compiled, validated list of patterns in configuration order, each carrying a value.
#[derive(Debug, Clone)]
pub(crate) struct PathRules<T> {
    set: GlobSet,
    entries: Vec<(String, T)>,
}

impl<T> Default for PathRules<T> {
    fn default() -> Self {
        Self {
            set: GlobSet::empty(),
            entries: Vec::new(),
        }
    }
}

impl<T> PathRules<T> {
    /// Validates and compiles `(pattern, value)` pairs; the first problem is the error.
    pub(crate) fn compile(
        entries: Vec<(String, T)>,
        field: &'static str,
    ) -> Result<Self, NamingError> {
        let invalid = |reason: String| NamingError::ConfigInvalid { field, reason };
        let mut builder = GlobSetBuilder::new();
        let mut seen: Vec<String> = Vec::new();
        for (pattern, _) in &entries {
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
            let glob = GlobBuilder::new(&parsed.render())
                .literal_separator(true)
                .backslash_escape(false)
                .case_insensitive(syntax == PathSyntax::Windows)
                .build()
                .map_err(|e| invalid(format!("pattern {pattern:?}: {e}")))?;
            builder.add(glob);
        }
        let set = builder.build().map_err(|e| invalid(e.to_string()))?;
        Ok(Self { set, entries })
    }

    /// The matching `(pattern, value)` pairs, in configuration order.
    pub(crate) fn matches<'a>(
        &'a self,
        rendered_path: &str,
    ) -> impl Iterator<Item = (&'a str, &'a T)> {
        self.set
            .matches(rendered_path)
            .into_iter()
            .filter_map(move |i| self.entries.get(i).map(|(p, v)| (p.as_str(), v)))
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
