//! Errors of store naming. Every variant has a stable camelCase code used by fixtures, logs and
//! the hook's `additionalContext`; the human text may change, the code may not.

/// Why a store name (or an enc slug) could not be derived. An error is final: the caller must
/// not create a link or a store, and must not fall back to a guessed name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NamingError {
    /// `transcript_path` is not `<config dir>/projects/<enc>/<session id>.jsonl`.
    #[error("transcript_path is not `<config>/projects/<enc>/<sid>.jsonl`: {path}")]
    MalformedTranscriptPath {
        /// The offending path.
        path: String,
    },
    /// The project directory name is not what the CLI can produce (`[A-Za-z0-9_-]+`).
    #[error("enc slug must match [A-Za-z0-9_-]+: {slug:?}")]
    InvalidEncSlug {
        /// The offending slug.
        slug: String,
    },
    /// The working directory is not an absolute path.
    #[error("cwd is not an absolute path: {cwd}")]
    RelativeCwd {
        /// The offending cwd.
        cwd: String,
    },
    /// The working directory is a filesystem root and has no name to derive from.
    #[error("cwd is a filesystem root and has no project name; add it to ignoreCwd: {cwd}")]
    RootCwd {
        /// The offending cwd.
        cwd: String,
    },
    /// git could not be run at all (missing binary, timeout).
    #[error("git could not be run: {reason}")]
    GitUnavailable {
        /// What the CLI observed.
        reason: String,
    },
    /// The search for the nearest `.git` entry above cwd did not complete.
    #[error("the search for `.git` above cwd failed: {reason}")]
    DotGitProbeFailed {
        /// What the CLI observed.
        reason: String,
    },
    /// A `.git` directory exists but git refused it (dubious ownership, corruption).
    #[error(
        "git refused the repository whose `.git` directory is {dot_git}; not falling back to the directory name"
    )]
    GitRefused {
        /// Path of the refused `.git` directory.
        dot_git: String,
    },
    /// git resolved a common dir that does not belong to the nearest `.git` entry.
    #[error(
        "git resolved {common_dir} but the nearest `.git` is {dot_git}: repository layout mismatch"
    )]
    GitLayoutMismatch {
        /// What `git rev-parse --git-common-dir` printed.
        common_dir: String,
        /// The nearest `.git` entry found above cwd.
        dot_git: String,
    },
    /// The common dir is neither `<repo>/.git` nor `<repo>/.git/modules/<path>`.
    #[error(
        "git common dir is neither `<repo>/.git` nor `<repo>/.git/modules/<path>` (bare repository, separate git dir or leaked GIT_DIR): {common_dir}"
    )]
    UnrecognizedGitLayout {
        /// The offending common dir.
        common_dir: String,
    },
    /// The repository root has no name (`/.git`, `D:\.git`).
    #[error("repository directory has no name: {common_dir}")]
    EmptyRepoName {
        /// The offending common dir.
        common_dir: String,
    },
    /// A `.git` pointer file has no `gitdir:` line.
    #[error("`.git` pointer file has no `gitdir:` line: {path}")]
    MalformedDotGitFile {
        /// Path of the pointer file.
        path: String,
    },
    /// The derived name cannot be a directory on macOS, Windows or inside a git tree.
    #[error("store name {name:?} is invalid: {reason}")]
    InvalidStoreName {
        /// The offending name.
        name: String,
        /// Which rule failed.
        reason: &'static str,
    },
    /// Existing stores already collide on the identity key of the derived name.
    #[error("store names {names:?} differ only by case or Unicode form")]
    StoreNameCollision {
        /// The colliding existing names.
        names: Vec<String>,
    },
    /// More than one `nameOverrides` pattern matches the working directory.
    #[error("more than one nameOverrides pattern matches {cwd}: {patterns:?}")]
    AmbiguousOverride {
        /// The working directory.
        cwd: String,
        /// The matching patterns, in configuration order.
        patterns: Vec<String>,
    },
    /// `stores.<id>.cwd` patterns of more than one store match the working directory.
    #[error("working directory {cwd} matches the cwd patterns of several stores: {stores:?}")]
    AmbiguousStore {
        /// The working directory.
        cwd: String,
        /// The matching stores, in configuration order.
        stores: Vec<String>,
    },
    /// `config.json` holds an invalid naming rule.
    #[error("config.json {field}: {reason}")]
    ConfigInvalid {
        /// `nameOverrides` or `ignoreCwd`.
        field: &'static str,
        /// What is wrong.
        reason: String,
    },
}

impl NamingError {
    /// Stable camelCase code of the variant.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::MalformedTranscriptPath { .. } => "malformedTranscriptPath",
            Self::InvalidEncSlug { .. } => "invalidEncSlug",
            Self::RelativeCwd { .. } => "relativeCwd",
            Self::RootCwd { .. } => "rootCwd",
            Self::GitUnavailable { .. } => "gitUnavailable",
            Self::DotGitProbeFailed { .. } => "dotGitProbeFailed",
            Self::GitRefused { .. } => "gitRefused",
            Self::GitLayoutMismatch { .. } => "gitLayoutMismatch",
            Self::UnrecognizedGitLayout { .. } => "unrecognizedGitLayout",
            Self::EmptyRepoName { .. } => "emptyRepoName",
            Self::MalformedDotGitFile { .. } => "malformedDotGitFile",
            Self::InvalidStoreName { .. } => "invalidStoreName",
            Self::StoreNameCollision { .. } => "storeNameCollision",
            Self::AmbiguousOverride { .. } => "ambiguousOverride",
            Self::AmbiguousStore { .. } => "ambiguousStore",
            Self::ConfigInvalid { .. } => "configInvalid",
        }
    }
}
