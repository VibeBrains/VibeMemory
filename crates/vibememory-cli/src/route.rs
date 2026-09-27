//! Which store a working directory belongs to, checked against this machine's disk.
//!
//! The routing itself is the core's (`StoreRoutes`). What only this machine can tell is whether a
//! pattern names a directory that exists here, in the case the disk spells it, and whether a
//! project already kept in the personal store lies under a team's pattern.

use std::path::Path;

use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::PathSyntax;

use crate::config::Config;

/// What is wrong with the routes of this machine, one line each, with what to do about it.
///
/// Nothing here is an error: the routes still work as written. Each line names a way they route
/// differently from what their owner most likely meant.
#[must_use]
pub fn warnings(config: &Config, personal_store: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    for (pattern, team, prefix) in config.routes.patterns() {
        match std::fs::canonicalize(&prefix) {
            Err(_) => lines.push(format!(
                "stores.{team}.cwd {pattern:?}: {prefix} does not exist on this machine, so nothing \
                 routes to {team} through it"
            )),
            Ok(found) => {
                let found = found.to_string_lossy();
                if found != prefix {
                    lines.push(format!(
                        "stores.{team}.cwd {pattern:?}: the disk spells {prefix} as {found}; write \
                         {:?} so the pattern reads as the directory does",
                        pattern.replacen(&prefix, &found, 1)
                    ));
                }
            }
        }
    }
    let roots = Roots::new(
        config.roots.clone().into_iter().collect(),
        PathSyntax::Posix,
    );
    for record in crate::links_file::read(personal_store, &config.machine_id).links {
        // a record no root covered keeps the local path as it was
        let cwd = if record.cwd.starts_with('{') {
            match roots.to_local(&record.cwd) {
                Ok(cwd) => cwd,
                Err(_) => continue,
            }
        } else {
            record.cwd.clone()
        };
        if let Ok(Some(team)) = config.routes.route(&cwd, record.syntax) {
            lines.push(format!(
                "project {} ({cwd}) is routed to {team}, but its sessions are in the personal store: \
                 vibememory project move {cwd:?} --to {team}",
                record.name
            ));
        }
    }
    lines
}

/// What a change of the routes did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changed {
    /// The configuration text with the change, and the pattern it added or removed.
    Text {
        /// The whole new text of `config.json`.
        text: String,
        /// The patterns added or removed.
        patterns: Vec<String>,
    },
    /// The route was already as asked.
    Already,
}

/// The pattern that routes a directory and everything under it: its path as the disk spells it,
/// with `/**`.
///
/// # Errors
///
/// A directory that does not exist here: a route to nothing would be a rule that never applies.
pub fn pattern_for(dir: &str) -> Result<(String, String), String> {
    let found = std::fs::canonicalize(dir)
        .map_err(|error| format!("{dir}: {error} — a route names a directory that exists here"))?;
    if !found.is_dir() {
        return Err(format!("{dir} is not a directory"));
    }
    let canonical = vibememory_core::naming::canonical_cwd(
        &found.to_string_lossy(),
        crate::hook::session_start::host_syntax(),
    );
    let pattern = format!("{}/**", canonical.trim_end_matches('/'));
    Ok((canonical, pattern))
}

/// The routes with `dir` routed to `team`, as the new text of `config.json`.
///
/// Only the `stores` member of the text changes; every other byte of the owner's file stays.
///
/// # Errors
///
/// A directory already routed to another team — a session is never split between teams, so the
/// other route is removed first, knowingly — or a configuration the result would not load.
pub fn add(text: &str, dir: &str, team: &str) -> Result<Changed, String> {
    let (canonical, pattern) = pattern_for(dir)?;
    let config = Config::parse(text, crate::hook::session_start::host_syntax())
        .map_err(|error| error.to_string())?;
    match config
        .routes
        .route(&canonical, crate::hook::session_start::host_syntax())
        .map_err(|error| error.to_string())?
    {
        Some(current) if current == team => return Ok(Changed::Already),
        Some(other) => {
            return Err(format!(
                "{canonical} is routed to team {other}: vibememory route remove {canonical:?} first"
            ));
        }
        None => {}
    }
    let mut stores = stores_of(text)?;
    let entry = stores
        .entry(team.to_owned())
        .or_insert_with(|| serde_json::json!({ "cwd": [] }));
    let cwd = entry
        .as_object_mut()
        .ok_or_else(|| format!("stores.{team} is not an object"))?
        .entry("cwd")
        .or_insert_with(|| serde_json::json!([]));
    cwd.as_array_mut()
        .ok_or_else(|| format!("stores.{team}.cwd is not a list"))?
        .push(serde_json::Value::String(pattern.clone()));
    let text = with_stores(text, &stores)?;
    Config::parse(&text, crate::hook::session_start::host_syntax())
        .map_err(|error| format!("the new route would not load: {error}"))?;
    Ok(Changed::Text {
        text,
        patterns: vec![pattern],
    })
}

/// The routes without the patterns that name `dir` itself, as the new text of `config.json`.
///
/// # Errors
///
/// A directory routed only by a pattern over a wider directory: removing that would take the whole
/// wider directory away from its team, which is not what was asked.
pub fn remove(text: &str, dir: &str) -> Result<Changed, String> {
    let (canonical, _) = pattern_for(dir)?;
    let config = Config::parse(text, crate::hook::session_start::host_syntax())
        .map_err(|error| error.to_string())?;
    let Some(team) = config
        .routes
        .route(&canonical, crate::hook::session_start::host_syntax())
        .map_err(|error| error.to_string())?
        .map(str::to_owned)
    else {
        return Ok(Changed::Already);
    };
    let own: Vec<String> = config
        .routes
        .patterns()
        .filter(|(_, id, prefix)| *id == team && same_dir(prefix, &canonical))
        .map(|(pattern, _, _)| pattern.to_owned())
        .collect();
    if own.is_empty() {
        let wider: Vec<String> = config
            .routes
            .patterns()
            .filter(|(_, id, _)| *id == team)
            .map(|(pattern, _, _)| pattern.to_owned())
            .collect();
        return Err(format!(
            "{canonical} is routed to team {team} by a pattern over a wider directory ({}): narrow it in \
             config.json by hand",
            wider.join(", ")
        ));
    }
    let mut stores = stores_of(text)?;
    if let Some(entry) = stores
        .get_mut(&team)
        .and_then(serde_json::Value::as_object_mut)
    {
        if let Some(cwd) = entry
            .get_mut("cwd")
            .and_then(serde_json::Value::as_array_mut)
        {
            cwd.retain(|pattern| !pattern.as_str().is_some_and(|p| own.iter().any(|o| o == p)));
            if cwd.is_empty() {
                entry.remove("cwd");
            }
        }
        if entry.is_empty() {
            stores.remove(&team);
        }
    }
    let text = if stores.is_empty() {
        vibememory_core::json_edit::remove_top_level(text, STORES).map_err(|e| e.to_string())?
    } else {
        with_stores(text, &stores)?
    };
    Config::parse(&text, crate::hook::session_start::host_syntax())
        .map_err(|error| format!("the configuration without the route would not load: {error}"))?;
    Ok(Changed::Text {
        text,
        patterns: own,
    })
}

/// The key of the routes in `config.json`.
const STORES: &str = "stores";

fn same_dir(prefix: &str, dir: &str) -> bool {
    if vibememory_core::naming::CaseRule::HOST == vibememory_core::naming::CaseRule::Insensitive {
        prefix.eq_ignore_ascii_case(dir) || prefix.to_lowercase() == dir.to_lowercase()
    } else {
        prefix == dir
    }
}

fn stores_of(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    match value.get(STORES) {
        None => Ok(serde_json::Map::new()),
        Some(serde_json::Value::Object(stores)) => Ok(stores.clone()),
        Some(_) => Err("stores in config.json is not an object".to_owned()),
    }
}

fn with_stores(
    text: &str,
    stores: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, String> {
    let value = serde_json::to_string_pretty(stores).map_err(|e| e.to_string())?;
    vibememory_core::json_edit::set_top_level(text, STORES, &value).map_err(|e| e.to_string())
}

/// Writes `config.json` whole or not at all.
///
/// # Errors
///
/// What the file system refused.
pub fn write_config(path: &Path, text: &str) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}
