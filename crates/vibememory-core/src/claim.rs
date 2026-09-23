//! The claim: what the cabinet answers when a machine trades a one-time code for a token or a
//! machine key, and what of that answer the engine may trust.
//!
//! Pure: curl's exit code and its stdout in, the grant or the reason out. The answer is a
//! contract with the cabinet — `fixtures/claim/claimAnswers.json` is read by both sides — and it
//! is read strictly: the team and the agent become directory names on this machine, so a name
//! that is not a slug, an unknown field or another version is refused, never guessed at.

use std::fmt;

use serde::Deserialize;

use crate::naming::is_slug;
use crate::token;

/// The only answer version there is.
const ANSWER_VERSION: u64 = 1;

/// curl's exit when `--fail` met an HTTP error: every refusal of a claim is one.
const CURL_HTTP_ERROR: i32 = 22;

/// The id prefix of a token in the snapshot and the answer.
const TOKEN_ID_PREFIX: &str = "tk_";

/// The id prefix of a machine key.
const KEY_ID_PREFIX: &str = "mk_";

/// The only machine key type there is.
const ED25519_PREFIX: &str = "ssh-ed25519 ";

/// A token as the cabinet handed it over.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TokenGrant {
    /// The cabinet's address: where `doctor` and `connect --refresh` send a person.
    pub cabinet: String,
    /// The team the token is for.
    pub team: String,
    /// The member it acts as.
    pub member: String,
    /// The agent it signs as.
    pub agent: String,
    /// Its public id, `tk_…`.
    pub token_id: String,
    /// The token itself: written to a file and never printed.
    pub token: String,
    /// The memory server the agent registers.
    pub mcp_url: String,
}

/// Never the token: a grant printed by accident must not print the secret with it.
impl fmt::Debug for TokenGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenGrant")
            .field("cabinet", &self.cabinet)
            .field("team", &self.team)
            .field("member", &self.member)
            .field("agent", &self.agent)
            .field("token_id", &self.token_id)
            .field("mcp_url", &self.mcp_url)
            .finish_non_exhaustive()
    }
}

/// A machine key as the cabinet registered it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyGrant {
    /// The cabinet's address.
    pub cabinet: String,
    /// The team whose store the machine clones.
    pub team: String,
    /// `memory` or `sync`.
    pub mode: String,
    /// The member the machine belongs to.
    pub member: String,
    /// The machine's name at the member.
    pub machine: String,
    /// `<member>-<machine>`: the machine's directory in the store.
    pub store_name: String,
    /// Its public id, `mk_…`.
    pub key_id: String,
    /// The host to connect to.
    pub ssh_host: String,
    /// The account the host's forced command runs under.
    pub ssh_user: String,
    /// `teams/<team>.git`.
    pub remote: String,
    /// The host's public keys, so the first connection is checked.
    pub host_keys: Vec<String>,
}

/// What a claim gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// A token for an agent.
    Token(TokenGrant),
    /// A key for a machine.
    Key(KeyGrant),
}

/// Why a claim gave nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimFailure {
    /// The cabinet said no: the code is unknown, used or expired — it does not say which.
    Refused,
    /// The cabinet was not reached; the exit code is curl's.
    Unreachable {
        /// curl's exit code.
        exit: i32,
    },
    /// An answer the engine does not understand, and so does not act on.
    Malformed(String),
}

impl fmt::Display for ClaimFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused => formatter.write_str(
                "the cabinet refused the code: it is unknown, used or expired — make a new one",
            ),
            Self::Unreachable { exit } => write!(
                formatter,
                "the cabinet was not reached (curl exit {exit}): check the address and the network"
            ),
            Self::Malformed(reason) => write!(
                formatter,
                "the cabinet's answer is not understood: {reason}"
            ),
        }
    }
}

/// Reads what `curl --fail` left: the exit code and stdout.
///
/// # Errors
///
/// [`ClaimFailure`]: a refusal, an unreachable cabinet, or an answer that breaks the contract.
pub fn read_answer(exit: i32, stdout: &str) -> Result<Claim, ClaimFailure> {
    match exit {
        0 => {}
        CURL_HTTP_ERROR => return Err(ClaimFailure::Refused),
        other => return Err(ClaimFailure::Unreachable { exit: other }),
    }
    let mut value: serde_json::Map<String, serde_json::Value> = serde_json::from_str(stdout)
        .map_err(|error| ClaimFailure::Malformed(format!("not a JSON object: {error}")))?;
    // The envelope is read here; every other field goes to the grant, which knows them all.
    let version = value.remove("version").and_then(|version| version.as_u64());
    if version != Some(ANSWER_VERSION) {
        return Err(ClaimFailure::Malformed(format!(
            "answer version {} (this engine reads {ANSWER_VERSION})",
            version.map_or_else(|| "none".to_owned(), |version| version.to_string())
        )));
    }
    let kind = value.remove("kind");
    let fields = serde_json::Value::Object(value);
    match kind.as_ref().and_then(serde_json::Value::as_str) {
        Some("token") => {
            let grant: TokenGrant =
                serde_json::from_value(fields).map_err(|error| malformed(&error))?;
            check_token(&grant)?;
            Ok(Claim::Token(grant))
        }
        Some("key") => {
            let grant: KeyGrant =
                serde_json::from_value(fields).map_err(|error| malformed(&error))?;
            check_key(&grant)?;
            Ok(Claim::Key(grant))
        }
        other => Err(ClaimFailure::Malformed(format!("unknown kind {other:?}"))),
    }
}

fn malformed(error: &serde_json::Error) -> ClaimFailure {
    ClaimFailure::Malformed(error.to_string())
}

/// A name that becomes a directory here: a slug, or the answer is refused.
fn slug(field: &str, value: &str) -> Result<(), ClaimFailure> {
    if is_slug(value) {
        Ok(())
    } else {
        Err(ClaimFailure::Malformed(format!(
            "{field} {value:?} is not a name"
        )))
    }
}

/// An address a person is sent to or an agent connects to: http(s) and nothing else.
fn address(field: &str, value: &str) -> Result<(), ClaimFailure> {
    if value.starts_with("https://") || value.starts_with("http://") {
        Ok(())
    } else {
        Err(ClaimFailure::Malformed(format!(
            "{field} is not an http address"
        )))
    }
}

fn check_token(grant: &TokenGrant) -> Result<(), ClaimFailure> {
    slug("team", &grant.team)?;
    slug("member", &grant.member)?;
    slug("agent", &grant.agent)?;
    address("cabinet", &grant.cabinet)?;
    address("mcpUrl", &grant.mcp_url)?;
    let carried = grant
        .token
        .strip_prefix(token::PREFIX)
        .and_then(|rest| rest.split_once('_'))
        .filter(|(id, secret)| token::is_id(id) && !secret.is_empty())
        .map(|(id, _)| id)
        .ok_or_else(|| ClaimFailure::Malformed("the token is not vmt_<id>_<secret>".to_owned()))?;
    if grant.token_id != format!("{TOKEN_ID_PREFIX}{carried}") {
        return Err(ClaimFailure::Malformed(
            "the token's id is not the id the answer names".to_owned(),
        ));
    }
    Ok(())
}

fn check_key(grant: &KeyGrant) -> Result<(), ClaimFailure> {
    slug("team", &grant.team)?;
    slug("member", &grant.member)?;
    slug("machine", &grant.machine)?;
    slug("sshUser", &grant.ssh_user)?;
    address("cabinet", &grant.cabinet)?;
    if grant.mode != "memory" && grant.mode != "sync" {
        return Err(ClaimFailure::Malformed(format!("mode {:?}", grant.mode)));
    }
    if grant.store_name != format!("{}-{}", grant.member, grant.machine) {
        return Err(ClaimFailure::Malformed(
            "storeName is not <member>-<machine>".to_owned(),
        ));
    }
    if !grant
        .key_id
        .strip_prefix(KEY_ID_PREFIX)
        .is_some_and(token::is_id)
    {
        return Err(ClaimFailure::Malformed("keyId is not mk_<id>".to_owned()));
    }
    if grant.remote != format!("teams/{}.git", grant.team) {
        return Err(ClaimFailure::Malformed(
            "remote is not the team's store".to_owned(),
        ));
    }
    if grant
        .host_keys
        .iter()
        .any(|key| !key.starts_with(ED25519_PREFIX) || key.chars().any(char::is_control))
    {
        return Err(ClaimFailure::Malformed(
            "a host key is not ssh-ed25519".to_owned(),
        ));
    }
    Ok(())
}

/// What `connect` leaves beside a token for `doctor`, `disconnect` and `connect --refresh`: where
/// it came from and what it is — never the token.
#[must_use]
pub fn token_sidecar(grant: &TokenGrant) -> String {
    let sidecar = serde_json::json!({
        "cabinet": grant.cabinet,
        "team": grant.team,
        "member": grant.member,
        "agent": grant.agent,
        "tokenId": grant.token_id,
        "mcpUrl": grant.mcp_url,
    });
    format!("{sidecar:#}\n")
}

/// The name an agent registers the team's memory server under.
#[must_use]
pub fn server_name(team: &str) -> String {
    format!("vibememory-{team}")
}

/// The client configuration fragment for agents that read a JSON file of MCP servers: the one file
/// besides the token's own that holds it, written with the same rights.
#[must_use]
pub fn client_fragment(grant: &TokenGrant) -> String {
    let fragment = serde_json::json!({
        "mcpServers": {
            server_name(&grant.team): {
                "type": "http",
                "url": grant.mcp_url,
                "headers": { "Authorization": format!("Bearer {}", grant.token) },
            }
        }
    });
    format!("{fragment:#}\n")
}

/// What `icacls <file>` says about who may reach a token file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessList {
    /// The owner alone: what `connect` leaves.
    OwnerOnly,
    /// Somebody else too — the grantees, as icacls names them.
    Others(Vec<String>),
    /// Nothing to judge by: the output names no grantee at all.
    Unreadable,
}

/// Judges `icacls <path>` output against the current user (`whoami`), case aside. Grantees are
/// compared with the user, never with group names: Windows prints those in its own language.
#[must_use]
pub fn access_list(output: &str, path: &str, user: &str) -> AccessList {
    let user = user.trim().to_lowercase();
    let mut grantees = Vec::new();
    for (index, line) in output.lines().enumerate() {
        let entry = if index == 0 {
            // the first line starts with the path as it was given, then the first grantee
            match line
                .strip_prefix(path)
                .and_then(|rest| rest.strip_prefix(' '))
            {
                Some(rest) => rest,
                None => continue,
            }
        } else {
            line
        };
        if let Some(grantee) = grantee_of(entry.trim()) {
            grantees.push(grantee.to_owned());
        }
    }
    if grantees.is_empty() {
        return AccessList::Unreadable;
    }
    let others: Vec<String> = grantees
        .into_iter()
        .filter(|grantee| grantee.to_lowercase() != user)
        .collect();
    if others.is_empty() {
        AccessList::OwnerOnly
    } else {
        AccessList::Others(others)
    }
}

/// `DOMAIN\user:(I)(F)` → `DOMAIN\user`: the name before the first of the rights in parentheses.
fn grantee_of(entry: &str) -> Option<&str> {
    let (who, rights) = entry.split_once(":(")?;
    (!who.is_empty() && rights.ends_with(')')).then_some(who)
}
