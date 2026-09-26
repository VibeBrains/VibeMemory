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
        Ok(Some(id)) => team(layout, id).map_err(|_| {
            format!(
                "this project belongs to team {id}, which is not connected on this machine: its \
                 sessions stay here until `vibememory connect` with a machine code of the team"
            )
        }),
        Err(error) => Err(error.to_string()),
    }
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
