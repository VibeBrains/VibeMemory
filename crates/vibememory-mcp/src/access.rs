//! The access snapshot: which teams exist, and which tokens and machine keys may use them.
//!
//! Pure: bytes in, a checked snapshot or the first rule it breaks out. The rules and their codes
//! are the ones `docs/manuals/accessSnapshotSpec.md` lists, checked in the order it lists them —
//! the cabinet runs every snapshot through this before publishing it and shows the code to a
//! person, so a renamed code breaks the cabinet as surely as a renamed field.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Number;
use vibememory_core::naming::StoreName;
use vibememory_core::token;

/// The only format version there is.
const VERSION: u64 = 1;
/// Public id of a token in the snapshot: this prefix and the id the token string carries.
const TOKEN_ID_PREFIX: &str = "tk_";
/// Public id of a machine key: this prefix and an id of the same alphabet.
const KEY_ID_PREFIX: &str = "mk_";
/// The id of the token issued before the cabinet, which has no id of its own.
pub const LEGACY_TOKEN_ID: &str = "tk_legacy";
/// Longest team slug, handle, machine or agent name: one DNS label.
const MAX_NAME_LENGTH: usize = 63;
/// The only machine key type accepted.
const ED25519: &str = "ssh-ed25519";
/// Bytes of an ed25519 public key.
const ED25519_KEY_BYTES: usize = 32;

/// What kind of store a team keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Only the memory server: members have no clone.
    Memory,
    /// A clone of the store on every member's machine, transcripts included.
    Sync,
}

/// A member's rank in a team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rank {
    /// The one who answers for the team; exactly one per team.
    Owner,
    /// Runs the team with the owner.
    Admin,
    /// Everyone else.
    Member,
}

impl Rank {
    /// Whether this rank runs the team: an owner or an admin may export a memory team's store and
    /// search past sessions.
    #[must_use]
    pub fn runs_the_team(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// What a token may do with the memory it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenRole {
    /// Reads only.
    Reader,
    /// Reads and writes.
    Writer,
}

/// The first rule a snapshot breaks: the spec's code, and what exactly is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// Stable camelCase code from the spec.
    pub code: &'static str,
    /// What is wrong, for a person.
    pub detail: String,
}

fn refuse(code: &'static str, detail: String) -> Refusal {
    Refusal { code, detail }
}

/// A team's limits. `None` is "not set", which only a deleted team may leave its limits at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Records of memory in one project.
    pub max_records: Option<u64>,
    /// Bytes of one record.
    pub max_record_bytes: Option<u64>,
    /// Size of the team's repository.
    pub quota_bytes: Option<u64>,
    /// Members of the team.
    pub seats: Option<u64>,
}

/// A team as the snapshot describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    /// The owner's personal store, which existed before teams.
    pub adopted: bool,
    /// Path of the adopted store's repository; `None` for every other team.
    pub repo: Option<String>,
    /// `None` only for the adopted store.
    pub mode: Option<Mode>,
    /// `false`: the grant is over or the team is deleted, and writes are refused.
    pub writable: bool,
    /// The day the team was deleted.
    pub deleted: Option<String>,
    /// The projects of a `memory` team; `None` for any other.
    pub projects: Option<Vec<String>>,
    /// Its limits.
    pub limits: Limits,
    /// Handle to rank.
    pub members: BTreeMap<String, Rank>,
}

impl Team {
    /// The bare repository of the team: the adopted store where it is, every other team under
    /// `teams_dir` by its slug.
    #[must_use]
    pub fn repository(&self, slug: &str, teams_dir: &Path) -> PathBuf {
        self.repo
            .as_ref()
            .map_or_else(|| teams_dir.join(format!("{slug}.git")), PathBuf::from)
    }
}

/// An HTTPS token of one member for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// `tk_` and eight characters, or `tk_legacy`.
    pub id: String,
    /// The token issued before the cabinet.
    pub legacy: bool,
    /// SHA-256 of the whole token string, lowercase hex.
    pub sha256: String,
    /// The team's slug.
    pub team: String,
    /// The member's handle.
    pub member: String,
    /// Who writes with it: `claude-code`, `vibeide`…
    pub agent: String,
    /// Reader or writer.
    pub role: TokenRole,
    /// Whether it may search past sessions.
    pub history: bool,
    /// `None`: every project of the team.
    pub projects: Option<Vec<String>>,
    /// When it stops working, as `YYYY-MM-DDTHH:MM:SSZ`.
    pub expires_at: Option<String>,
}

/// An ssh key of one machine of one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// `mk_` and eight characters.
    pub id: String,
    /// The member's handle.
    pub member: String,
    /// The machine's name at this member.
    pub machine: String,
    /// `<member>-<machine>`: the machine's directory in a team store and the prefix of its versions.
    pub store_name: String,
    /// The teams it opens.
    pub teams: Vec<String>,
    /// `ssh-ed25519 <base64>`.
    pub public_key: String,
}

/// A snapshot that passed every rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Teams the cabinet knew of when it wrote this, deleted ones included.
    pub team_count: u64,
    /// Slug to team.
    pub teams: BTreeMap<String, Team>,
    /// HTTPS tokens.
    pub tokens: Vec<Token>,
    /// Machine keys.
    pub keys: Vec<Key>,
    /// Token id to its position: a token is found by id, never by trying every digest.
    by_id: HashMap<String, usize>,
    /// Position of the legacy token, when there is one.
    legacy: Option<usize>,
}

/// Who a presented token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission<'a> {
    /// No token matches: unknown, revoked, or none presented.
    Unknown,
    /// A token that matched, and whose time is over. The door answers it exactly as it answers an
    /// unknown one; only the journal line differs, because a known client is not an attack.
    Expired(&'a Token),
    /// A token that matched and still works.
    Granted(&'a Token),
}

impl Snapshot {
    /// The machine key with this id.
    #[must_use]
    pub fn key(&self, id: &str) -> Option<&Key> {
        self.keys.iter().find(|key| key.id == id)
    }

    /// Who `presented` is, at `now` (`YYYY-MM-DDTHH:MM:SSZ`).
    ///
    /// A `vmt_<id>_<secret>` string is looked up by its id and compared by digest; anything else
    /// can only be the legacy token. The digest is of the whole string, as it stood after
    /// `Bearer `, for both kinds: one formula, so the cabinet and the server cannot disagree.
    #[must_use]
    pub fn admit(&self, presented: &str, now: &str) -> Admission<'_> {
        let position = if let Some(rest) = presented.strip_prefix(token::PREFIX) {
            let Some((id, secret)) = rest.split_once('_') else {
                return Admission::Unknown;
            };
            let id = format!("{TOKEN_ID_PREFIX}{id}");
            if secret.is_empty() || !is_public_id(&id, TOKEN_ID_PREFIX) {
                return Admission::Unknown;
            }
            self.by_id.get(&id).copied()
        } else if presented.is_empty() {
            None
        } else {
            self.legacy
        };
        let Some(token) = position.and_then(|position| self.tokens.get(position)) else {
            return Admission::Unknown;
        };
        let digest = vibememory_cli::sha256::hex(presented.as_bytes());
        if !same_secret(digest.as_bytes(), token.sha256.as_bytes()) {
            return Admission::Unknown;
        }
        match &token.expires_at {
            // Both sides are `YYYY-MM-DDTHH:MM:SSZ`, so the text order is the time order.
            Some(until) if now >= until.as_str() => Admission::Expired(token),
            _ => Admission::Granted(token),
        }
    }
}

/// Compares in time that does not depend on where the first difference is: an early exit would
/// tell a patient caller how much of the value they already have.
fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// Reads a snapshot and checks every rule of the spec.
///
/// # Errors
///
/// The first rule it breaks, in the spec's order.
pub fn check(bytes: &[u8]) -> Result<Snapshot, Refusal> {
    let raw: RawSnapshot = serde_json::from_slice(bytes)
        .map_err(|error| refuse("snapshotMalformed", error.to_string()))?;
    if raw.version != VERSION {
        return Err(refuse(
            "unsupportedVersion",
            format!("version is {}, only {VERSION} is known", raw.version),
        ));
    }
    names(&raw)?;
    dates(&raw)?;
    adopted(&raw)?;
    team_shapes(&raw)?;
    team_state(&raw)?;
    token_and_key_ids(&raw)?;
    references(&raw)?;
    tokens(&raw)?;
    keys(&raw)?;
    Ok(snapshot(raw))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSnapshot {
    version: u64,
    team_count: u64,
    teams: BTreeMap<String, RawTeam>,
    tokens: Vec<RawToken>,
    keys: Vec<RawKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawTeam {
    #[serde(default)]
    adopted: bool,
    #[serde(default)]
    repo: Option<String>,
    #[serde(default)]
    mode: Option<Mode>,
    writable: bool,
    #[serde(default)]
    deleted: Option<String>,
    #[serde(default)]
    projects: Option<Vec<String>>,
    limits: RawLimits,
    members: BTreeMap<String, Rank>,
}

/// Numbers are read as JSON numbers and judged by the rules: "not a positive integer" is a
/// broken limit, not a snapshot the reader could not parse.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawLimits {
    #[serde(default)]
    max_records: Option<Number>,
    #[serde(default)]
    max_record_bytes: Option<Number>,
    #[serde(default)]
    quota_bytes: Option<Number>,
    #[serde(default)]
    seats: Option<Number>,
}

impl RawLimits {
    fn named(&self) -> [(&'static str, Option<&Number>); 4] {
        [
            ("maxRecords", self.max_records.as_ref()),
            ("maxRecordBytes", self.max_record_bytes.as_ref()),
            ("quotaBytes", self.quota_bytes.as_ref()),
            ("seats", self.seats.as_ref()),
        ]
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawToken {
    id: String,
    #[serde(default)]
    legacy: bool,
    sha256: String,
    team: String,
    member: String,
    agent: String,
    role: TokenRole,
    history: bool,
    projects: Option<Vec<String>>,
    expires_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawKey {
    id: String,
    member: String,
    machine: String,
    store_name: String,
    teams: Vec<String>,
    public_key: String,
}

/// `^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$`: a team slug, a handle, a machine, an agent. The name
/// is a directory and an argument to git and ssh; a leading hyphen would make it an option.
#[must_use]
pub fn is_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=MAX_NAME_LENGTH).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes.first() != Some(&b'-')
        && bytes.last() != Some(&b'-')
}

/// `<prefix>` and a public id.
fn is_public_id(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(token::is_id)
}

fn names(raw: &RawSnapshot) -> Result<(), Refusal> {
    if let Some(slug) = raw.teams.keys().find(|slug| !is_name(slug)) {
        return Err(refuse("invalidSlug", format!("team slug {slug:?}")));
    }
    let handles = raw
        .teams
        .values()
        .flat_map(|team| team.members.keys())
        .chain(raw.tokens.iter().map(|token| &token.member))
        .chain(raw.keys.iter().map(|key| &key.member));
    for handle in handles {
        if !is_name(handle) {
            return Err(refuse("invalidHandle", format!("handle {handle:?}")));
        }
    }
    if let Some(key) = raw.keys.iter().find(|key| !is_name(&key.machine)) {
        return Err(refuse(
            "invalidMachine",
            format!("machine {:?} of key {}", key.machine, key.id),
        ));
    }
    if let Some(token) = raw.tokens.iter().find(|token| !is_name(&token.agent)) {
        return Err(refuse(
            "invalidAgent",
            format!("agent {:?} of token {}", token.agent, token.id),
        ));
    }
    let projects = raw
        .teams
        .values()
        .filter_map(|team| team.projects.as_ref())
        .chain(
            raw.tokens
                .iter()
                .filter_map(|token| token.projects.as_ref()),
        )
        .flatten();
    for project in projects {
        StoreName::parse(project).map_err(|error| refuse("invalidStoreName", error.to_string()))?;
    }
    for (slug, team) in &raw.teams {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for project in team.projects.iter().flatten() {
            let key = StoreName::parse(project)
                .map_err(|error| refuse("invalidStoreName", error.to_string()))?
                .key();
            if let Some(first) = seen.insert(key, project) {
                return Err(refuse(
                    "storeNameCollision",
                    format!(
                        "team {slug}: {first:?} and {project:?} are one directory on macOS and Windows"
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn dates(raw: &RawSnapshot) -> Result<(), Refusal> {
    for (slug, team) in &raw.teams {
        if let Some(day) = &team.deleted
            && !is_day(day)
        {
            return Err(refuse(
                "invalidDate",
                format!("team {slug}: deleted {day:?} is not YYYY-MM-DD"),
            ));
        }
    }
    for token in &raw.tokens {
        if let Some(moment) = &token.expires_at
            && !is_moment(moment)
        {
            return Err(refuse(
                "invalidDate",
                format!(
                    "token {}: expiresAt {moment:?} is not YYYY-MM-DDTHH:MM:SSZ",
                    token.id
                ),
            ));
        }
    }
    Ok(())
}

/// The digits of `text` as a number, when every character is a digit.
fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `YYYY-MM-DD`, a day that exists.
#[must_use]
pub fn is_day(text: &str) -> bool {
    let mut parts = text.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return false;
    }
    let (Some(year), Some(month), Some(day)) = (digits(year), digits(month), digits(day)) else {
        return false;
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=last).contains(&day)
}

/// `YYYY-MM-DDTHH:MM:SSZ`, a moment that exists, in UTC.
fn is_moment(text: &str) -> bool {
    let Some((day, time)) = text.split_once('T') else {
        return false;
    };
    let Some(time) = time.strip_suffix('Z') else {
        return false;
    };
    let mut parts = time.split(':');
    let (Some(hour), Some(minute), Some(second), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let two = |part: &str, below: u32| part.len() == 2 && digits(part).is_some_and(|n| n < below);
    is_day(day) && two(hour, 24) && two(minute, 60) && two(second, 60)
}

fn adopted(raw: &RawSnapshot) -> Result<(), Refusal> {
    let adopted: Vec<(&String, &RawTeam)> =
        raw.teams.iter().filter(|(_, team)| team.adopted).collect();
    if adopted.len() > 1 {
        let slugs: Vec<&str> = adopted.iter().map(|(slug, _)| slug.as_str()).collect();
        return Err(refuse(
            "adoptedTwice",
            format!("more than one adopted team: {slugs:?}"),
        ));
    }
    for (slug, team) in &adopted {
        let absolute = team
            .repo
            .as_deref()
            .is_some_and(|repo| repo.starts_with('/'));
        let extra = [
            ("mode", team.mode.is_some()),
            ("projects", team.projects.is_some()),
            ("deleted", team.deleted.is_some()),
            ("quotaBytes", team.limits.quota_bytes.is_some()),
            ("seats", team.limits.seats.is_some()),
        ]
        .into_iter()
        .find_map(|(field, present)| present.then_some(field));
        if !absolute {
            return Err(refuse(
                "adoptedRules",
                format!("team {slug}: an adopted store needs an absolute repo"),
            ));
        }
        if let Some(field) = extra {
            return Err(refuse(
                "adoptedRules",
                format!("team {slug}: an adopted store has no {field}"),
            ));
        }
    }
    for (slug, team) in &adopted {
        let only_owner =
            team.members.len() == 1 && team.members.values().all(|rank| *rank == Rank::Owner);
        if !only_owner {
            return Err(refuse(
                "adoptedMembers",
                format!(
                    "team {slug}: the personal store holds the owner's transcripts and has one member, its owner"
                ),
            ));
        }
    }
    Ok(())
}

fn team_shapes(raw: &RawSnapshot) -> Result<(), Refusal> {
    let others = || raw.teams.iter().filter(|(_, team)| !team.adopted);
    if let Some((slug, _)) = others().find(|(_, team)| team.repo.is_some()) {
        return Err(refuse(
            "repoOutsideAdopted",
            format!("team {slug}: only the adopted store names its repo"),
        ));
    }
    if let Some((slug, _)) = others().find(|(_, team)| team.mode.is_none()) {
        return Err(refuse("modeMissing", format!("team {slug} has no mode")));
    }
    if let Some((slug, team)) =
        others().find(|(_, team)| (team.mode == Some(Mode::Memory)) != team.projects.is_some())
    {
        let why = if team.projects.is_some() {
            "only a memory team lists projects"
        } else {
            "a memory team lists its projects"
        };
        return Err(refuse("projectsMisplaced", format!("team {slug}: {why}")));
    }
    for (slug, team) in &raw.teams {
        for (name, value) in team.limits.named() {
            if let Some(value) = value
                && value.as_u64().is_none_or(|number| number == 0)
            {
                return Err(refuse(
                    "limitsInvalid",
                    format!("team {slug}: {name} {value} is not an integer above zero"),
                ));
            }
        }
        if team.deleted.is_some() {
            continue;
        }
        // The first two bind every live team; a team with its own repository also has a size
        // and a number of seats.
        let named = team.limits.named();
        let required = if team.adopted {
            named.get(..2).unwrap_or_default()
        } else {
            named.as_slice()
        };
        if let Some((name, _)) = required.iter().find(|(_, value)| value.is_none()) {
            return Err(refuse(
                "limitsInvalid",
                format!("team {slug}: {name} is required"),
            ));
        }
    }
    Ok(())
}

fn team_state(raw: &RawSnapshot) -> Result<(), Refusal> {
    for (slug, team) in raw.teams.iter().filter(|(_, team)| team.deleted.is_some()) {
        let what = if team.writable {
            Some("is writable")
        } else if !team.members.is_empty() {
            Some("has members")
        } else if team.projects.as_ref().is_some_and(|list| !list.is_empty()) {
            Some("has projects")
        } else if raw.tokens.iter().any(|token| token.team == *slug) {
            Some("is named by a token")
        } else if raw.keys.iter().any(|key| key.teams.contains(slug)) {
            Some("is named by a machine key")
        } else {
            None
        };
        if let Some(what) = what {
            return Err(refuse(
                "deletedTeamActive",
                format!("deleted team {slug} {what}"),
            ));
        }
    }
    let active = || raw.teams.iter().filter(|(_, team)| team.deleted.is_none());
    for (slug, team) in active() {
        let owners = team
            .members
            .values()
            .filter(|rank| **rank == Rank::Owner)
            .count();
        if owners != 1 {
            return Err(refuse(
                "ownerCount",
                format!("team {slug} has {owners} owners, not one"),
            ));
        }
    }
    for (slug, team) in &raw.teams {
        if let Some(seats) = team.limits.seats.as_ref().and_then(Number::as_u64)
            && u64::try_from(team.members.len()).unwrap_or(u64::MAX) > seats
        {
            return Err(refuse(
                "seatsExceeded",
                format!(
                    "team {slug} has {} members and {seats} seats",
                    team.members.len()
                ),
            ));
        }
    }
    let teams = u64::try_from(raw.teams.len()).unwrap_or(u64::MAX);
    if raw.team_count < teams {
        return Err(refuse(
            "teamCountLow",
            format!(
                "teamCount {} is below the {teams} teams listed",
                raw.team_count
            ),
        ));
    }
    Ok(())
}

fn token_and_key_ids(raw: &RawSnapshot) -> Result<(), Refusal> {
    if let Some(token) = raw
        .tokens
        .iter()
        .find(|token| token.id != LEGACY_TOKEN_ID && !is_public_id(&token.id, TOKEN_ID_PREFIX))
    {
        return Err(refuse("invalidTokenId", format!("token id {:?}", token.id)));
    }
    if let Some(key) = raw
        .keys
        .iter()
        .find(|key| !is_public_id(&key.id, KEY_ID_PREFIX))
    {
        return Err(refuse("invalidKeyId", format!("key id {:?}", key.id)));
    }
    let mut seen = BTreeSet::new();
    for id in raw.tokens.iter().map(|token| &token.id) {
        if !seen.insert(id) {
            return Err(refuse("duplicateId", format!("two tokens {id}")));
        }
    }
    let mut seen = BTreeSet::new();
    for id in raw.keys.iter().map(|key| &key.id) {
        if !seen.insert(id) {
            return Err(refuse("duplicateId", format!("two keys {id}")));
        }
    }
    if let Some(token) = raw.tokens.iter().find(|token| {
        token.sha256.len() != 64
            || !token
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(refuse(
            "invalidDigest",
            format!(
                "token {}: sha256 is not 64 lowercase hex characters",
                token.id
            ),
        ));
    }
    Ok(())
}

fn references(raw: &RawSnapshot) -> Result<(), Refusal> {
    let named = raw
        .tokens
        .iter()
        .map(|token| (format!("token {}", token.id), &token.member, &token.team))
        .chain(raw.keys.iter().flat_map(|key| {
            key.teams
                .iter()
                .map(move |team| (format!("key {}", key.id), &key.member, team))
        }));
    let mut membership = Vec::new();
    for (who, member, slug) in named {
        let Some(team) = raw.teams.get(slug) else {
            return Err(refuse(
                "unknownTeam",
                format!("{who} names team {slug}, which the snapshot does not hold"),
            ));
        };
        membership.push((who, member, slug, team));
    }
    for (who, member, slug, team) in membership {
        if !team.members.contains_key(member) {
            return Err(refuse(
                "notAMember",
                format!("{who}: {member} is not a member of {slug}"),
            ));
        }
    }
    Ok(())
}

fn tokens(raw: &RawSnapshot) -> Result<(), Refusal> {
    let rank = |token: &RawToken| {
        raw.teams
            .get(&token.team)
            .and_then(|team| team.members.get(&token.member))
            .copied()
    };
    if let Some(token) = raw
        .tokens
        .iter()
        .find(|token| token.history && rank(token) == Some(Rank::Member))
    {
        return Err(refuse(
            "historyNotAllowed",
            format!(
                "token {}: past sessions are searched only by an owner or an admin",
                token.id
            ),
        ));
    }
    for token in &raw.tokens {
        let Some(team) = raw.teams.get(&token.team) else {
            continue;
        };
        if team.mode != Some(Mode::Memory) {
            continue;
        }
        let listed = team.projects.as_deref().unwrap_or_default();
        if let Some(outside) = token
            .projects
            .iter()
            .flatten()
            .find(|project| !listed.contains(project))
        {
            return Err(refuse(
                "projectOutsideTeam",
                format!(
                    "token {}: {outside} is not a project of {}",
                    token.id, token.team
                ),
            ));
        }
    }
    let legacy: Vec<&RawToken> = raw.tokens.iter().filter(|token| token.legacy).collect();
    if legacy.len() > 1 {
        return Err(refuse(
            "legacyTokenTwice",
            format!("{} tokens are marked legacy, one at most", legacy.len()),
        ));
    }
    let misplaced = raw.tokens.iter().find(|token| {
        let adopted = raw.teams.get(&token.team).is_some_and(|team| team.adopted);
        let named = token.id == LEGACY_TOKEN_ID;
        (token.legacy && !(adopted && named)) || (named && !token.legacy)
    });
    if let Some(token) = misplaced {
        return Err(refuse(
            "legacyTokenMisplaced",
            format!(
                "token {}: the legacy token is tk_legacy, marked legacy, in the adopted store",
                token.id
            ),
        ));
    }
    Ok(())
}

fn keys(raw: &RawSnapshot) -> Result<(), Refusal> {
    if let Some(key) = raw
        .keys
        .iter()
        .find(|key| key.store_name != format!("{}-{}", key.member, key.machine))
    {
        return Err(refuse(
            "storeNameMismatch",
            format!(
                "key {}: storeName {:?} is not {}-{}",
                key.id, key.store_name, key.member, key.machine
            ),
        ));
    }
    let mut seen = BTreeSet::new();
    for key in &raw.keys {
        if !seen.insert(&key.store_name) {
            return Err(refuse(
                "storeNameTaken",
                format!("two keys write as {}", key.store_name),
            ));
        }
    }
    if let Some(key) = raw.keys.iter().find(|key| key.teams.is_empty()) {
        return Err(refuse(
            "keyWithoutTeams",
            format!("key {} opens no team", key.id),
        ));
    }
    if let Some((key, slug)) = raw.keys.iter().find_map(|key| {
        key.teams
            .iter()
            .find(|slug| raw.teams.get(*slug).is_some_and(|team| team.adopted))
            .map(|slug| (key, slug))
    }) {
        return Err(refuse(
            "keyForAdopted",
            format!(
                "key {} names {slug}: the personal store goes by its owner's own key",
                key.id
            ),
        ));
    }
    if let Some(key) = raw
        .keys
        .iter()
        .find(|key| !is_ed25519_line(&key.public_key))
    {
        return Err(refuse(
            "invalidPublicKey",
            format!(
                "key {}: expected `{ED25519} <base64>` of a 32-byte key, on one line and without a comment",
                key.id
            ),
        ));
    }
    Ok(())
}

/// `ssh-ed25519 <base64>` and nothing else: one line, no comment, and a blob that is exactly an
/// ed25519 key — the line becomes a line of `authorized_keys`, where anything more is an option.
fn is_ed25519_line(line: &str) -> bool {
    if line.chars().any(char::is_control) {
        return false;
    }
    let Some(encoded) = line
        .strip_prefix(ED25519)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    let Some(blob) = base64(encoded) else {
        return false;
    };
    // The wire form: a string naming the type, then a string holding the key.
    let mut expected = Vec::with_capacity(4 + ED25519.len() + 4 + ED25519_KEY_BYTES);
    expected.extend_from_slice(&u32::try_from(ED25519.len()).unwrap_or(0).to_be_bytes());
    expected.extend_from_slice(ED25519.as_bytes());
    expected.extend_from_slice(&u32::try_from(ED25519_KEY_BYTES).unwrap_or(0).to_be_bytes());
    blob.len() == expected.len() + ED25519_KEY_BYTES && blob.starts_with(&expected)
}

/// Standard base64 with padding, strictly: no spaces, no line breaks, no missing padding.
fn base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
    if padding > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes.get(..bytes.len() - padding)? {
        let value = ALPHABET.iter().position(|candidate| candidate == byte)?;
        buffer = (buffer << 6) | u32::try_from(value).ok()?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

fn snapshot(raw: RawSnapshot) -> Snapshot {
    let limit = |value: Option<Number>| value.as_ref().and_then(Number::as_u64);
    let teams = raw
        .teams
        .into_iter()
        .map(|(slug, team)| {
            let limits = Limits {
                max_records: limit(team.limits.max_records),
                max_record_bytes: limit(team.limits.max_record_bytes),
                quota_bytes: limit(team.limits.quota_bytes),
                seats: limit(team.limits.seats),
            };
            let team = Team {
                adopted: team.adopted,
                repo: team.repo,
                mode: team.mode,
                writable: team.writable,
                deleted: team.deleted,
                projects: team.projects,
                limits,
                members: team.members,
            };
            (slug, team)
        })
        .collect();
    let tokens: Vec<Token> = raw
        .tokens
        .into_iter()
        .map(|token| Token {
            id: token.id,
            legacy: token.legacy,
            sha256: token.sha256,
            team: token.team,
            member: token.member,
            agent: token.agent,
            role: token.role,
            history: token.history,
            projects: token.projects,
            expires_at: token.expires_at,
        })
        .collect();
    let keys = raw
        .keys
        .into_iter()
        .map(|key| Key {
            id: key.id,
            member: key.member,
            machine: key.machine,
            store_name: key.store_name,
            teams: key.teams,
            public_key: key.public_key,
        })
        .collect();
    let by_id = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| !token.legacy)
        .map(|(position, token)| (token.id.clone(), position))
        .collect();
    let legacy = tokens.iter().position(|token| token.legacy);
    Snapshot {
        team_count: raw.team_count,
        teams,
        tokens,
        keys,
        by_id,
        legacy,
    }
}
