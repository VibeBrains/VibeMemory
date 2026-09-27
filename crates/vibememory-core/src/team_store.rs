//! What a machine knows about a team store it takes part in: `stores/<id>/store.json`, written by
//! `connect` from the cabinet's answer to a machine key claim and read by every tick. The owner's
//! `config.json` holds only which working directories are the team's; this record holds the rest,
//! so the engine never rewrites a file a person wrote.

use serde::{Deserialize, Serialize};

use crate::claim::KeyGrant;
use crate::naming::is_slug;

/// The version of the record this engine writes and reads.
pub const RECORD_VERSION: u64 = 1;

/// The file of a store's generation: a number the host raises each time it rewrites the store's
/// history — when the team's sessions are switched off — so a machine holding an older clone knows
/// it must not merge into the new one.
pub const GENERATION_FILE: &str = ".vibememory-generation";

/// The mode of a team whose sessions are on: the only one a machine clones the store of.
pub const SYNC_MODE: &str = "sync";

/// A team store as this machine knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoreRecord {
    /// [`RECORD_VERSION`].
    pub version: u64,
    /// The cabinet the key was claimed from: where `connect --refresh` asks and `doctor` sends.
    pub cabinet: String,
    /// The team's slug — the store's id.
    pub team: String,
    /// The member's handle: it signs every memory version this machine writes into the team.
    pub member: String,
    /// The machine's name at the member.
    pub machine: String,
    /// `<member>-<machine>`: the one directory of `machines/` the host lets this machine push.
    pub store_name: String,
    /// The key's public id, `mk_…`, as the cabinet shows it for revocation.
    pub key_id: String,
    /// The host.
    pub ssh_host: String,
    /// The account of the host's forced command.
    pub ssh_user: String,
    /// `teams/<team>.git`.
    pub remote: String,
}

/// Why a record cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// Not the JSON of a record, or another version.
    #[error("store.json is not a record this engine reads: {0}")]
    Invalid(String),
    /// A field that becomes a directory, an argument or a signature is not what it must be.
    #[error("store.json: {0}")]
    Field(&'static str),
}

impl StoreRecord {
    /// The record of a key grant — `None` for a team whose sessions are off: its store is not
    /// cloned, and a key for it has nothing to push.
    #[must_use]
    pub fn from_grant(grant: &KeyGrant) -> Option<Self> {
        (grant.mode == SYNC_MODE).then(|| Self {
            version: RECORD_VERSION,
            cabinet: grant.cabinet.clone(),
            team: grant.team.clone(),
            member: grant.member.clone(),
            machine: grant.machine.clone(),
            store_name: grant.store_name.clone(),
            key_id: grant.key_id.clone(),
            ssh_host: grant.ssh_host.clone(),
            ssh_user: grant.ssh_user.clone(),
            remote: grant.remote.clone(),
        })
    }

    /// Reads a record and checks what it names: the file sits on the machine, and a hand edit or a
    /// half write must stop the store rather than push under another name.
    ///
    /// # Errors
    ///
    /// [`RecordError`].
    pub fn parse(text: &str) -> Result<Self, RecordError> {
        let record: Self =
            serde_json::from_str(text).map_err(|error| RecordError::Invalid(error.to_string()))?;
        if record.version != RECORD_VERSION {
            return Err(RecordError::Invalid(format!(
                "version {} (this engine reads {RECORD_VERSION})",
                record.version
            )));
        }
        for (field, value) in [
            ("team is not a slug", &record.team),
            ("member is not a slug", &record.member),
            ("machine is not a slug", &record.machine),
            ("sshUser is not a slug", &record.ssh_user),
        ] {
            if !is_slug(value) {
                return Err(RecordError::Field(field));
            }
        }
        if record.store_name != format!("{}-{}", record.member, record.machine) {
            return Err(RecordError::Field("storeName is not <member>-<machine>"));
        }
        if record.remote != format!("teams/{}.git", record.team) {
            return Err(RecordError::Field("remote is not the team's store"));
        }
        // it becomes part of an ssh argument: a leading dash would be an option
        if record.ssh_host.is_empty()
            || record.ssh_host.starts_with('-')
            || !record
                .ssh_host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            return Err(RecordError::Field("sshHost is not a host name"));
        }
        Ok(record)
    }

    /// The record as `connect` writes it.
    #[must_use]
    pub fn to_json(&self) -> String {
        // a record of plain strings always serializes
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }

    /// The git address of the team's repository: the host's forced command reads the path.
    #[must_use]
    pub fn git_url(&self) -> String {
        format!("{}@{}:{}", self.ssh_user, self.ssh_host, self.remote)
    }
}

/// The team's own `known_hosts`: the host's keys from the claim, so the first connection is
/// checked against what the cabinet vouched for and never against a key taken on trust. `None`
/// when the cabinet named none — then there is nothing to check the host by, and nothing is
/// cloned.
#[must_use]
pub fn known_hosts(host: &str, keys: &[String]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    Some(keys.iter().fold(String::new(), |mut text, key| {
        text.push_str(host);
        text.push(' ');
        text.push_str(key.trim());
        text.push('\n');
        text
    }))
}

/// The public answer of the cabinet's `GET /api/agent/hostkeys`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostKeysAnswer {
    ssh_host: String,
    host_keys: Vec<String>,
}

/// The team's new `known_hosts` from the cabinet's host keys, for `connect --refresh`: the only
/// file it rewrites. The answer must name the host the store record names — a cabinet that now
/// points elsewhere is a question for a person, not a silent switch — and every key must be an
/// ed25519 line of one line.
///
/// # Errors
///
/// What is wrong with the answer, in a sentence for the terminal.
pub fn refreshed_known_hosts(body: &str, record: &StoreRecord) -> Result<String, String> {
    let answer: HostKeysAnswer = serde_json::from_str(body)
        .map_err(|error| format!("the cabinet's host keys are not understood: {error}"))?;
    if answer.ssh_host != record.ssh_host {
        return Err(format!(
            "the cabinet now names host {:?}, not {:?}: connect again with a new code",
            answer.ssh_host, record.ssh_host
        ));
    }
    if answer
        .host_keys
        .iter()
        .any(|key| !crate::ssh_key::is_ed25519_line(key))
    {
        return Err("a host key the cabinet named is not ssh-ed25519".to_owned());
    }
    known_hosts(&record.ssh_host, &answer.host_keys)
        .ok_or_else(|| "the cabinet named no host key: try again in a few minutes".to_owned())
}
