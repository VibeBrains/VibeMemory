//! The host's report: `host.json` for the cabinet, and the part of it a machine key may see.
//!
//! Pure: the snapshot, what the last application did and what the host found on disk in; the
//! report out. The shapes are those of `docs/manuals/hostStatusSpec.md`. Reading a report back is
//! strict — a field the reader does not know is refused, because an extended format raises
//! `version`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::access::{Mode, Rank, Snapshot};
use crate::layout::{TeamDir, team_dir};

/// The only format version there is.
pub const VERSION: u64 = 1;

/// Something the application of a snapshot did not do, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Problem {
    /// Stable camelCase code from the spec.
    pub code: String,
    /// The team it is about, when it is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// What exactly, for a person.
    pub detail: String,
}

/// `applied.json`: the last snapshot applied, and what was not done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Applied {
    /// Format version.
    pub version: u64,
    /// `serial` of the snapshot applied last. A hand-written snapshot has none: then the field is
    /// not written, and a file without it reads as 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub serial: u64,
    /// SHA-256 of the bytes of the snapshot applied last.
    pub snapshot_hash: String,
    /// When it was applied.
    pub applied_at: String,
    /// `teamCount` of that snapshot.
    pub team_count: u64,
    /// What was not done, and why.
    pub problems: Vec<Problem>,
    /// The last snapshot refused since, if any: the catch-up does not refuse the same bytes again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refused: Option<Refused>,
}

/// A refused application: of which bytes, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Refused {
    /// SHA-256 of the refused `access.json`; `None` when there was no file to read.
    pub snapshot_hash: Option<String>,
    /// The code of the refusal, as it stands first in `problems`.
    pub code: String,
}

/// Whether a serial is the absent one of a hand-written snapshot.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde's `skip_serializing_if` passes a reference
const fn is_zero(serial: &u64) -> bool {
    *serial == 0
}

/// What the host finds in one repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepoFacts {
    /// Size on disk.
    pub size_bytes: u64,
    /// The bytes of the files in `main`: what the team's quota counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_bytes: Option<u64>,
    /// The directories under `projects/` of `main`.
    pub projects: Vec<String>,
    /// The time of the last commit of `main`; `None` without one.
    pub last_commit_at: Option<String>,
    /// Whether `main` has a `machines/` directory: a hint that the team synced clones.
    pub machines: bool,
    /// What each project of `main` holds and who wrote to it last.
    #[serde(default)]
    pub project_facts: BTreeMap<String, ProjectFacts>,
    /// When the memory server was last asked for the team, reads included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_access_at: Option<String>,
}

/// One project of a store: what the owner of a team looks at to tell a live project from a stale one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectFacts {
    /// The bytes of its files in `main`, not of their history.
    pub size_bytes: u64,
    /// Of those, the bytes of its memory — the journal `memory.jsonl` and the `memory/` directory;
    /// the rest are its sessions. Absent in a report of an older host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// The time of the last commit of `main` that touched it.
    pub last_commit_at: Option<String>,
    /// Who made that commit: the machine's git author, or the writer the memory server commits as.
    pub last_author: Option<String>,
}

/// Free and total space of the partition the stores live on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Disk {
    /// Bytes free for an unprivileged writer.
    pub free_bytes: u64,
    /// Bytes of the partition.
    pub total_bytes: u64,
}

/// The nightly backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Backup {
    /// When it last finished.
    pub last_at: String,
}

/// Everything the host found, before it is matched against the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Facts {
    /// When the facts were gathered.
    pub generated_at: String,
    /// Every directory under `teams/`, by name.
    pub repos: BTreeMap<String, RepoFacts>,
    /// The adopted store of the snapshot, by slug, when its repository is where the snapshot says.
    pub adopted: BTreeMap<String, RepoFacts>,
    /// The host's public keys: type and base64, no comment.
    pub host_keys: Vec<String>,
    /// The partition of the stores.
    pub disk: Disk,
    /// `systemctl is-active` of each service.
    pub services: BTreeMap<String, String>,
    /// The last backup; `None` before the first.
    pub backup: Option<Backup>,
}

/// A team of the snapshot whose repository is on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamReport {
    /// The directories under `projects/` of `main`.
    pub projects: Vec<String>,
    /// Size on disk.
    pub size_bytes: u64,
    /// The bytes of the files in `main`: what the team's quota counts, and what the cabinet shows
    /// as used. Absent in a report of an older host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_bytes: Option<u64>,
    /// The time of the last commit of `main`.
    pub last_commit_at: Option<String>,
    /// What each project holds and who wrote to it last.
    #[serde(default)]
    pub project_facts: BTreeMap<String, ProjectFacts>,
    /// When the memory server was last asked for the team, reads included: with the last commit,
    /// what tells a team in use from an abandoned one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_access_at: Option<String>,
}

/// A deleted team's renamed directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deleted {
    /// The slug, from the name.
    pub slug: String,
    /// The day of the deletion, from the name.
    pub date: String,
    /// Size on disk.
    pub size_bytes: u64,
}

/// A deleted team's directory still under its live name: the application has not renamed it —
/// it will, or its problems say why it could not. Reported so that nobody takes the team for gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retiring {
    /// The slug, from the name.
    pub slug: String,
    /// Size on disk.
    pub size_bytes: u64,
}

/// A team directory the snapshot does not know.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Orphan {
    /// The slug, from the name.
    pub slug: String,
    /// The directories under `projects/` of `main`.
    pub projects: Vec<String>,
    /// Size on disk.
    pub size_bytes: u64,
    /// Whether `main` has a `machines/` directory: the team was probably `sync`.
    pub machines: bool,
}

/// `host.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostReport {
    /// Format version.
    pub version: u64,
    /// When the report was made.
    pub generated_at: String,
    /// `applied.json` as it is; `None` before the first application.
    pub applied: Option<Applied>,
    /// Teams of the snapshot, not deleted, whose repository is on the host.
    pub teams: BTreeMap<String, TeamReport>,
    /// Renamed directories of deleted teams.
    pub deleted: Vec<Deleted>,
    /// Deleted teams whose directory is not renamed yet. A report written before the field was
    /// added reads as none.
    #[serde(default)]
    pub retiring: Vec<Retiring>,
    /// Team directories the snapshot does not know.
    pub orphans: Vec<Orphan>,
    /// The host's public keys.
    pub host_keys: Vec<String>,
    /// The partition of the stores.
    pub disk: Disk,
    /// `systemctl is-active` of each service.
    pub services: BTreeMap<String, String>,
    /// The last backup; `None` before the first.
    pub backup: Option<Backup>,
}

/// Builds `host.json`. `snapshot` is `None` when the snapshot cannot be used: then the host cannot
/// tell its teams from orphans and lists neither, while a deleted team's directory still says what
/// it is by its name.
#[must_use]
pub fn host_report(
    snapshot: Option<&Snapshot>,
    applied: Option<Applied>,
    facts: Facts,
) -> HostReport {
    let mut teams = BTreeMap::new();
    let mut deleted = Vec::new();
    let mut retiring = Vec::new();
    let mut orphans = Vec::new();
    for (dir, repo) in facts.repos {
        match team_dir(&dir) {
            None => {}
            Some(TeamDir::Deleted { slug, date }) => deleted.push(Deleted {
                slug,
                date,
                size_bytes: repo.size_bytes,
            }),
            Some(TeamDir::Store(slug)) => {
                let Some(snapshot) = snapshot else {
                    continue;
                };
                match snapshot.teams.get(&slug) {
                    None => orphans.push(Orphan {
                        slug,
                        projects: repo.projects,
                        size_bytes: repo.size_bytes,
                        machines: repo.machines,
                    }),
                    Some(team) if team.deleted.is_none() && !team.adopted => {
                        teams.insert(
                            slug,
                            TeamReport {
                                projects: repo.projects,
                                size_bytes: repo.size_bytes,
                                tree_bytes: repo.tree_bytes,
                                last_commit_at: repo.last_commit_at,
                                project_facts: repo.project_facts,
                                last_access_at: repo.last_access_at,
                            },
                        );
                    }
                    // A deleted team whose directory is not renamed yet: the application renames
                    // it, or says in its problems why it could not.
                    Some(team) if team.deleted.is_some() => retiring.push(Retiring {
                        slug,
                        size_bytes: repo.size_bytes,
                    }),
                    // The adopted store lives where the snapshot says, not under `teams/`.
                    Some(_) => {}
                }
            }
        }
    }
    if let Some(snapshot) = snapshot {
        for (slug, repo) in facts.adopted {
            if snapshot
                .teams
                .get(&slug)
                .is_some_and(|team| team.adopted && team.deleted.is_none())
            {
                teams.insert(
                    slug,
                    TeamReport {
                        projects: repo.projects,
                        size_bytes: repo.size_bytes,
                        tree_bytes: repo.tree_bytes,
                        last_commit_at: repo.last_commit_at,
                        project_facts: repo.project_facts,
                        last_access_at: repo.last_access_at,
                    },
                );
            }
        }
    }
    deleted.sort_by(|a, b| (&a.slug, &a.date).cmp(&(&b.slug, &b.date)));
    retiring.sort_by(|a, b| a.slug.cmp(&b.slug));
    orphans.sort_by(|a, b| a.slug.cmp(&b.slug));
    HostReport {
        version: VERSION,
        generated_at: facts.generated_at,
        applied,
        teams,
        deleted,
        retiring,
        orphans,
        host_keys: facts.host_keys,
        disk: facts.disk,
        services: facts.services,
        backup: facts.backup,
    }
}

/// Whose key answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyIdentity {
    /// `mk_…`.
    pub id: String,
    /// The member's handle.
    pub member: String,
    /// The machine's label.
    pub machine: String,
    /// `<member>-<machine>`: the machine's directory and version prefix.
    pub store_name: String,
}

/// One team of the key, as its member sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyTeam {
    /// `memory` or `sync`.
    pub mode: Option<Mode>,
    /// The member's rank in the team.
    pub role: Rank,
    /// Whether writes are open.
    pub writable: bool,
    /// Size of the repository; `None` while the host has none.
    pub size_bytes: Option<u64>,
    /// The team's quota.
    pub quota_bytes: Option<u64>,
    /// The last commit of `main`; `None` while there is none.
    pub last_commit_at: Option<String>,
}

/// Free space, and nothing else about the host's disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyDisk {
    /// Bytes free for an unprivileged writer.
    pub free_bytes: u64,
}

/// The answer of `status` to a machine key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusAnswer {
    /// Format version.
    pub version: u64,
    /// When the report it comes from was made.
    pub generated_at: String,
    /// Whose key this is.
    pub key: KeyIdentity,
    /// `storeName` of every key of the same member, sorted.
    pub store_names: Vec<String>,
    /// The key's teams.
    pub teams: BTreeMap<String, KeyTeam>,
    /// The host's public keys.
    pub host_keys: Vec<String>,
    /// Free space.
    pub disk: KeyDisk,
}

/// The report as `key` may see it: its own teams and the host's keys and free space, nothing of
/// any other team. `None` for a key the snapshot does not hold.
#[must_use]
pub fn answer(snapshot: &Snapshot, report: &HostReport, key: &str) -> Option<StatusAnswer> {
    let key = snapshot.key(key)?;
    let mut store_names: Vec<String> = snapshot
        .keys
        .iter()
        .filter(|other| other.member == key.member)
        .map(|other| other.store_name.clone())
        .collect();
    store_names.sort();
    let teams = key
        .teams
        .iter()
        .filter_map(|slug| {
            let team = snapshot
                .teams
                .get(slug)
                .filter(|team| team.deleted.is_none())?;
            let role = *team.members.get(&key.member)?;
            let found = report.teams.get(slug);
            Some((
                slug.clone(),
                KeyTeam {
                    mode: team.mode,
                    role,
                    writable: team.writable,
                    size_bytes: found.map(|found| found.size_bytes),
                    quota_bytes: team.limits.quota_bytes,
                    last_commit_at: found.and_then(|found| found.last_commit_at.clone()),
                },
            ))
        })
        .collect();
    Some(StatusAnswer {
        version: VERSION,
        generated_at: report.generated_at.clone(),
        key: KeyIdentity {
            id: key.id.clone(),
            member: key.member.clone(),
            machine: key.machine.clone(),
            store_name: key.store_name.clone(),
        },
        store_names,
        teams,
        host_keys: report.host_keys.clone(),
        disk: KeyDisk {
            free_bytes: report.disk.free_bytes,
        },
    })
}
