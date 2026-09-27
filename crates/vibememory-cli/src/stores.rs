//! Which store a session belongs to: the personal store, or the store of a team the working
//! directory is routed to by `stores.<id>.cwd`. The hooks ask it by working directory when a
//! session starts, and by the transcript's real path when it stops — the link already decided
//! then, and the clone holding the file is the answer.

use std::path::{Path, PathBuf};

use vibememory_core::naming::PathSyntax;

use crate::config::Config;
use crate::install::Layout;

/// A store as the hooks use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOf {
    /// The clone.
    pub clone: PathBuf,
    /// Its state directory: tick state, quarantine, what is held back.
    pub state_dir: PathBuf,
    /// This machine's name in the store: `machineId` in the personal one, `storeName` in a team's.
    pub machine_id: String,
    /// The team, for a team store.
    pub team: Option<String>,
}

/// The personal store.
#[must_use]
pub fn personal(layout: &Layout, config: &Config) -> StoreOf {
    StoreOf {
        clone: layout.store(),
        state_dir: layout.engine_dir.clone(),
        machine_id: config.machine_id.clone(),
        team: None,
    }
}

/// A connected team's store, from the record `connect` left.
///
/// # Errors
///
/// The team is not connected here: no record, or one this engine does not read.
pub fn team(layout: &Layout, team: &str) -> Result<StoreOf, String> {
    let record = crate::team_connect::read_record(layout, team)?;
    Ok(StoreOf {
        clone: layout.team_store(team),
        state_dir: layout.team_state_dir(team),
        machine_id: record.store_name,
        team: Some(team.to_owned()),
    })
}

/// The store of a working directory, in canonical form.
///
/// # Errors
///
/// Two teams claim the directory, or it belongs to a team that is not connected on this machine.
/// Either way the session goes to no store: sending a team's work to the personal store would
/// carry it where the team never agreed it goes.
pub fn for_cwd(
    layout: &Layout,
    config: &Config,
    cwd: &str,
    syntax: PathSyntax,
) -> Result<StoreOf, String> {
    match config.routes.route(cwd, syntax) {
        Ok(None) => Ok(personal(layout, config)),
        // a team whose memory lives on the host alone keeps no sessions: they stay here, and the
        // memory server of the directory reaches the team's memory over HTTPS
        Ok(Some(id)) if !is_connected_clone(layout, id) && memory_only(layout, id) => {
            Ok(personal(layout, config))
        }
        Ok(Some(id)) => team(layout, id).map_err(|_| {
            format!(
                "this project belongs to team {id}, which is not connected on this machine: its \
                 sessions stay here until `vibememory connect` with a machine code of the team"
            )
        }),
        Err(error) => Err(error.to_string()),
    }
}

/// Whether a team's clone is connected on this machine.
fn is_connected_clone(layout: &Layout, id: &str) -> bool {
    crate::team_connect::connected_teams(layout)
        .iter()
        .any(|team| team == id)
}

/// Whether this machine reaches a team by token alone: a token kept for it, and no clone.
#[must_use]
pub fn memory_only(layout: &Layout, id: &str) -> bool {
    !is_connected_clone(layout, id)
        && layout
            .engine_dir
            .join(crate::connect::TOKENS_DIR)
            .join(id)
            .is_dir()
}

/// The memory server a directory routed to a token-only team reaches: its address from the
/// token's sidecar for `agent`. `None` for a directory of the personal store or a team's clone.
///
/// # Errors
///
/// A directory routed to a token-only team for which `agent` has no token, or whose sidecar does
/// not say where the server is: the memory would otherwise go to the personal store.
pub fn remote_memory(
    layout: &Layout,
    config: &Config,
    cwd: &str,
    syntax: PathSyntax,
    agent: &str,
) -> Result<Option<(String, String)>, String> {
    let Some(id) = config
        .routes
        .route(cwd, syntax)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if !memory_only(layout, id) {
        return Ok(None);
    }
    let token = crate::connect::token_file(layout, id, agent);
    let sidecar = crate::connect::sidecar_file(&token);
    let text = std::fs::read_to_string(&sidecar).map_err(|_| {
        format!(
            "this project belongs to team {id}, and agent {agent} has no token of it here: \
             vibememory connect --cabinet <address> --agent {agent}"
        )
    })?;
    let url = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|sidecar| sidecar.get("mcpUrl")?.as_str().map(str::to_owned))
        .ok_or_else(|| {
            format!(
                "{} does not name the team's memory server",
                sidecar.display()
            )
        })?;
    Ok(Some((id.to_owned(), url)))
}

/// The store whose clone holds a file, by its real path: the personal store first, then every
/// connected team. `None` for a file in no clone — a session in a real directory.
#[must_use]
pub fn for_file(layout: &Layout, config: &Config, real: &Path) -> Option<(StoreOf, PathBuf)> {
    std::iter::once(personal(layout, config))
        .chain(
            crate::team_connect::connected_teams(layout)
                .iter()
                .filter_map(|id| team(layout, id).ok()),
        )
        .find_map(|store| {
            let clone = std::fs::canonicalize(&store.clone).ok()?;
            let relative = real.strip_prefix(&clone).ok()?.to_path_buf();
            Some((StoreOf { clone, ..store }, relative))
        })
}
