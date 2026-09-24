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
use crate::terminal::printable;
use crate::token;

/// The only answer version there is.
const ANSWER_VERSION: u64 = 1;

/// The claim endpoint's answer: the grant.
const STATUS_OK: u16 = 200;

/// The claim endpoint's one refusal: unknown, used, expired — it does not say which.
const STATUS_REFUSED: u16 = 400;

/// The cabinet is out of step with its host and read no code.
const STATUS_UNAVAILABLE: u16 = 503;

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
    /// The cabinet is out of step with its host for now: it read no code, which stays good.
    Unavailable,
    /// The cabinet answered, but not as a claim endpoint does: likely not the cabinet's address.
    Unexpected {
        /// The HTTP status.
        status: u16,
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
            Self::Unavailable => formatter.write_str(
                "the cabinet is not in step with its host right now and did not use the code: try again in a few minutes",
            ),
            Self::Unexpected { status } => write!(
                formatter,
                "the cabinet answered HTTP {status}, which no claim is answered with: check the address"
            ),
            Self::Malformed(reason) => write!(
                formatter,
                "the cabinet's answer is not understood: {reason}"
            ),
        }
    }
}

/// Reads what curl left: its exit code, the HTTP status and the body. `cabinet` is the address the
/// claim was sent to, as [`cabinet_address`] gave it: the answer must name that cabinet, and the
/// memory server it names must be the cabinet's host or its parent.
///
/// # Errors
///
/// [`ClaimFailure`]: a refusal, a cabinet out of step, an unreachable cabinet, or an answer that
/// breaks the contract.
pub fn read_answer(
    exit: i32,
    status: u16,
    body: &str,
    cabinet: &str,
) -> Result<Claim, ClaimFailure> {
    if exit != 0 {
        return Err(ClaimFailure::Unreachable { exit });
    }
    match status {
        STATUS_OK => {}
        STATUS_REFUSED => return Err(ClaimFailure::Refused),
        STATUS_UNAVAILABLE => return Err(ClaimFailure::Unavailable),
        other => return Err(ClaimFailure::Unexpected { status: other }),
    }
    let mut value: serde_json::Map<String, serde_json::Value> = serde_json::from_str(body)
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
            check_token(&grant, cabinet)?;
            Ok(Claim::Token(grant))
        }
        Some("key") => {
            let grant: KeyGrant =
                serde_json::from_value(fields).map_err(|error| malformed(&error))?;
            check_key(&grant, cabinet)?;
            Ok(Claim::Key(grant))
        }
        other => Err(ClaimFailure::Malformed(format!("unknown kind {other:?}"))),
    }
}

/// A parse error of the answer: its text quotes the answer — a field's name, a value — and is
/// printed to a terminal, so whatever could steer the terminal is escaped.
fn malformed(error: &serde_json::Error) -> ClaimFailure {
    ClaimFailure::Malformed(printable(&error.to_string()))
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

/// An address split into what the checks need: whether it is https, its host in lower case
/// (without the brackets of an IPv6 address), its port when it is not the scheme's own, and its
/// path.
struct Address {
    https: bool,
    host: String,
    port: Option<u16>,
    path: String,
}

impl Address {
    /// `host[:port]` in the one form addresses are compared in.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        self.port
            .map_or(host.clone(), |port| format!("{host}:{port}"))
    }
}

/// The port of an address when it is not the scheme's own: `:443` of https and `:80` of http name
/// the same address as none at all.
fn explicit_port(https: bool, port: u16) -> Option<u16> {
    let default = if https { 443 } else { 80 };
    (port != default).then_some(port)
}

/// Splits `scheme://authority/path` of an http(s) address. `None` for anything else: another
/// scheme, a user name (`@`), a query or a fragment, a space or a control character, a host that
/// is not a name or an address, anything after an IPv6 address but a port, a port that is not a
/// number from 1 to 65535.
fn split_address(value: &str) -> Option<Address> {
    if value
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    let (https, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        (true, rest)
    } else {
        (false, lower.strip_prefix("http://")?)
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    if authority.contains(['@', '?', '#']) || path.contains(['?', '#']) {
        return None;
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (inside, after) = bracketed.split_once(']')?;
        inside.parse::<std::net::Ipv6Addr>().ok()?;
        let port = if after.is_empty() {
            None
        } else {
            Some(after.strip_prefix(':')?)
        };
        (inside, port)
    } else {
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        };
        if !is_host_name(host) {
            return None;
        }
        (host, port)
    };
    let port = match port {
        None => None,
        Some(digits) => {
            if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let number = digits.parse::<u16>().ok().filter(|number| *number > 0)?;
            explicit_port(https, number)
        }
    };
    Some(Address {
        https,
        host: host.to_owned(),
        port,
        // the path keeps its case: only the scheme and the host are case-blind
        path: value
            .get(value.len() - path.len()..)
            .unwrap_or_default()
            .to_owned(),
    })
}

/// A host name or an IPv4 address: dot-separated labels of letters, digits and inner dashes.
fn is_host_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// This machine: plain http is taken only for a cabinet that never leaves it.
fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Whether a memory server's host belongs to the cabinet: the cabinet's own host, or its parent
/// domain (`app.vibememory.ru` → `vibememory.ru`). Never a sibling: under a suffix anyone registers
/// in (`*.co.uk`), a neighbour of the cabinet is somebody else's host. An address, or a name with
/// no parent of two labels, is only itself.
fn serves_cabinet(server: &str, cabinet: &str) -> bool {
    if server == cabinet {
        return true;
    }
    cabinet.parse::<std::net::IpAddr>().is_err()
        && cabinet
            .split_once('.')
            .is_some_and(|(_, parent)| parent.contains('.') && server == parent)
}

/// A cabinet's address as `connect --cabinet` takes it, made the one form it is compared in:
/// `https://<host>[:port]`, lower case, no path, no port of the scheme's own. Plain `http://` only
/// for a cabinet on this machine — a code sent in the open can be read on the way and redeemed by
/// whoever reads it.
///
/// # Errors
///
/// Why the address is not taken, for a person to read.
pub fn cabinet_address(value: &str) -> Result<String, String> {
    // `{:?}` quotes the value and escapes whatever could steer a terminal
    let address =
        split_address(value.trim()).ok_or_else(|| format!("{value:?} is not an https address"))?;
    if !address.https && !is_loopback(&address.host) {
        return Err(format!(
            "{value:?} is not https: a code sent in the open can be read on the way"
        ));
    }
    if !address.path.is_empty() && address.path != "/" {
        return Err(format!(
            "{value:?} has a path: the cabinet is its address alone"
        ));
    }
    let scheme = if address.https { "https" } else { "http" };
    Ok(format!("{scheme}://{}", address.authority()))
}

/// The cabinet the answer names must be the one the claim was sent to.
fn same_cabinet(field: &str, value: &str, asked: &str) -> Result<(), ClaimFailure> {
    match cabinet_address(value) {
        Ok(named) if named == asked => Ok(()),
        _ => Err(ClaimFailure::Malformed(format!(
            "{field} is not the cabinet the code was sent to"
        ))),
    }
}

/// The memory server an agent is registered with: https (http only on this machine), a plain path,
/// the cabinet's own host or its parent — the line that registers it is pasted into a shell, and
/// the server it names receives the token with every call.
fn memory_server(value: &str, cabinet: &str) -> Result<(), ClaimFailure> {
    let refuse = |why: &str| Err(ClaimFailure::Malformed(format!("mcpUrl {why}")));
    let Some(address) = split_address(value) else {
        return refuse("is not an https address");
    };
    if !address.https && !is_loopback(&address.host) {
        return refuse("is not https");
    }
    if !address.path.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'~' | b'-')
    }) {
        return refuse("has a path with characters a shell would read");
    }
    let asked = split_address(cabinet).map(|cabinet| cabinet.host);
    if !asked.is_some_and(|asked| serves_cabinet(&address.host, &asked)) {
        return refuse("is not the cabinet's host or its parent domain");
    }
    Ok(())
}

fn check_token(grant: &TokenGrant, cabinet: &str) -> Result<(), ClaimFailure> {
    slug("team", &grant.team)?;
    slug("member", &grant.member)?;
    slug("agent", &grant.agent)?;
    same_cabinet("cabinet", &grant.cabinet, cabinet)?;
    memory_server(&grant.mcp_url, cabinet)?;
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

fn check_key(grant: &KeyGrant, cabinet: &str) -> Result<(), ClaimFailure> {
    slug("team", &grant.team)?;
    slug("member", &grant.member)?;
    slug("machine", &grant.machine)?;
    slug("sshUser", &grant.ssh_user)?;
    same_cabinet("cabinet", &grant.cabinet, cabinet)?;
    // it becomes an ssh argument: a name that starts with a dash would be read as an option
    if !is_host_name(&grant.ssh_host) {
        return Err(ClaimFailure::Malformed(format!(
            "sshHost {:?} is not a host name",
            grant.ssh_host
        )));
    }
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

/// The current user's security identifier from `whoami /user /fo csv /nh` — one line,
/// `"<domain>\\<user>","S-1-5-…"`. A grant by SID names this very account; a bare user name may
/// resolve to a same-named local account in a domain.
#[must_use]
pub fn sid_of(whoami: &str) -> Option<String> {
    let line = whoami.lines().find(|line| !line.trim().is_empty())?;
    let sid = line.rsplit(',').next()?.trim().trim_matches('"');
    let digits = sid.strip_prefix("S-1-")?;
    (!digits.is_empty()
        && digits
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())))
    .then(|| sid.to_owned())
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
