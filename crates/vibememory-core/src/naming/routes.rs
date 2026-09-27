//! Which store a working directory belongs to: the personal store, or the store of a team whose
//! `stores.<id>.cwd` patterns match it. The patterns are the dialect of `nameOverrides` and
//! `ignoreCwd` — a literal path matches exactly, a subtree needs `/**` — so one rule of reading
//! paths holds for the whole configuration.

use super::error::NamingError;
use super::path::{ParsedPath, PathSyntax};
use super::rules::{CaseRule, PathRules};
use super::slug::is_slug;

/// The configuration field the patterns come from, as `configInvalid` names it.
const STORES_CWD: &str = "stores.cwd";

/// Compiled `stores.<id>.cwd` patterns, each carrying its store's id.
#[derive(Debug, Clone, Default)]
pub struct StoreRoutes {
    rules: PathRules<String>,
}

impl StoreRoutes {
    /// Validates and compiles `(store id, patterns)` pairs.
    ///
    /// # Errors
    ///
    /// `configInvalid` for a store id that is not a slug — it names a directory on disk and a
    /// repository on the host — or for a pattern `nameOverrides` would refuse too.
    pub fn compile(stores: &[(String, Vec<String>)]) -> Result<Self, NamingError> {
        Self::compile_with(stores, CaseRule::HOST)
    }

    /// [`Self::compile`] for paths of a file system with the given case rule.
    ///
    /// # Errors
    ///
    /// As [`Self::compile`].
    pub fn compile_with(
        stores: &[(String, Vec<String>)],
        case: CaseRule,
    ) -> Result<Self, NamingError> {
        let mut entries = Vec::new();
        for (id, patterns) in stores {
            if !is_slug(id) {
                return Err(NamingError::ConfigInvalid {
                    field: STORES_CWD,
                    reason: format!("store id {id:?} is not a slug"),
                });
            }
            entries.extend(patterns.iter().map(|pattern| (pattern.clone(), id.clone())));
        }
        Ok(Self {
            rules: PathRules::compile(entries, STORES_CWD, case)?,
        })
    }

    /// Every pattern with the store it routes to, in configuration order, and the directory each
    /// is anchored at — what a check against the disk looks at.
    pub fn patterns(&self) -> impl Iterator<Item = (&str, &str, String)> {
        self.rules.entries().map(|(pattern, id)| {
            (
                pattern,
                id.as_str(),
                PathRules::<String>::literal_prefix(pattern),
            )
        })
    }

    /// The id of the team store `cwd` belongs to, or `None` for the personal store.
    ///
    /// # Errors
    ///
    /// `relativeCwd` for a path that anchors nothing; `ambiguousStore` when patterns of two
    /// stores match — a session must never be split between teams, so the owner narrows a
    /// pattern rather than the engine picking one.
    pub fn route(&self, cwd: &str, syntax: PathSyntax) -> Result<Option<&str>, NamingError> {
        let parsed = ParsedPath::parse(cwd, syntax);
        if !parsed.is_anchored() {
            return Err(NamingError::RelativeCwd {
                cwd: cwd.to_owned(),
            });
        }
        let rendered = parsed.render();
        let mut stores: Vec<&str> = Vec::new();
        for (_, id) in self.rules.matches(&rendered) {
            if !stores.contains(&id.as_str()) {
                stores.push(id);
            }
        }
        match stores.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only)),
            many => Err(NamingError::AmbiguousStore {
                cwd: rendered,
                stores: many.iter().map(|id| (*id).to_owned()).collect(),
            }),
        }
    }
}
