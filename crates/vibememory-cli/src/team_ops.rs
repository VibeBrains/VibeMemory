//! Two ways out of trouble with a team's store: making the clone anew after the host refused what it
//! holds, and leaving the team. Neither removes anything of the person's: the old clone and the
//! left team's clone stay on the disk as archives the engine names and never deletes.

use std::path::{Path, PathBuf};

use crate::install::Layout;

/// A moment as a file name: the clock's own form with the colons Windows refuses taken out.
fn stamp_name(stamp: &str) -> String {
    stamp.replace(':', "-")
}

/// What `store reclone` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recloned {
    /// Where the refused clone went.
    pub rejected: PathBuf,
    /// Files carried over from it, relative to the clone.
    pub carried: Vec<String>,
}

/// Makes a team's clone anew after its host refused a path, a branch or a token in it: the old
/// clone goes aside as `store.rejected-<moment>`, the store is cloned again, and of the old tree
/// only what the team's store may hold comes over — its projects and this machine's own directory
/// — with the sessions this machine kept local still local. The pause ends; the session links
/// point at the same path and stay good.
///
/// # Errors
///
/// A team not connected here, a clone that is not there, or a clone that fails; after a failed
/// clone the old one is put back.
pub fn reclone(layout: &Layout, team: &str, stamp: &str) -> Result<Recloned, String> {
    let record = crate::team_connect::read_record(layout, team)?;
    reclone_from(layout, team, stamp, &record.git_url())
}

/// [`reclone`] from a given source, so the carrying over is checked without a host.
///
/// # Errors
///
/// As [`reclone`].
pub fn reclone_from(
    layout: &Layout,
    team: &str,
    stamp: &str,
    source: &str,
) -> Result<Recloned, String> {
    let record = crate::team_connect::read_record(layout, team)?;
    let state_dir = layout.team_state_dir(team);
    let clone = layout.team_store(team);
    if !clone.join(".git").is_dir() {
        return Err(format!("team {team} has no clone to make anew"));
    }
    let rejected = state_dir.join(format!("store.rejected-{}", stamp_name(stamp)));
    std::fs::rename(&clone, &rejected).map_err(|error| error.to_string())?;
    if let Err(error) = crate::team_connect::clone_store(&state_dir, &record, source) {
        let _ = std::fs::remove_dir_all(&clone);
        let _ = std::fs::rename(&rejected, &clone);
        return Err(error);
    }
    let mut carried = Vec::new();
    for tree in [
        "projects".to_owned(),
        format!("machines/{}", record.store_name),
    ] {
        carry(
            &rejected.join(&tree),
            &clone.join(&tree),
            &tree,
            &mut carried,
        )?;
    }
    crate::local_only::keep(&clone, &crate::local_only::kept_paths(&rejected))?;
    let state = crate::guard::TickState::read(&state_dir);
    crate::guard::TickState {
        pause: None,
        consecutive_failures: 0,
        runs_to_skip: 0,
        ..state
    }
    .write(&state_dir)?;
    Ok(Recloned { rejected, carried })
}

/// Copies what `to` does not have from `from`, and names it relative to the clone.
fn carry(from: &Path, to: &Path, prefix: &str, carried: &mut Vec<String>) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}/{name}");
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            carry(&entry.path(), &to.join(&name), &relative, carried)?;
        } else if kind.is_file() && !to.join(&name).exists() {
            std::fs::copy(entry.path(), to.join(&name)).map_err(|error| error.to_string())?;
            carried.push(relative);
        }
    }
    Ok(())
}

/// What leaving a team did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    /// Projects moved from the personal store that came back, with this member's part.
    pub brought_back: Vec<String>,
    /// Session directories of the team's own projects no longer linked here.
    pub unlinked: Vec<String>,
    /// Where the team's clone went.
    pub archive: PathBuf,
    /// The machine key to revoke, and where.
    pub key_id: String,
    /// The cabinet.
    pub cabinet: String,
}

/// Leaves a team with sessions: each project this machine moved into it comes back to the personal
/// store with this member's part; the links of the team's own projects are taken down; the team's
/// state directory goes aside as `stores/<team>.disconnected-<moment>`, the machine key taken out
/// of it. The key still opens the team until it is revoked in the cabinet.
///
/// # Errors
///
/// A team not connected here, a live session in one of its projects, or what the file system
/// refused.
pub fn leave(
    layout: &Layout,
    config: &crate::config::Config,
    team: &str,
    stamp: &str,
) -> Result<Left, String> {
    let record = crate::team_connect::read_record(layout, team)?;
    let clone = std::fs::canonicalize(layout.team_store(team)).map_err(|e| e.to_string())?;
    let personal = crate::stores::personal(layout, config);
    if let Some((machine, _, _)) = crate::relink::live_everywhere(&clone).into_iter().next() {
        return Err(format!(
            "a session of the team is live on {machine}: close it and leave again"
        ));
    }
    let mut brought_back = Vec::new();
    let mut unlinked = Vec::new();
    let links = std::fs::read_dir(layout.config_dir.join("projects"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for link in links {
        let Ok(target) = std::fs::read_link(&link) else {
            continue;
        };
        let Ok(project) = std::fs::canonicalize(&target) else {
            continue;
        };
        let Ok(relative) = project.strip_prefix(clone.join("projects")) else {
            continue;
        };
        let name = relative.to_string_lossy().into_owned();
        let archive = personal.clone.join("projects").join(&name);
        let moved_here = std::fs::read_to_string(archive.join(crate::project_move::MOVED_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<crate::project_move::Moved>(&text).ok())
            .is_some_and(|moved| moved.to == team);
        if moved_here {
            crate::project_move::bring_back(layout, team, &project, &archive, &link, &name)?;
            brought_back.push(name);
        } else {
            crate::dir_link::remove(&link)?;
            unlinked.push(name);
        }
    }
    let state_dir = layout.team_state_dir(team);
    let archive = layout
        .engine_dir
        .join("stores")
        .join(format!("{team}.disconnected-{}", stamp_name(stamp)));
    std::fs::rename(&state_dir, &archive).map_err(|error| error.to_string())?;
    for name in [
        crate::team_connect::KEY_FILE.to_owned(),
        format!("{}.pub", crate::team_connect::KEY_FILE),
    ] {
        match std::fs::remove_file(archive.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Left {
        brought_back,
        unlinked,
        archive,
        key_id: record.key_id,
        cabinet: record.cabinet,
    })
}
