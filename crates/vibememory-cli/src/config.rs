//! `~/.vibememory/config.json` — everything this machine needs to know about itself.
//!
//! The file is per-machine and never enters the store: the paths in it are the paths of this
//! disk. It is read once, whole, and either understood completely or refused — a configuration
//! that is half applied is worse than none, because the half that failed is the half the owner
//! wrote deliberately.
//!
//! Hence `deny_unknown_fields`. A typo in a key would otherwise mean "this rule does not exist",
//! and the owner would learn about it when a session lands in the wrong store, not when the
//! engine started.

use std::collections::BTreeMap;

use serde::Deserialize;
use vibememory_core::naming::{
    NamingConfig, NamingError, PathSyntax, RawNamingConfig, StoreRoutes,
};

/// Where the store of Desktop descriptors is. Written as a string: `"auto"`, or the directory
/// itself when the search would guess wrong.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub enum DesktopStore {
    /// Find it in the places the installers use — the Squirrel and MSIX layouts differ, and on
    /// Windows the same machine may hold both.
    #[default]
    Auto,
    /// This exact directory.
    Path(String),
}

/// The word that asks for the search instead of naming a directory.
const AUTO: &str = "auto";

impl From<String> for DesktopStore {
    fn from(value: String) -> Self {
        if value == AUTO {
            Self::Auto
        } else {
            Self::Path(value)
        }
    }
}

/// The owner's rule for a team store: `stores.<id>` of the file. What the machine knows about the
/// team — its remote, cabinet, member and machine name — `connect` keeps in the store's own
/// `store.json`, so this file stays the owner's and the engine never rewrites it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreRules {
    /// Working directories whose sessions and memory belong to the team, in the dialect of
    /// `nameOverrides`: a literal path matches exactly, a subtree needs `/**`.
    #[serde(default)]
    pub cwd: Vec<String>,
}

/// The configuration exactly as written, before validation.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RawConfig {
    /// Unique per machine; goes into the names of everything this machine writes into the store,
    /// so that two machines never collide by construction.
    pub machine_id: String,
    /// The git remote of the store. Absent while the store is local: a machine migrates first and
    /// publishes later, and the tick simply neither fetches nor pushes until then.
    pub remote: Option<String>,
    /// Named roots, so a working directory can be written portably as `{NAME}/rel` and mean the
    /// same project on the other machine.
    pub roots: BTreeMap<String, String>,
    /// Where the Desktop descriptors live.
    pub desktop_store: DesktopStore,
    /// Code signing identity for the installed binary, e.g. `VibeMemory Local`. Optional, and
    /// meaningful on macOS only.
    pub signing_identity: Option<String>,
    /// How many transcripts one tick may remove before it holds and asks. Absent means the
    /// default; it is the owner's policy, because one person forgets single sessions and another
    /// drops a whole project.
    pub max_deletions_per_tick: Option<usize>,
    /// The owner's rules for team stores, by id — the team's slug.
    pub stores: BTreeMap<String, StoreRules>,
    /// `nameOverrides` and `ignoreCwd`, validated by the core.
    #[serde(flatten)]
    pub naming: RawNamingConfig,
}

/// Why a configuration cannot be used. The engine refuses to start rather than run on half of it.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file is not the JSON this version understands — including an unknown key, which is
    /// nearly always a typo in a key that was meant to matter.
    #[error("config.json is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
    /// A required field is missing or empty.
    #[error("config.json: {field} is required")]
    Missing {
        /// The field's name as written in the file.
        field: &'static str,
    },
    /// A root is not an absolute path, and so cannot anchor anything.
    #[error("config.json: root {name} must be an absolute path, got {path:?}")]
    RelativeRoot {
        /// The root's name.
        name: String,
        /// What was written.
        path: String,
    },
    /// The naming rules did not validate.
    #[error(transparent)]
    Naming(#[from] NamingError),
}

/// A configuration that has been read and checked.
#[derive(Debug, Clone)]
pub struct Config {
    /// Unique per machine.
    pub machine_id: String,
    /// The git remote of the store, when one has been declared.
    pub remote: Option<String>,
    /// Named roots, canonical and absolute.
    pub roots: BTreeMap<String, String>,
    /// Where the Desktop descriptors live.
    pub desktop_store: DesktopStore,
    /// The code signing identity to sign the installed binary with, when the owner made one.
    ///
    /// macOS grants access to a removable volume, and to everything else it guards, to a
    /// *signature*, not to a path. An ad-hoc signed binary has a different identity after every
    /// rebuild, so every rebuild asks the owner for permission again. A stable self-signed
    /// identity is asked once.
    pub signing_identity: Option<String>,
    /// Validated naming rules.
    pub naming: NamingConfig,
    /// How many transcripts one tick may remove before holding.
    pub max_deletions_per_tick: usize,
    /// Which store a working directory belongs to, compiled from `stores.<id>.cwd`.
    pub routes: StoreRoutes,
}

impl Config {
    /// Reads a configuration from the text of the file.
    ///
    /// `syntax` is the syntax of this machine, passed in rather than derived: a root written as
    /// `/Users/o` is a POSIX path on macOS and nonsense on Windows, and the difference is the
    /// host's, not the string's.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] — and the engine then does not start. Refusing is the point: the rules in
    /// this file are the owner's manual override of everything automatic.
    pub fn parse(text: &str, syntax: PathSyntax) -> Result<Self, ConfigError> {
        let raw: RawConfig = serde_json::from_str(text)?;
        Self::from_raw(raw, syntax)
    }

    /// Validates a configuration that has already been read.
    ///
    /// # Errors
    ///
    /// [`ConfigError`], as [`Config::parse`].
    pub fn from_raw(raw: RawConfig, syntax: PathSyntax) -> Result<Self, ConfigError> {
        if raw.machine_id.trim().is_empty() {
            return Err(ConfigError::Missing { field: "machineId" });
        }
        // A remote is optional, but a declared one must name something: an empty string would
        // make the tick try to push to nowhere every two minutes.
        if raw
            .remote
            .as_deref()
            .is_some_and(|remote| remote.trim().is_empty())
        {
            return Err(ConfigError::Missing { field: "remote" });
        }
        // A declared identity must name something: an empty string would make every install run
        // `codesign --sign ""`, which fails in a way that looks like a broken build.
        if raw
            .signing_identity
            .as_deref()
            .is_some_and(|identity| identity.trim().is_empty())
        {
            return Err(ConfigError::Missing {
                field: "signingIdentity",
            });
        }
        let routes = StoreRoutes::compile(
            &raw.stores
                .iter()
                .map(|(id, store)| (id.clone(), store.cwd.clone()))
                .collect::<Vec<_>>(),
        )?;
        let mut roots = BTreeMap::new();
        for (name, path) in raw.roots {
            let canonical = vibememory_core::naming::canonical_cwd(&path, syntax);
            if !is_absolute(&canonical, syntax) {
                return Err(ConfigError::RelativeRoot { name, path });
            }
            roots.insert(name, canonical);
        }
        Ok(Self {
            machine_id: raw.machine_id,
            remote: raw.remote,
            roots,
            desktop_store: raw.desktop_store,
            signing_identity: raw
                .signing_identity
                .map(|identity| identity.trim().to_owned()),
            naming: NamingConfig::from_raw(&raw.naming)?,
            max_deletions_per_tick: raw
                .max_deletions_per_tick
                .unwrap_or(crate::guard::DEFAULT_MAX_DELETIONS_PER_TICK),
            routes,
        })
    }

    /// The named roots of this machine, as the core wants them.
    #[must_use]
    pub fn roots(&self) -> vibememory_core::desktop::roots::Roots {
        vibememory_core::desktop::roots::Roots::new(
            self.roots.clone().into_iter().collect(),
            PathSyntax::Posix,
        )
    }

    /// The working directory in the form other machines can read, or the local one when no root
    /// covers it — a path nobody can translate is still better in a log than nothing.
    #[must_use]
    pub fn portable_cwd(&self, cwd: &str) -> String {
        self.roots()
            .to_portable(cwd)
            .unwrap_or_else(|_| cwd.to_owned())
    }
}

/// Whether a canonical path names a place rather than a direction: `/a`, `D:/a`, `//server/share`.
fn is_absolute(canonical: &str, syntax: PathSyntax) -> bool {
    match syntax {
        PathSyntax::Posix => canonical.starts_with('/'),
        PathSyntax::Windows => {
            canonical.starts_with("//")
                || canonical
                    .split_once(":/")
                    .is_some_and(|(drive, _)| drive.len() == 1)
        }
    }
}
