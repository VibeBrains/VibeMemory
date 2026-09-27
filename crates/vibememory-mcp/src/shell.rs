//! What a machine key may run on the host: the forced command of every key line in `vmgit`'s
//! `authorized_keys`.
//!
//! Pure: the command the client asked for (`SSH_ORIGINAL_COMMAND`), the key and the snapshot in;
//! one of four actions or a refusal with its code out. The rules and codes are those of
//! `docs/manuals/hostShellSpec.md`, in its order. Anything that is not exactly one of the four
//! shapes is refused before any repository is looked at: the command is text from the network.

use crate::access::{Mode, Snapshot, is_name};
use crate::layout::{TeamDir, team_dir};

/// Where team stores live, as a client names them: `teams/<slug>.git`.
const TEAMS_PREFIX: &str = "teams/";

/// What the key may run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellAction {
    /// `git-upload-pack` of a team store: clone and fetch.
    Upload {
        /// The team's slug.
        team: String,
    },
    /// `git-receive-pack` of a sync team's store: push.
    Receive {
        /// The team's slug.
        team: String,
    },
    /// The memory server of a team over stdio.
    Mcp {
        /// The team's slug.
        team: String,
        /// The agent, as the client names it.
        agent: String,
    },
    /// The status of the key's teams.
    Status,
}

/// Why the key may not run what it asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRefusal {
    /// Stable camelCase code from the spec.
    pub code: &'static str,
    /// What exactly, for the person at the other end.
    pub detail: String,
}

fn refuse(code: &'static str, detail: impl Into<String>) -> ShellRefusal {
    ShellRefusal {
        code,
        detail: detail.into(),
    }
}

/// What the command asks for, before the snapshot says whether it may.
enum Asked {
    Upload(String),
    Receive(String),
    Mcp(String, String),
    Status,
}

/// Decides what `key` may run for `original`, the command its client sent, at `now`
/// (`YYYY-MM-DDTHH:MM:SSZ`).
///
/// `snapshot` is `None` when the access snapshot cannot be used: then nobody is let in. `teams_dir`
/// is where the host keeps team stores, for the absolute form of a path. The key of a banned member
/// is refused as one the snapshot does not have.
///
/// # Errors
///
/// The first rule the request breaks.
pub fn decide(
    original: &str,
    key: &str,
    snapshot: Option<&Snapshot>,
    teams_dir: &str,
    now: &str,
) -> Result<ShellAction, ShellRefusal> {
    let Some(snapshot) = snapshot else {
        return Err(refuse(
            "snapshotUnusable",
            "the host cannot read who may come in; nobody is let in until it can",
        ));
    };
    let Some(key) = snapshot
        .key(key)
        .filter(|found| !snapshot.barred(&found.member, now))
    else {
        return Err(refuse(
            "unknownKey",
            format!("key {key} is not in the snapshot"),
        ));
    };
    let asked = parse(original, teams_dir).ok_or_else(|| {
        refuse(
            "commandDenied",
            "a key may run git-upload-pack or git-receive-pack of teams/<slug>.git, \
             mcp --team <slug> --agent <agent>, or status",
        )
    })?;
    let team_slug = match &asked {
        Asked::Status => return Ok(ShellAction::Status),
        Asked::Upload(team) | Asked::Receive(team) | Asked::Mcp(team, _) => team.clone(),
    };
    let Some(team) = snapshot
        .teams
        .get(&team_slug)
        .filter(|team| team.deleted.is_none())
    else {
        return Err(refuse("teamGone", format!("there is no team {team_slug}")));
    };
    let rank = key
        .teams
        .contains(&team_slug)
        .then(|| team.members.get(&key.member))
        .flatten();
    let Some(rank) = rank else {
        return Err(refuse(
            "unknownTeam",
            format!("key {} does not open team {team_slug}", key.id),
        ));
    };
    match asked {
        Asked::Upload(team_slug) => {
            if team.mode == Some(Mode::Memory) && !rank.runs_the_team() {
                return Err(refuse(
                    "exportDenied",
                    format!(
                        "{team_slug} is a memory team: only its owner or an admin exports its store"
                    ),
                ));
            }
            Ok(ShellAction::Upload { team: team_slug })
        }
        Asked::Receive(team_slug) => {
            // the personal store takes its owner's machines' pushes, as a sync team takes its members'
            if team.mode != Some(Mode::Sync) && !team.adopted {
                return Err(refuse(
                    "pushDenied",
                    format!(
                        "{team_slug} is a memory team: only the memory server writes its store"
                    ),
                ));
            }
            Ok(ShellAction::Receive { team: team_slug })
        }
        Asked::Mcp(team_slug, _) if team.adopted => Err(refuse(
            "mcpDenied",
            format!(
                "{team_slug} is the personal store: its owner's engine keeps its memory, a machine key reaches it over git"
            ),
        )),
        Asked::Mcp(team_slug, agent) => Ok(ShellAction::Mcp {
            team: team_slug,
            agent,
        }),
        Asked::Status => Ok(ShellAction::Status),
    }
}

/// Reads the command into one of the four shapes, or nothing.
fn parse(original: &str, teams_dir: &str) -> Option<Asked> {
    if original == "status" {
        return Some(Asked::Status);
    }
    if let Some(path) = original.strip_prefix("git-upload-pack ") {
        return team_of_path(path, teams_dir).map(Asked::Upload);
    }
    if let Some(path) = original.strip_prefix("git-receive-pack ") {
        return team_of_path(path, teams_dir).map(Asked::Receive);
    }
    let words: Vec<&str> = original.split(' ').collect();
    if let ["mcp", "--team", team, "--agent", agent] = words.as_slice()
        && is_name(team)
        && is_name(agent)
    {
        return Some(Asked::Mcp((*team).to_owned(), (*agent).to_owned()));
    }
    None
}

/// The team a store path names: `teams/<slug>.git` or `<teams_dir>/<slug>.git`, in single quotes
/// or bare. The slug follows the name rule, which already leaves no room for `..`, `/`, quotes or
/// anything a shell would read; a deleted team's renamed directory is no store to serve.
fn team_of_path(path: &str, teams_dir: &str) -> Option<String> {
    let path = path
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(path);
    let absolute = format!("{}/", teams_dir.trim_end_matches('/'));
    let file = path
        .strip_prefix(TEAMS_PREFIX)
        .or_else(|| path.strip_prefix(absolute.as_str()))?;
    match team_dir(file)? {
        TeamDir::Store(slug) => Some(slug),
        TeamDir::Deleted { .. } => None,
    }
}
