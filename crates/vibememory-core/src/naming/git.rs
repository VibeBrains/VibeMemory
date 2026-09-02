//! What the CLI observed about git in the working directory, and how a store name follows from
//! it. Nothing here runs git: the probe is data recorded by the caller.

use serde::Deserialize;

use super::error::NamingError;
use super::path::{ParsedPath, PathSyntax};
use super::store_name::{GIT_DIR_NAME, StoreName};

/// Directory under `.git` that holds the git dirs of absorbed submodules.
const GIT_MODULES_DIR_NAME: &str = "modules";
/// Directory under a git dir that holds the private dirs of linked worktrees.
const GIT_WORKTREES_DIR_NAME: &str = "worktrees";
/// First line of a `.git` pointer file: `gitdir: <path>`.
const GITDIR_LINE_PREFIX: &str = "gitdir:";

/// Result of the CLI running `git rev-parse --path-format=absolute --git-common-dir` in the
/// working directory (with every `GIT_*` variable except `GIT_EXEC_PATH` removed from the
/// environment) together with its own search for the nearest `.git` entry above the working
/// directory. Records are written by the CLI itself, so unknown fields are mistakes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GitProbe {
    /// git could not be run at all (not installed, timeout).
    Unavailable {
        /// What the CLI observed.
        reason: String,
    },
    /// git ran.
    Ran {
        /// Its stdout when the exit code was 0 (the core trims it); `None` otherwise.
        common_dir: Option<String>,
        /// The nearest `.git` entry above the working directory. Always probed, because git
        /// silently walks past a broken or empty `.git` directory to an outer repository.
        dot_git: DotGitProbe,
    },
}

/// The nearest `.git` entry found by walking up from the working directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum DotGitProbe {
    /// An entry named `.git` exists.
    Found(DotGit),
    /// The walk reached the root of the volume without finding one.
    NotFound,
    /// The walk did not complete (permission denied, hung mount, timeout).
    Unknown {
        /// What the CLI observed.
        reason: String,
    },
}

/// A `.git` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum DotGit {
    /// A directory: the git dir of a regular repository.
    Dir {
        /// Absolute path of the directory.
        path: String,
    },
    /// A pointer file: a linked worktree or an absorbed submodule.
    File {
        /// Absolute path of the file.
        path: String,
        /// Its full text.
        content: String,
    },
}

/// Which rule produced a store name. Recorded next to the link and quoted in `additionalContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameSource {
    /// A `nameOverrides` pattern matched.
    Override {
        /// The pattern.
        pattern: String,
    },
    /// `<repo>/.git`: a repository, a subdirectory of one, or a linked worktree.
    Repo {
        /// The common dir.
        common_dir: String,
    },
    /// `<super>/.git/modules/<path>`: an absorbed submodule.
    Submodule {
        /// The common dir.
        common_dir: String,
    },
    /// git refused, but the `.git` pointer file said where the git dir is.
    DotGitFile {
        /// The resolved `gitdir:` target.
        gitdir: String,
    },
    /// Not a repository: the directory name.
    PlainDir,
}

impl NameSource {
    /// Stable camelCase code of the variant.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Override { .. } => "override",
            Self::Repo { .. } => "repo",
            Self::Submodule { .. } => "submodule",
            Self::DotGitFile { .. } => "dotGitFile",
            Self::PlainDir => "plainDir",
        }
    }
}

/// A recognized git dir layout.
pub(crate) enum GitLayout {
    Repo,
    Submodule,
}

/// `<repo>/.git` names the repository; `<super>/.git/modules/<path>` names the submodule by its
/// last component; anything else (bare repository, `--separate-git-dir`, leaked `GIT_DIR`) is an
/// error rather than a guess.
pub(crate) fn name_from_git_dir(
    git_dir: &ParsedPath,
) -> Result<(StoreName, GitLayout), NamingError> {
    let normals = git_dir.normals();
    let shown = || git_dir.render();
    if normals.last().is_some_and(|last| last == GIT_DIR_NAME) {
        let Some(repo) = normals.len().checked_sub(2).and_then(|i| normals.get(i)) else {
            return Err(NamingError::EmptyRepoName {
                common_dir: shown(),
            });
        };
        return Ok((StoreName::parse(repo)?, GitLayout::Repo));
    }
    let is_modules_pair = |window: &[String]| matches!(window, [a, b] if a == GIT_DIR_NAME && b == GIT_MODULES_DIR_NAME);
    let modules_at = normals.windows(2).position(is_modules_pair);
    match (modules_at, normals.last()) {
        (Some(at), Some(last)) if at + 2 < normals.len() => {
            Ok((StoreName::parse(last)?, GitLayout::Submodule))
        }
        _ => Err(NamingError::UnrecognizedGitLayout {
            common_dir: shown(),
        }),
    }
}

/// Where a `.git` pointer file points, in two readings: as written, and with a trailing
/// `worktrees/<id>` pair dropped (a linked worktree's private dir sits under the git dir it
/// belongs to: `<repo>/.git/worktrees/<id>` or `<super>/.git/modules/<path>/worktrees/<id>`).
/// When git answered, the reading that equals its common dir is the right one; without git the
/// shortened reading is preferred, which misreads only a submodule whose own path ends in
/// `worktrees/<name>`.
pub(crate) struct PointerTarget {
    as_written: ParsedPath,
    without_worktree: Option<ParsedPath>,
}

impl PointerTarget {
    /// Both readings, most likely first.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = &ParsedPath> {
        self.without_worktree
            .iter()
            .chain(std::iter::once(&self.as_written))
    }

    /// The reading used when git gave no answer.
    pub(crate) fn preferred(&self) -> &ParsedPath {
        self.without_worktree.as_ref().unwrap_or(&self.as_written)
    }
}

/// Resolves a `.git` pointer file: the first line `gitdir: <path>`, a relative path resolved
/// against the file's directory.
pub(crate) fn git_dir_from_pointer(
    path: &str,
    content: &str,
    syntax: PathSyntax,
) -> Result<PointerTarget, NamingError> {
    let target = content
        .lines()
        .next()
        .and_then(|line| line.trim().strip_prefix(GITDIR_LINE_PREFIX))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| NamingError::MalformedDotGitFile {
            path: path.to_owned(),
        })?;
    let as_written = ParsedPath::parse(path, syntax).parent().join(target);
    let normals = as_written.normals();
    let is_worktree_slot = normals
        .len()
        .checked_sub(2)
        .and_then(|i| normals.get(i))
        .is_some_and(|dir| dir == GIT_WORKTREES_DIR_NAME);
    let without_worktree = is_worktree_slot.then(|| {
        let mut shortened = as_written.clone();
        shortened.truncate_normals(2);
        shortened
    });
    Ok(PointerTarget {
        as_written,
        without_worktree,
    })
}
