//! The naming part of `~/.vibememory/config.json`: `nameOverrides` and `ignoreCwd`.

use std::fmt;

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};

use super::error::NamingError;
use super::rules::PathRules;
use super::store_name::StoreName;

const NAME_OVERRIDES: &str = "nameOverrides";
const IGNORE_CWD: &str = "ignoreCwd";

/// `nameOverrides` and `ignoreCwd` exactly as written in `config.json`, unvalidated. Meant to be
/// embedded (`#[serde(flatten)]`) into the full configuration type, which decides about unknown
/// fields; on its own it ignores them.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RawNamingConfig {
    /// Pattern → store name, in file order; a repeated key is kept so that validation can
    /// reject it. A literal path matches exactly; `/**` matches a subtree.
    #[serde(deserialize_with = "ordered_pairs")]
    pub name_overrides: Vec<(String, String)>,
    /// Patterns of working directories the engine leaves alone.
    pub ignore_cwd: Vec<String>,
}

/// Reads a JSON object as ordered pairs without collapsing duplicate keys.
fn ordered_pairs<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<(String, String)>, D::Error> {
    struct Pairs;

    impl<'de> Visitor<'de> for Pairs {
        type Value = Vec<(String, String)>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an object of pattern → store name")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut pairs = Vec::new();
            while let Some(pair) = map.next_entry::<String, String>()? {
                pairs.push(pair);
            }
            Ok(pairs)
        }
    }

    deserializer.deserialize_map(Pairs)
}

/// Validated naming rules.
#[derive(Debug, Default, Clone)]
pub struct NamingConfig {
    overrides: PathRules<StoreName>,
    ignore: PathRules<()>,
}

impl NamingConfig {
    /// Validates every pattern and every override value; the first problem is the error.
    pub fn from_raw(raw: &RawNamingConfig) -> Result<Self, NamingError> {
        let mut overrides = Vec::with_capacity(raw.name_overrides.len());
        for (pattern, name) in &raw.name_overrides {
            if !PathRules::<StoreName>::has_literal_component(pattern) {
                return Err(NamingError::ConfigInvalid {
                    field: NAME_OVERRIDES,
                    reason: format!(
                        "pattern {pattern:?} has no literal component and would capture every project"
                    ),
                });
            }
            let name = StoreName::parse(name).map_err(|e| NamingError::ConfigInvalid {
                field: NAME_OVERRIDES,
                reason: format!("value for {pattern:?}: {e}"),
            })?;
            overrides.push((pattern.clone(), name));
        }
        let ignore = raw.ignore_cwd.iter().map(|p| (p.clone(), ())).collect();
        Ok(Self {
            overrides: PathRules::compile(overrides, NAME_OVERRIDES)?,
            ignore: PathRules::compile(ignore, IGNORE_CWD)?,
        })
    }

    /// The first `ignoreCwd` pattern matching the rendered path, in configuration order.
    /// Whether a canonical working directory is one the engine leaves alone. The migration asks
    /// this about `/` before importing an archive of runner sessions that live there.
    #[must_use]
    pub fn ignores(&self, rendered_path: &str) -> bool {
        self.ignore_match(rendered_path).is_some()
    }

    pub(crate) fn ignore_match(&self, rendered_path: &str) -> Option<&str> {
        self.ignore
            .matches(rendered_path)
            .next()
            .map(|(pattern, ())| pattern)
    }

    /// The single `nameOverrides` entry matching the rendered path; more than one is an error.
    pub(crate) fn override_for(
        &self,
        rendered_path: &str,
    ) -> Result<Option<(&str, &StoreName)>, NamingError> {
        let hits: Vec<(&str, &StoreName)> = self.overrides.matches(rendered_path).collect();
        match hits.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
            many => Err(NamingError::AmbiguousOverride {
                cwd: rendered_path.to_owned(),
                patterns: many.iter().map(|(p, _)| (*p).to_owned()).collect(),
            }),
        }
    }
}
