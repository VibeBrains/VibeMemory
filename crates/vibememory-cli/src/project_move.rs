//! `vibememory project move <dir> --to <team>|personal`: a project's memory and sessions between the
//! personal store and a team's.
//!
//! Into a team: the project's memory goes to the team, its sessions are copied into the team's
//! clone as this machine's own — kept local like every session from before the project went to
//! the team — and the link is aimed at the team's clone. The personal store keeps its copy as an
//! archive with `moved.json`: the engine removes nothing.
//!
//! Back to the personal store: only what is this member's — sessions of this member's machines and
//! memory versions this member wrote — comes back. A teammate's work never leaves the team for a
//! personal store, which has its own remote and mirror the team never agreed to.
//!
//! The owner's routing decides first: a directory goes to a team only when `stores.<id>.cwd` of
//! `config.json` already names it, and back only when it no longer does. The command does not
//! rewrite the owner's file; it refuses and says which line to change.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vibememory_core::naming::encode_cwd;

use crate::config::Config;
use crate::install::Layout;
use crate::stores::StoreOf;

/// The mark a moved project leaves in the personal store's archive of it.
pub const MOVED_FILE: &str = "moved.json";

/// Where a project went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Moved {
    /// The team.
    pub to: String,
    /// When.
    pub at: String,
}

/// What a move did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveReport {
    /// The project's name in both stores.
    pub name: String,
    /// Files copied into the destination, relative to its project directory.
    pub copied: Vec<String>,
    /// Memory versions carried over.
    pub memory_versions: usize,
}

/// Where the project's session directory points now: the project's directory in some clone.
fn linked_project(link: &Path) -> Result<PathBuf, String> {
    let target = std::fs::read_link(link).map_err(|_| {
        format!(
            "{} is not linked to a store: open a session there first",
            link.display()
        )
    })?;
    std::fs::canonicalize(&target).map_err(|error| format!("{}: {error}", target.display()))
}

/// The project's name, when `project` is a directory of `store`'s `projects/`.
fn project_in(store: &Path, project: &Path) -> Option<String> {
    let projects = std::fs::canonicalize(store.join("projects")).ok()?;
    let relative = project.strip_prefix(&projects).ok()?;
    let mut parts = relative.components();
    let name = parts.next()?.as_os_str().to_string_lossy().into_owned();
    parts.next().is_none().then_some(name)
}

/// Refuses while any machine of either store reports a live session in the directory: moving the
/// files under a session costs it.
fn refuse_live(stores: &[&StoreOf], portable_cwd: &str) -> Result<(), String> {
    for store in stores {
        if let Some(claim) = crate::relink::live_everywhere(&store.clone)
            .into_iter()
            .find(|claim| claim.mark.cwd == portable_cwd)
        {
            return Err(format!(
                "{}. Nothing was moved — move again once the session is over",
                claim.explain()
            ));
        }
    }
    Ok(())
}

/// Copies `from` into `to`, keeping what `to` already has, and answers the copied paths relative
/// to `to`. `keep` decides which files go.
fn copy_tree(
    from: &Path,
    to: &Path,
    keep: &dyn Fn(&str) -> bool,
    prefix: &str,
    copied: &mut Vec<String>,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type().map_err(|error| error.to_string())?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), keep, &relative, copied)?;
        } else if kind.is_file() && keep(&relative) && !to.join(&name).exists() {
            std::fs::copy(entry.path(), to.join(&name)).map_err(|error| error.to_string())?;
            copied.push(relative);
        }
    }
    Ok(())
}

/// Whether a path inside a project is its memory rather than a session.
fn is_memory(relative: &str) -> bool {
    relative == crate::memory::JOURNAL_FILE || relative.starts_with("memory/")
}

/// Aims the project's session directory at `target`.
fn relink_to(link: &Path, target: &Path) -> Result<(), String> {
    crate::dir_link::remove(link)?;
    crate::dir_link::create(target, link)
}

/// Moves a project of the personal store into a team's.
///
/// # Errors
///
/// A directory not routed to the team, a project not in the personal store, a live session, or
/// what the file system refused.
pub fn to_team(
    layout: &Layout,
    config: &Config,
    cwd: &str,
    portable_cwd: &str,
    team: &str,
    stamp: &str,
) -> Result<MoveReport, String> {
    let syntax = crate::hook::session_start::host_syntax();
    if config
        .routes
        .route(cwd, syntax)
        .map_err(|e| e.to_string())?
        != Some(team)
    {
        return Err(format!(
            "{cwd} is not routed to team {team}: vibememory route add {cwd:?} --to {team} first"
        ));
    }
    let personal = crate::stores::personal(layout, config);
    let target = crate::stores::team(layout, team)?;
    let link = session_link(layout, cwd)?;
    let project = linked_project(&link)?;
    let name = project_in(&personal.clone, &project)
        .ok_or_else(|| format!("{cwd} is not a project of the personal store"))?;
    refuse_live(&[&personal, &target], portable_cwd)?;

    let destination = target.clone.join("projects").join(&name);
    let mut report = MoveReport {
        name: name.clone(),
        ..MoveReport::default()
    };
    copy_tree(
        &project,
        &destination,
        &|relative| relative != MOVED_FILE,
        "",
        &mut report.copied,
    )?;
    // the sessions are this machine's own from before the move: local, like every old session
    let local: Vec<String> = report
        .copied
        .iter()
        .filter(|relative| !is_memory(relative))
        .map(|relative| format!("projects/{name}/{relative}"))
        .collect();
    crate::local_only::keep(&target.clone, &local)?;
    report.memory_versions = report.copied.iter().filter(|r| is_memory(r)).count();
    relink_to(&link, &destination)?;
    let moved = Moved {
        to: team.to_owned(),
        at: stamp.to_owned(),
    };
    std::fs::write(
        project.join(MOVED_FILE),
        serde_json::to_string_pretty(&moved).map_err(|e| e.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(report)
}

/// Brings a moved project back to the personal store: this member's sessions and memory versions.
///
/// # Errors
///
/// A directory still routed to a team, a project born in the team, a live session, or what the
/// file system refused.
pub fn to_personal(
    layout: &Layout,
    config: &Config,
    cwd: &str,
    portable_cwd: &str,
) -> Result<MoveReport, String> {
    let syntax = crate::hook::session_start::host_syntax();
    if let Some(team) = config
        .routes
        .route(cwd, syntax)
        .map_err(|e| e.to_string())?
    {
        return Err(format!(
            "{cwd} is still routed to team {team}: vibememory route remove {cwd:?} first"
        ));
    }
    let link = session_link(layout, cwd)?;
    let project = linked_project(&link)?;
    let (source, name) = crate::team_connect::connected_teams(layout)
        .iter()
        .filter_map(|team| crate::stores::team(layout, team).ok())
        .find_map(|store| project_in(&store.clone, &project).map(|name| (store, name)))
        .ok_or_else(|| format!("{cwd} is not a project of a connected team"))?;
    let team = source.team.clone().unwrap_or_default();
    let personal = crate::stores::personal(layout, config);
    let archive = personal.clone.join("projects").join(&name);
    let moved: Option<Moved> = std::fs::read_to_string(archive.join(MOVED_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    if moved.as_ref().map(|moved| moved.to.as_str()) != Some(team.as_str()) {
        return Err(format!(
            "{name} belongs to team {team}: it was not moved there from this personal store"
        ));
    }
    refuse_live(&[&personal, &source], portable_cwd)?;

    bring_back(layout, &team, &project, &archive, &link, &name)
}

/// Brings this member's part of a team's project back into the personal archive it was moved from,
/// and aims the session directory at the archive again: sessions of this member's machines, and
/// the memory versions this member wrote. The caller has checked that no session is live.
///
/// # Errors
///
/// The store record does not read, or what the file system refused.
pub fn bring_back(
    layout: &Layout,
    team: &str,
    project: &Path,
    archive: &Path,
    link: &Path,
    name: &str,
) -> Result<MoveReport, String> {
    let record = crate::team_connect::read_record(layout, team)?;
    let clone = layout.team_store(team);
    let own = own_sessions(&clone, &record.member);
    let mut report = MoveReport {
        name: name.to_owned(),
        ..MoveReport::default()
    };
    copy_tree(
        project,
        archive,
        &|relative| {
            let session = relative
                .split('/')
                .next()
                .unwrap_or_default()
                .trim_end_matches(".jsonl");
            !is_memory(relative) && own.contains(session)
        },
        "",
        &mut report.copied,
    )?;
    report.memory_versions = carry_own_memory(project, archive, &record.member)?;
    relink_to(link, archive)?;
    match std::fs::remove_file(archive.join(MOVED_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(report)
}

/// The session ids of every machine of `member` in a team's clone: `machines/<member>-*/tails.json`.
fn own_sessions(clone: &Path, member: &str) -> BTreeSet<String> {
    let prefix = format!("{member}-");
    let Ok(machines) = std::fs::read_dir(clone.join("machines")) else {
        return BTreeSet::new();
    };
    machines
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("tails.json")).ok())
        .filter_map(|text| serde_json::from_str::<crate::hook::stop::Tails>(&text).ok())
        .flat_map(|tails| tails.sessions.into_keys())
        .collect()
}

/// Appends to the personal journal the versions of the team's journal that `member` wrote and the
/// personal one does not hold yet; answers how many.
fn carry_own_memory(project: &Path, archive: &Path, member: &str) -> Result<usize, String> {
    let Ok(team_journal) = std::fs::read_to_string(project.join(crate::memory::JOURNAL_FILE))
    else {
        return Ok(0);
    };
    let path = archive.join(crate::memory::JOURNAL_FILE);
    let mut own = std::fs::read_to_string(&path).unwrap_or_default();
    let held: BTreeSet<String> = own.lines().map(str::to_owned).collect();
    let mut carried = 0;
    for line in team_journal.lines() {
        let by_member = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("member")
                    .and_then(|m| m.as_str())
                    .map(str::to_owned)
            })
            .is_some_and(|written_by| written_by == member);
        if by_member && !held.contains(line) {
            if !own.is_empty() && !own.ends_with('\n') {
                own.push('\n');
            }
            own.push_str(line);
            own.push('\n');
            carried += 1;
        }
    }
    if carried > 0 {
        std::fs::write(&path, own).map_err(|error| error.to_string())?;
    }
    Ok(carried)
}

/// The project's session directory under the config directory.
fn session_link(layout: &Layout, cwd: &str) -> Result<PathBuf, String> {
    let enc = encode_cwd(cwd).map_err(|error| error.to_string())?;
    Ok(layout.config_dir.join("projects").join(enc.as_str()))
}
