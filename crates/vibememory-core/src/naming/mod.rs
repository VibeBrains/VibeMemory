//! Store naming: which directory under `<store>/projects/` a working directory belongs to, and
//! which `projects/<enc>` directory the CLI writes the transcript into.
//!
//! Order of resolution: a relative working directory is an error; then `ignoreCwd`; then a
//! filesystem root is an error; then `nameOverrides`, git, the directory name; the result is
//! validated, reconciled with existing store names, and finally checked against
//! `CLAUDE_CODE_PROJECT_DIR_NAME`. Every failure is an error the caller must respect: no link, no
//! store, no guessed name.

pub mod canonical;
pub mod config;
pub mod conflict;
pub mod enc;
pub mod error;
pub mod git;
pub(crate) mod path;
pub mod routes;
mod rules;
pub mod slug;
pub mod store_name;

pub use canonical::canonical_cwd;
pub use config::{NamingConfig, RawNamingConfig};
pub use conflict::{ConflictCopy, conflict_copies};
pub use enc::{
    EncSlug, PROJECTS_DIR_NAME, TRANSCRIPT_EXTENSION, enc_from_transcript_path, encode_cwd,
};
pub use error::NamingError;
pub use git::{DotGit, DotGitProbe, GitProbe, NameSource};
pub use path::PathSyntax;
pub use routes::StoreRoutes;
pub use slug::{MAX_SLUG_LENGTH, is_slug};
pub use store_name::{GIT_DIR_NAME, StoreName};

use git::GitLayout;
use path::ParsedPath;

/// Shown in a `gitLayoutMismatch` when no `.git` entry exists above the working directory.
const NO_DOT_GIT: &str = "<none>";

/// What the hook knows about the session before touching git.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamingInput<'a> {
    /// The working directory as the CLI reported it: NFC-normalized by the CLI; resolving
    /// symlinks (`/tmp` → `/private/tmp`) is the caller's job and must happen before every use
    /// of the path, because git prints physical paths. Never a verbatim Windows path.
    pub cwd: &'a str,
    /// Syntax of `cwd`: the host OS of the CLI.
    pub syntax: PathSyntax,
    /// `CLAUDE_CODE_PROJECT_DIR_NAME` from the hook's environment, if set (raw: the core applies
    /// the CLI's own acceptance rule).
    pub project_dir_name: Option<&'a str>,
}

/// Why the engine leaves a working directory alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreReason {
    /// An `ignoreCwd` pattern matched.
    Pattern(String),
    /// `CLAUDE_CODE_PROJECT_DIR_NAME` is set to something other than the store name, so the
    /// project directory is shared by every working directory of that environment.
    ProjectDirName(String),
}

/// Outcome of [`resolve_store_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Link `projects/<enc>` to `<store>/projects/<name>`.
    Named {
        /// The store directory name.
        name: StoreName,
        /// Which rule produced it.
        source: NameSource,
    },
    /// Do nothing: no link, no store.
    Ignored {
        /// Why.
        reason: IgnoreReason,
    },
}

/// Derives the store name for a working directory.
///
/// `probe` runs git only when neither `ignoreCwd` nor `nameOverrides` decided; `existing` holds
/// the names already present under `<store>/projects/` so that a name differing only by case or
/// Unicode form reuses the existing spelling instead of creating a second directory.
pub fn resolve_store_name(
    input: &NamingInput<'_>,
    probe: impl FnOnce() -> GitProbe,
    config: &NamingConfig,
    existing: &[StoreName],
) -> Result<Resolution, NamingError> {
    let cwd = ParsedPath::parse(input.cwd, input.syntax);
    if !cwd.is_anchored() {
        return Err(NamingError::RelativeCwd {
            cwd: input.cwd.to_owned(),
        });
    }
    // ignoreCwd before the root check: it is the remedy for a root cwd.
    let rendered = cwd.render();
    if let Some(pattern) = config.ignore_match(&rendered) {
        return Ok(Resolution::Ignored {
            reason: IgnoreReason::Pattern(pattern.to_owned()),
        });
    }
    let Some(dir_name) = cwd.last_normal() else {
        return Err(NamingError::RootCwd {
            cwd: input.cwd.to_owned(),
        });
    };
    let (name, source) = match config.override_for(&rendered)? {
        Some((pattern, name)) => (
            name.clone(),
            NameSource::Override {
                pattern: pattern.to_owned(),
            },
        ),
        None => name_from_git(&cwd, dir_name, probe())?,
    };
    let name = reconcile_with_existing(name, existing)?;
    if let Some(project_dir_name) = input
        .project_dir_name
        .and_then(enc::effective_project_dir_name)
        && !StoreName::parse(project_dir_name).is_ok_and(|p| p.key() == name.key())
    {
        return Ok(Resolution::Ignored {
            reason: IgnoreReason::ProjectDirName(project_dir_name.to_owned()),
        });
    }
    Ok(Resolution::Named { name, source })
}

fn name_from_git(
    cwd: &ParsedPath,
    dir_name: &str,
    probe: GitProbe,
) -> Result<(StoreName, NameSource), NamingError> {
    let syntax = cwd.syntax();
    let (common_dir, dot_git) = match probe {
        GitProbe::Unavailable { reason } => return Err(NamingError::GitUnavailable { reason }),
        GitProbe::Ran {
            common_dir,
            dot_git,
        } => (common_dir, dot_git),
    };
    match (common_dir, dot_git) {
        (_, DotGitProbe::Unknown { reason }) => Err(NamingError::DotGitProbeFailed { reason }),
        (Some(common_dir), dot_git) => {
            // Git for Windows and wrappers may leave CR or spaces; the layout must not.
            let common_dir = common_dir.trim().to_owned();
            let resolved = ParsedPath::parse(&common_dir, syntax);
            // An unrecognized layout (bare, separate git dir, leaked GIT_DIR) is the more
            // specific diagnosis and is reported before any mismatch with the nearest `.git`.
            let (name, layout) = git::name_from_git_dir(&resolved)?;
            let (explained, shown) = match dot_git {
                DotGitProbe::Found(DotGit::Dir { path }) => (
                    ParsedPath::parse(&path, syntax).same_location(&resolved),
                    path,
                ),
                DotGitProbe::Found(DotGit::File { path, content }) => {
                    let target = git::git_dir_from_pointer(&path, &content, syntax)?;
                    (
                        target.candidates().any(|c| c.same_location(&resolved)),
                        path,
                    )
                }
                DotGitProbe::NotFound | DotGitProbe::Unknown { .. } => {
                    (false, NO_DOT_GIT.to_owned())
                }
            };
            if !explained {
                return Err(NamingError::GitLayoutMismatch {
                    common_dir,
                    dot_git: shown,
                });
            }
            let source = match layout {
                GitLayout::Repo => NameSource::Repo { common_dir },
                GitLayout::Submodule => NameSource::Submodule { common_dir },
            };
            Ok((name, source))
        }
        (None, DotGitProbe::Found(DotGit::File { path, content })) => {
            let target = git::git_dir_from_pointer(&path, &content, syntax)?;
            let git_dir = target.preferred();
            let (name, _) = git::name_from_git_dir(git_dir)?;
            Ok((
                name,
                NameSource::DotGitFile {
                    gitdir: git_dir.render(),
                },
            ))
        }
        (None, DotGitProbe::Found(DotGit::Dir { path })) => {
            Err(NamingError::GitRefused { dot_git: path })
        }
        (None, DotGitProbe::NotFound) => Ok((StoreName::parse(dir_name)?, NameSource::PlainDir)),
    }
}

/// One store per identity key: an existing spelling wins, two different existing spellings are
/// a collision (the same spelling listed twice is not).
fn reconcile_with_existing(
    name: StoreName,
    existing: &[StoreName],
) -> Result<StoreName, NamingError> {
    let key = name.key();
    let mut same: Vec<&StoreName> = existing.iter().filter(|e| e.key() == key).collect();
    same.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    same.dedup_by(|a, b| a.as_str() == b.as_str());
    match same.as_slice() {
        [] => Ok(name),
        [one] => Ok((*one).clone()),
        many => Err(NamingError::StoreNameCollision {
            names: many.iter().map(|n| n.as_str().to_owned()).collect(),
        }),
    }
}
