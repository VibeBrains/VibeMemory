//! Which project of the store a working directory belongs to, by the rules the `SessionStart` hook
//! links it with — so a memory written from a directory lands next to that directory's transcripts,
//! whichever program writes it.

use std::path::Path;
use std::time::Duration;

use vibememory_core::naming::{
    NamingConfig, NamingError, NamingInput, Resolution, resolve_store_name,
};

/// Set by Claude Code when every working directory of a run shares one project directory.
pub const PROJECT_DIR_NAME_VAR: &str = "CLAUDE_CODE_PROJECT_DIR_NAME";

/// How long the naming rules may wait for git.
pub const NAMING_GIT_TIMEOUT: Duration = Duration::from_secs(3);

/// The store project of `cwd`, which must already be canonical (see `canonical_cwd`).
///
/// One function for the hook and the memory server: written twice, the two would one day disagree
/// about where a directory's memories live.
///
/// # Errors
///
/// What the naming rules refused — a relative directory, an ambiguous override.
pub fn resolve(store: &Path, naming: &NamingConfig, cwd: &str) -> Result<Resolution, NamingError> {
    let names = crate::store::existing(store, NAMING_GIT_TIMEOUT);
    let project_dir_name = std::env::var(PROJECT_DIR_NAME_VAR).ok();
    let input = NamingInput {
        cwd,
        syntax: crate::hook::session_start::host_syntax(),
        project_dir_name: project_dir_name.as_deref(),
    };
    resolve_store_name(
        &input,
        || crate::git::probe(Path::new(cwd), NAMING_GIT_TIMEOUT),
        naming,
        &names.names,
    )
}
