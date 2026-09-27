//! Which memory servers Claude Code has registered, read from its own configuration and never
//! written: the engine does not touch `.claude.json`, and says what to run instead.
//!
//! On a machine with the engine one local server, `vibememory`, reaches every store — the personal
//! one, a team's clone, and a team whose memory lives on the host alone, by the directory's route.
//! A server per team registered before it (`vibememory-<team>`) only makes the agent guess which
//! of two memories to write into.

use std::path::{Path, PathBuf};

use crate::install::Layout;

/// The name of the local server.
pub const LOCAL_SERVER: &str = "vibememory";

/// The agent the printed registration names.
const AGENT: &str = "claude-code";

/// What Claude Code has registered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registered {
    /// Whether the local server is registered.
    pub local: bool,
    /// The teams with a server of their own, `vibememory-<team>`.
    pub teams: Vec<String>,
}

/// Where Claude Code keeps its user configuration: inside its configuration directory when that
/// was moved (`CLAUDE_CONFIG_DIR`), beside the home directory otherwise.
fn claude_json(layout: &Layout) -> PathBuf {
    let inside = layout.config_dir.join(".claude.json");
    if inside.is_file() {
        return inside;
    }
    layout
        .config_dir
        .parent()
        .map_or(inside.clone(), |home| home.join(".claude.json"))
}

/// The memory servers registered for every project, read only.
#[must_use]
pub fn read(layout: &Layout) -> Registered {
    read_from(&claude_json(layout))
}

fn read_from(path: &Path) -> Registered {
    let Some(servers) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|config| config.get("mcpServers").cloned())
    else {
        return Registered::default();
    };
    let Some(servers) = servers.as_object() else {
        return Registered::default();
    };
    let prefix = format!("{LOCAL_SERVER}-");
    let mut teams: Vec<String> = servers
        .keys()
        .filter_map(|name| name.strip_prefix(&prefix).map(str::to_owned))
        .filter(|team| vibememory_core::naming::is_slug(team))
        .collect();
    teams.sort();
    Registered {
        local: servers.contains_key(LOCAL_SERVER),
        teams,
    }
}

/// What to run so the agent has one memory server, one line each. Empty when that is so already.
#[must_use]
pub fn advice(
    layout: &Layout,
    config: &crate::config::Config,
    registered: &Registered,
) -> Vec<String> {
    let mut lines = Vec::new();
    if !registered.local {
        lines.push(format!(
            "the local memory server is not registered: claude mcp add -s user {LOCAL_SERVER} {} -- --agent {AGENT}",
            crate::install::installed_named(layout, crate::install::MCP_BINARY).display()
        ));
    }
    let routed: Vec<&str> = config.routes.patterns().map(|(_, team, _)| team).collect();
    for team in &registered.teams {
        let covered = team == "personal"
            || crate::team_connect::connected_teams(layout).contains(team)
            || routed.contains(&team.as_str());
        if covered {
            lines.push(format!(
                "{LOCAL_SERVER}-{team} repeats what the local server reaches, and the agent has to guess between \
                 them: claude mcp remove -s user {LOCAL_SERVER}-{team}"
            ));
        } else {
            lines.push(format!(
                "{LOCAL_SERVER}-{team} is the only way to team {team} here: route its directories with \
                 vibememory route add <dir> --to {team}, then remove it"
            ));
        }
    }
    lines
}
