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
