//! The host's own console for the access snapshot: teams, members, projects, agent tokens and machine
//! keys of a host without the cabinet.
//!
//! The cabinet is the start0 pack, which is source-available and cannot ship with an open product, so
//! a host set up by its owner is run from here. The console writes exactly the snapshot the cabinet
//! writes — version 2, a serial that grows with every change — and every change goes through
//! [`crate::access::check`] before it may be written: the console cannot put on the host anything the
//! host would refuse.
//!
//! Pure: a snapshot document and an operation in, the changed document and what to hand to a person
//! out. Randomness and today's date come from the caller, the file from `main`.

use serde_json::{Map, Value, json};

use crate::access;

/// The version of the snapshot the console writes: the cabinet's, with a serial and bans.
const VERSION: u64 = 2;

/// Characters of a public id: Crockford's base32, lowercase.
const ID_ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Characters of a public id.
const ID_LENGTH: usize = 8;

/// Characters of a token's secret: 52 base32 characters, 260 bits, as the cabinet mints them.
const SECRET_LENGTH: usize = 52;

/// The id the cabinet keeps for its probes and never hands out; the console keeps away from it too.
const PROBE_ID: &str = "00000000";

/// Limits of a new team, as the cabinet's defaults.
pub const DEFAULT_MAX_RECORDS: u64 = 5000;
/// Bytes of one record of a new team.
pub const DEFAULT_MAX_RECORD_BYTES: u64 = 65_536;
/// Room of a new team: a self-hosted team has the host's disk, and the quota is only a guard.
pub const DEFAULT_QUOTA_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// What the console needs to know about the host to tell a machine where to go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    /// The host's domain: the memory server at `https://<domain>/mcp`, git over ssh at `<domain>`.
    pub domain: String,
    /// The account machine keys log in as.
    pub ssh_user: String,
    /// The host's own ssh keys, `ssh-ed25519 <base64>`, so a machine checks the host on first contact.
    pub host_keys: Vec<String>,
}

impl HostFacts {
    /// The address a grant names as where it came from: the host itself, in place of a cabinet.
    #[must_use]
    pub fn address(&self) -> String {
        format!("https://{}", self.domain)
    }
}

/// A role of a member in a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Runs the team with the owner.
    Admin,
    /// Everyone else.
    Member,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// One change of the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// A new team with its owner.
    TeamAdd {
        /// Its slug.
        slug: String,
        /// The owner's handle.
        owner: String,
        /// `true` for `sync` (sessions on), `false` for `memory`.
        sessions: bool,
        /// Its room in bytes.
        quota_bytes: u64,
    },
    /// The team is deleted: its store is renamed aside by the host, its tokens and keys go.
    TeamRemove {
        /// Its slug.
        slug: String,
    },
    /// Sessions of a team on (`sync`) or off (`memory`).
    TeamSessions {
        /// Its slug.
        slug: String,
        /// On or off.
        on: bool,
    },
    /// A team's room.
    TeamQuota {
        /// Its slug.
        slug: String,
        /// Bytes.
        quota_bytes: u64,
    },
    /// A project of a `memory` team.
    ProjectAdd {
        /// The team.
        team: String,
        /// The project's directory name.
        name: String,
    },
    /// A project of a `memory` team goes from its list.
    ProjectRemove {
        /// The team.
        team: String,
        /// The project's directory name.
        name: String,
    },
    /// A member joins a team.
    MemberAdd {
        /// The team.
        team: String,
        /// The handle.
        handle: String,
        /// Admin or member.
        role: Role,
    },
    /// A member's role changes.
    MemberRole {
        /// The team.
        team: String,
        /// The handle.
        handle: String,
        /// The new role.
        role: Role,
    },
    /// A member leaves a team, with their tokens of it; their machine keys stop opening it.
    MemberRemove {
        /// The team.
        team: String,
        /// The handle.
        handle: String,
    },
    /// A token for one agent of one member.
    TokenIssue {
        /// The team.
        team: String,
        /// The member.
        member: String,
        /// The agent.
        agent: String,
        /// Reads only.
        reader: bool,
        /// May search past sessions; only an owner or an admin.
        history: bool,
        /// `None`: every project of the team.
        projects: Option<Vec<String>>,
        /// `YYYY-MM-DDTHH:MM:SSZ`, or `None` for good.
        expires_at: Option<String>,
    },
    /// A token goes.
    TokenRevoke {
        /// Its id, `tk_…`.
        id: String,
    },
    /// A machine key of a member, for the teams it opens.
    KeyAdd {
        /// The member.
        member: String,
        /// The machine's name at the member.
        machine: String,
        /// The teams; the first one is the store the grant sets the machine up for.
        teams: Vec<String>,
        /// `ssh-ed25519 <base64>`.
        public_key: String,
    },
    /// A machine key goes.
    KeyRevoke {
        /// Its id, `mk_…`.
        id: String,
    },
}

/// What a change hands to a person besides the new snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to hand over.
    Done(String),
    /// A grant for `vibememory connect --grant` on the member's machine: one line of JSON. A token
    /// grant holds the token, which exists nowhere else — the snapshot keeps only its digest.
    Grant {
        /// What was made, for the person.
        said: String,
        /// The grant, as the cabinet's claim answer.
        grant: String,
    },
}

/// A new, empty snapshot.
#[must_use]
pub fn empty() -> Value {
    json!({ "version": VERSION, "serial": 0, "teamCount": 0, "teams": {}, "tokens": [], "keys": [], "bans": [] })
}

/// Applies one change to a snapshot document and checks the result as the host will.
///
/// `random` fills a buffer with random bytes; `today` is `YYYY-MM-DD`. A hand-written snapshot
/// (version 1) becomes the console's version 2 on its first change.
///
/// # Errors
///
/// A sentence for the person: a team, member, token or key that is not there, a change the spec
/// forbids, or the code of the first rule the changed snapshot breaks.
pub fn apply(
    document: &Value,
    operation: &Operation,
    facts: &HostFacts,
    today: &str,
    random: &mut dyn FnMut(&mut [u8]),
) -> Result<(Value, Outcome), String> {
    let mut snapshot = document
        .as_object()
        .cloned()
        .ok_or_else(|| "the snapshot is not a JSON object".to_owned())?;
    let outcome = change(&mut snapshot, operation, facts, today, random)?;
    let serial = snapshot.get("serial").and_then(Value::as_u64).unwrap_or(0) + 1;
    snapshot.insert("version".to_owned(), VERSION.into());
    snapshot.insert("serial".to_owned(), serial.into());
    snapshot
        .entry("bans")
        .or_insert_with(|| Value::Array(Vec::new()));
    let teams = teams(&snapshot)?.len() as u64;
    let counted = snapshot
        .get("teamCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    snapshot.insert("teamCount".to_owned(), counted.max(teams).into());
    let changed = Value::Object(snapshot);
    let bytes = render(&changed);
    access::check(bytes.as_bytes())
        .map_err(|refusal| format!("{}: {}", refusal.code, refusal.detail))?;
    Ok((changed, outcome))
}

/// The snapshot as it is written: pretty JSON with a final newline.
#[must_use]
pub fn render(snapshot: &Value) -> String {
    let mut text = serde_json::to_string_pretty(snapshot).unwrap_or_default();
    text.push('\n');
    text
}

fn change(
    snapshot: &mut Map<String, Value>,
    operation: &Operation,
    facts: &HostFacts,
    today: &str,
    random: &mut dyn FnMut(&mut [u8]),
) -> Result<Outcome, String> {
    match operation {
        Operation::TeamAdd {
            slug,
            owner,
            sessions,
            quota_bytes,
        } => team_add(snapshot, slug, owner, *sessions, *quota_bytes),
        Operation::TeamRemove { slug } => team_remove(snapshot, slug, today),
        Operation::TeamSessions { slug, on } => team_sessions(snapshot, slug, *on),
        Operation::TeamQuota { slug, quota_bytes } => team_quota(snapshot, slug, *quota_bytes),
        Operation::ProjectAdd { team, name } => project(snapshot, team, name, true),
        Operation::ProjectRemove { team, name } => project(snapshot, team, name, false),
        Operation::MemberAdd { team, handle, role } => member_add(snapshot, team, handle, *role),
        Operation::MemberRole { team, handle, role } => member_role(snapshot, team, handle, *role),
        Operation::MemberRemove { team, handle } => member_remove(snapshot, team, handle),
        Operation::TokenIssue { .. } => token_issue(snapshot, operation, facts, random),
        Operation::TokenRevoke { id } => token_revoke(snapshot, id),
        Operation::KeyAdd {
            member,
            machine,
            teams: opened,
            public_key,
        } => key_add(snapshot, facts, member, machine, opened, public_key, random),
        Operation::KeyRevoke { id } => key_revoke(snapshot, id),
    }
}

fn team_add(
    snapshot: &mut Map<String, Value>,
    slug: &str,
    owner: &str,
    sessions: bool,
    quota_bytes: u64,
) -> Result<Outcome, String> {
    if teams(snapshot)?.contains_key(slug) {
        return Err(format!(
            "team {slug} exists already, deleted ones keep their name for good"
        ));
    }
    let mut team = Map::new();
    team.insert("mode".to_owned(), mode_name(sessions).into());
    team.insert("writable".to_owned(), true.into());
    team.insert(
        "limits".to_owned(),
        json!({
            "maxRecords": DEFAULT_MAX_RECORDS,
            "maxRecordBytes": DEFAULT_MAX_RECORD_BYTES,
            "quotaBytes": quota_bytes,
        }),
    );
    team.insert("members".to_owned(), json!({ owner: "owner" }));
    if !sessions {
        team.insert("projects".to_owned(), json!([]));
    }
    teams_mut(snapshot)?.insert(slug.to_owned(), Value::Object(team));
    Ok(Outcome::Done(format!(
        "team {slug} is made, {owner} owns it, sessions {}",
        on_off(sessions)
    )))
}

fn team_remove(
    snapshot: &mut Map<String, Value>,
    slug: &str,
    today: &str,
) -> Result<Outcome, String> {
    let team = live_team_mut(snapshot, slug)?;
    if team.get("adopted").and_then(Value::as_bool) == Some(true) {
        return Err(format!(
            "{slug} is the owner's personal store: it is not deleted"
        ));
    }
    team.insert("deleted".to_owned(), today.into());
    team.insert("writable".to_owned(), false.into());
    team.insert("members".to_owned(), json!({}));
    team.insert("limits".to_owned(), json!({}));
    if team.contains_key("projects") {
        team.insert("projects".to_owned(), json!([]));
    }
    retain_tokens(snapshot, |token| token_team(token) != slug);
    strip_team_from_keys(snapshot, slug, None);
    Ok(Outcome::Done(format!(
        "team {slug} is deleted: the host renames its store aside and keeps it; its tokens and keys are gone"
    )))
}

fn team_sessions(
    snapshot: &mut Map<String, Value>,
    slug: &str,
    on: bool,
) -> Result<Outcome, String> {
    let team = live_team_mut(snapshot, slug)?;
    refuse_adopted(team, slug)?;
    team.insert("mode".to_owned(), mode_name(on).into());
    if on {
        team.remove("projects");
    } else {
        team.entry("projects").or_insert_with(|| json!([]));
    }
    Ok(Outcome::Done(if on {
        format!("sessions of {slug} are on: machines connect with keys")
    } else {
        format!(
            "sessions of {slug} are off: the host rewrites the store to its memory, and the sessions leave the server"
        )
    }))
}

fn team_quota(
    snapshot: &mut Map<String, Value>,
    slug: &str,
    quota_bytes: u64,
) -> Result<Outcome, String> {
    let team = live_team_mut(snapshot, slug)?;
    refuse_adopted(team, slug)?;
    let limits = team
        .entry("limits")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("the limits of {slug} are not an object"))?;
    limits.insert("quotaBytes".to_owned(), quota_bytes.into());
    Ok(Outcome::Done(format!(
        "{slug} has {} MB now",
        quota_bytes / 1_048_576
    )))
}

fn project(
    snapshot: &mut Map<String, Value>,
    team: &str,
    name: &str,
    adding: bool,
) -> Result<Outcome, String> {
    let entry = live_team_mut(snapshot, team)?;
    let projects = entry
        .get_mut("projects")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| {
            format!(
                "{team} keeps its projects in its store, not in a list: only a team with sessions off has one"
            )
        })?;
    let present = projects
        .iter()
        .any(|project| project.as_str() == Some(name));
    if adding {
        if present {
            return Err(format!("{team} has project {name} already"));
        }
        projects.push(name.into());
    } else {
        if !present {
            return Err(format!("{team} has no project {name}"));
        }
        projects.retain(|project| project.as_str() != Some(name));
    }
    Ok(Outcome::Done(format!(
        "{team}: project {name} {}",
        if adding {
            "added"
        } else {
            "removed from the list; its records stay in the store"
        }
    )))
}

fn member_add(
    snapshot: &mut Map<String, Value>,
    team: &str,
    handle: &str,
    role: Role,
) -> Result<Outcome, String> {
    let members = members_mut(live_team_mut(snapshot, team)?, team)?;
    if members.contains_key(handle) {
        return Err(format!("{handle} is in {team} already"));
    }
    members.insert(handle.to_owned(), role.name().into());
    Ok(Outcome::Done(format!(
        "{handle} joined {team} as {}",
        role.name()
    )))
}

/// The rank of a member who may change: not there, or the owner, refuses.
fn changeable(members: &Map<String, Value>, team: &str, handle: &str) -> Result<(), String> {
    match members.get(handle).and_then(Value::as_str) {
        None => Err(format!("{handle} is not in {team}")),
        Some("owner") => Err(format!(
            "{handle} owns {team}: a team keeps exactly one owner"
        )),
        Some(_) => Ok(()),
    }
}

fn member_role(
    snapshot: &mut Map<String, Value>,
    team: &str,
    handle: &str,
    role: Role,
) -> Result<Outcome, String> {
    let members = members_mut(live_team_mut(snapshot, team)?, team)?;
    changeable(members, team, handle)?;
    members.insert(handle.to_owned(), role.name().into());
    if role == Role::Member {
        // past sessions are for those who run the team
        for token in tokens_mut(snapshot)? {
            if token_team(token) == team
                && token_member(token) == handle
                && let Some(fields) = token.as_object_mut()
            {
                fields.insert("history".to_owned(), false.into());
            }
        }
    }
    Ok(Outcome::Done(format!(
        "{handle} is {} of {team}",
        role.name()
    )))
}

fn member_remove(
    snapshot: &mut Map<String, Value>,
    team: &str,
    handle: &str,
) -> Result<Outcome, String> {
    let members = members_mut(live_team_mut(snapshot, team)?, team)?;
    changeable(members, team, handle)?;
    members.remove(handle);
    retain_tokens(snapshot, |token| {
        !(token_team(token) == team && token_member(token) == handle)
    });
    strip_team_from_keys(snapshot, team, Some(handle));
    Ok(Outcome::Done(format!(
        "{handle} left {team}: their tokens of it are revoked, their machines no longer open it"
    )))
}

fn token_issue(
    snapshot: &mut Map<String, Value>,
    operation: &Operation,
    facts: &HostFacts,
    random: &mut dyn FnMut(&mut [u8]),
) -> Result<Outcome, String> {
    let Operation::TokenIssue {
        team,
        member,
        agent,
        reader,
        history,
        projects,
        expires_at,
    } = operation
    else {
        return Err("not a token".to_owned());
    };
    live_team_mut(snapshot, team)?;
    let id = fresh_id(snapshot, "tokens", "tk_", random);
    let public = id.trim_start_matches("tk_");
    let secret = random_text(SECRET_LENGTH, random);
    let token = format!("{}{public}_{secret}", vibememory_core::token::PREFIX);
    let digest = vibememory_cli::sha256::hex(token.as_bytes());
    tokens_mut(snapshot)?.push(json!({
        "id": id,
        "sha256": digest,
        "team": team,
        "member": member,
        "agent": agent,
        "role": if *reader { "reader" } else { "writer" },
        "history": history,
        "projects": projects,
        "expiresAt": expires_at,
    }));
    let grant = json!({
        "version": 1,
        "kind": "token",
        "cabinet": facts.address(),
        "team": team,
        "member": member,
        "agent": agent,
        "tokenId": id,
        "token": token,
        "mcpUrl": format!("{}/mcp", facts.address()),
    });
    Ok(Outcome::Grant {
        said: format!("token {id} for {agent} of {member} in {team}"),
        grant: grant.to_string(),
    })
}

fn token_revoke(snapshot: &mut Map<String, Value>, id: &str) -> Result<Outcome, String> {
    let before = tokens_mut(snapshot)?.len();
    retain_tokens(snapshot, |token| {
        token.get("id").and_then(Value::as_str) != Some(id)
    });
    if tokens_mut(snapshot)?.len() == before {
        return Err(format!("no token {id}"));
    }
    Ok(Outcome::Done(format!(
        "token {id} is revoked: its next request is refused"
    )))
}

fn key_add(
    snapshot: &mut Map<String, Value>,
    facts: &HostFacts,
    member: &str,
    machine: &str,
    opened: &[String],
    public_key: &str,
    random: &mut dyn FnMut(&mut [u8]),
) -> Result<Outcome, String> {
    let Some(first) = opened.first() else {
        return Err("a key opens at least one team".to_owned());
    };
    let mode = {
        let team = live_team_mut(snapshot, first)?;
        if team.get("adopted").and_then(Value::as_bool) == Some(true) {
            vibememory_core::team_store::PERSONAL_MODE.to_owned()
        } else {
            team.get("mode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        }
    };
    if mode == "memory" {
        return Err(format!(
            "sessions of {first} are off: a machine key has nothing to push — turn them on first"
        ));
    }
    let id = fresh_id(snapshot, "keys", "mk_", random);
    let store_name = format!("{member}-{machine}");
    keys_mut(snapshot)?.push(json!({
        "id": id,
        "member": member,
        "machine": machine,
        "storeName": store_name,
        "teams": opened,
        "publicKey": public_key,
    }));
    let grant = json!({
        "version": 1,
        "kind": "key",
        "cabinet": facts.address(),
        "team": first,
        "mode": mode,
        "member": member,
        "machine": machine,
        "storeName": store_name,
        "keyId": id,
        "sshHost": facts.domain,
        "sshUser": facts.ssh_user,
        "remote": format!("teams/{first}.git"),
        "hostKeys": facts.host_keys,
    });
    Ok(Outcome::Grant {
        said: format!(
            "key {id} of {member}'s {machine}, for {}",
            opened.join(", ")
        ),
        grant: grant.to_string(),
    })
}

fn key_revoke(snapshot: &mut Map<String, Value>, id: &str) -> Result<Outcome, String> {
    let keys = keys_mut(snapshot)?;
    let before = keys.len();
    keys.retain(|key| key.get("id").and_then(Value::as_str) != Some(id));
    if keys.len() == before {
        return Err(format!("no key {id}"));
    }
    Ok(Outcome::Done(format!(
        "key {id} is revoked: the machine is refused from its next connection"
    )))
}

fn mode_name(sessions: bool) -> &'static str {
    if sessions { "sync" } else { "memory" }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn teams(snapshot: &Map<String, Value>) -> Result<&Map<String, Value>, String> {
    snapshot
        .get("teams")
        .and_then(Value::as_object)
        .ok_or_else(|| "the snapshot has no teams object".to_owned())
}

fn teams_mut(snapshot: &mut Map<String, Value>) -> Result<&mut Map<String, Value>, String> {
    snapshot
        .get_mut("teams")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "the snapshot has no teams object".to_owned())
}

fn tokens_mut(snapshot: &mut Map<String, Value>) -> Result<&mut Vec<Value>, String> {
    snapshot
        .entry("tokens")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "tokens is not a list".to_owned())
}

fn keys_mut(snapshot: &mut Map<String, Value>) -> Result<&mut Vec<Value>, String> {
    snapshot
        .entry("keys")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| "keys is not a list".to_owned())
}

/// A team that exists and is not deleted.
fn live_team_mut<'a>(
    snapshot: &'a mut Map<String, Value>,
    slug: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let team = teams_mut(snapshot)?
        .get_mut(slug)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("no team {slug}"))?;
    if team.contains_key("deleted") {
        return Err(format!("team {slug} is deleted"));
    }
    Ok(team)
}

fn refuse_adopted(team: &Map<String, Value>, slug: &str) -> Result<(), String> {
    if team.get("adopted").and_then(Value::as_bool) == Some(true) {
        return Err(format!(
            "{slug} is the owner's personal store: it has no mode and no quota"
        ));
    }
    Ok(())
}

fn members_mut<'a>(
    team: &'a mut Map<String, Value>,
    slug: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    team.get_mut("members")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| format!("the members of {slug} are not an object"))
}

fn token_team(token: &Value) -> &str {
    token
        .get("team")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn token_member(token: &Value) -> &str {
    token
        .get("member")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn retain_tokens(snapshot: &mut Map<String, Value>, keep: impl Fn(&Value) -> bool) {
    if let Ok(tokens) = tokens_mut(snapshot) {
        tokens.retain(|token| keep(token));
    }
}

/// Takes `team` out of the keys of `member` (of every member when `None`); a key left with no team
/// goes, since a key opens at least one.
fn strip_team_from_keys(snapshot: &mut Map<String, Value>, team: &str, member: Option<&str>) {
    let Ok(keys) = keys_mut(snapshot) else {
        return;
    };
    for key in keys.iter_mut() {
        let owner = key
            .get("member")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if member.is_some_and(|member| member != owner) {
            continue;
        }
        if let Some(teams) = key.get_mut("teams").and_then(Value::as_array_mut) {
            teams.retain(|opened| opened.as_str() != Some(team));
        }
    }
    keys.retain(|key| {
        key.get("teams")
            .and_then(Value::as_array)
            .is_some_and(|teams| !teams.is_empty())
    });
}

/// A public id with `prefix` that no entry of `list` carries, never the probe id.
fn fresh_id(
    snapshot: &Map<String, Value>,
    list: &str,
    prefix: &str,
    random: &mut dyn FnMut(&mut [u8]),
) -> String {
    let taken = |id: &str| {
        snapshot
            .get(list)
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry.get("id").and_then(Value::as_str) == Some(id))
            })
    };
    loop {
        let public = random_text(ID_LENGTH, random);
        let id = format!("{prefix}{public}");
        if public != PROBE_ID && !taken(&id) {
            return id;
        }
    }
}

/// `length` characters of the id alphabet. 256 is a multiple of the alphabet's 32, so taking a byte
/// modulo the alphabet favours no character.
fn random_text(length: usize, random: &mut dyn FnMut(&mut [u8])) -> String {
    let mut bytes = vec![0u8; length];
    random(&mut bytes);
    bytes
        .iter()
        .map(|byte| {
            char::from(
                ID_ALPHABET
                    .get(usize::from(*byte) % ID_ALPHABET.len())
                    .copied()
                    .unwrap_or(b'0'),
            )
        })
        .collect()
}

/// The snapshot in words, for `admin show`: teams with their members and projects, then tokens and
/// keys — ids and names, never a digest.
#[must_use]
pub fn describe(snapshot: &Value) -> String {
    let mut lines = Vec::new();
    let serial = snapshot.get("serial").and_then(Value::as_u64).unwrap_or(0);
    lines.push(format!("snapshot serial {serial}"));
    if let Some(teams) = snapshot.get("teams").and_then(Value::as_object) {
        for (slug, team) in teams {
            let kind = if team.get("adopted").and_then(Value::as_bool) == Some(true) {
                "personal store".to_owned()
            } else if let Some(day) = team.get("deleted").and_then(Value::as_str) {
                format!("deleted {day}")
            } else {
                let mode = team.get("mode").and_then(Value::as_str).unwrap_or("?");
                let quota = team
                    .pointer("/limits/quotaBytes")
                    .and_then(Value::as_u64)
                    .map_or_else(String::new, |bytes| format!(", {} MB", bytes / 1_048_576));
                format!("sessions {}{quota}", on_off(mode == "sync"))
            };
            lines.push(format!("team {slug} — {kind}"));
            if let Some(members) = team.get("members").and_then(Value::as_object) {
                for (handle, rank) in members {
                    lines.push(format!("  {handle}: {}", rank.as_str().unwrap_or("?")));
                }
            }
            if let Some(projects) = team.get("projects").and_then(Value::as_array) {
                let names: Vec<&str> = projects.iter().filter_map(Value::as_str).collect();
                lines.push(format!(
                    "  projects: {}",
                    if names.is_empty() {
                        "none".to_owned()
                    } else {
                        names.join(", ")
                    }
                ));
            }
        }
    }
    for token in snapshot
        .get("tokens")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |name: &str| {
            token
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned()
        };
        let expires = token
            .get("expiresAt")
            .and_then(Value::as_str)
            .map_or_else(String::new, |until| format!(", until {until}"));
        lines.push(format!(
            "token {} — {} of {} in {}, {}{}{expires}",
            field("id"),
            field("agent"),
            field("member"),
            field("team"),
            field("role"),
            if token.get("history").and_then(Value::as_bool) == Some(true) {
                ", history"
            } else {
                ""
            },
        ));
    }
    for key in snapshot
        .get("keys")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |name: &str| {
            key.get(name)
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_owned()
        };
        let opened: Vec<&str> = key
            .get("teams")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        lines.push(format!(
            "key {} — {}'s {}, opens {}",
            field("id"),
            field("member"),
            field("machine"),
            opened.join(", ")
        ));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}
