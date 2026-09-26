//! `vibememory connect --cabinet <address> --agent <name>`: a claim code from the cabinet — typed
//! at the prompt or piped in, never an argument — traded for a token, and the token kept where the
//! agent reads it and nobody else does.
//!
//! The token passes through this process and three files — its own, and the client fragment for
//! agents that read MCP servers from JSON — and nowhere else: not stdout, not an argument, not a
//! log. What is printed is paths and the line that registers the server, which reads the token
//! from its file when it runs.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vibememory_core::claim::{
    TokenGrant, claude_code_config, client_fragment, server_name, token_sidecar,
};
use vibememory_core::naming::PathSyntax;

use crate::install::{Layout, shell_word, shell_word_for};

/// The cabinet's claim endpoint, under the address `--cabinet` names.
const CLAIM_PATH: &str = "/api/agent/claim";

/// How long the cabinet may take to answer, in seconds.
const CLAIM_TIMEOUT_SECONDS: &str = "30";

/// The directory of tokens, under the engine directory: outside `~/.claude`, so nothing the engine
/// exports can ever carry one.
pub const TOKENS_DIR: &str = "tokens";

/// The sidecar beside a token: where it came from and what it is, never the token.
pub const SIDECAR_EXTENSION: &str = "json";

/// The client fragment beside a token: the one other file that holds it.
pub const FRAGMENT_SUFFIX: &str = ".mcp.json";

/// The claim code, from the first line of `input`: the terminal a person pastes it into, or a pipe.
/// Never an argument — `ps` shows those to every user of the machine.
///
/// # Errors
///
/// When nothing could be read, or the line is empty.
pub fn read_code(mut input: impl std::io::BufRead) -> Result<String, String> {
    let mut line = String::new();
    input
        .read_line(&mut line)
        .map_err(|error| format!("the code could not be read: {error}"))?;
    let code = line.trim();
    if code.is_empty() {
        return Err("no code was given".to_owned());
    }
    Ok(code.to_owned())
}

/// Asks the cabinet with curl. The code goes through stdin: an argument would show in `ps` to
/// every user of the machine. curl, like git and ssh, is the machine's own — there is no TLS in
/// this binary. `cabinet` is an address [`vibememory_core::claim::cabinet_address`] took: curl
/// reads no `.curlrc` (`-q`) and speaks its scheme alone (`--proto`), so no configuration of the
/// machine sends the code anywhere else.
///
/// # Errors
///
/// When curl cannot be started or its output read. An answer, a refusal or an unreachable cabinet
/// is not an error here: they are the exit code, the HTTP status and the body, for
/// [`vibememory_core::claim::read_answer`], and curl's own words on stderr for a person. The status
/// comes after the body on a line of its own (`--write-out`): a refusal (400) and a cabinet out of
/// step with its host (503) mean different things, and `--fail` would make both one exit code.
pub fn ask_cabinet(
    cabinet: &str,
    code: &str,
    public_key: Option<&str>,
) -> Result<CabinetReply, String> {
    let url = format!("{cabinet}{CLAIM_PATH}");
    let protocol = if cabinet.starts_with("https://") {
        "=https"
    } else {
        "=http"
    };
    let mut child = Command::new("curl")
        .args([
            // first, or curl reads the user's .curlrc before it
            "-q",
            "--proto",
            protocol,
            "--write-out",
            "\n%{http_code}",
            "--silent",
            "--show-error",
            "--max-time",
            CLAIM_TIMEOUT_SECONDS,
            "--data-binary",
            "@-",
            "-H",
            "Content-Type: application/json",
            &url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    // a fresh public key goes with every code: only the answer says whether the code was for a
    // machine, and a key the cabinet did not take is dropped
    let body = match public_key {
        Some(key) => serde_json::json!({ "code": code, "publicKey": key }),
        None => serde_json::json!({ "code": code }),
    }
    .to_string();
    child
        .stdin
        .take()
        .ok_or_else(|| "curl took no input".to_owned())?
        .write_all(body.as_bytes())
        .map_err(|error| format!("the code could not be handed to curl: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl did not finish: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, status) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
    Ok(CabinetReply {
        exit: output.status.code().unwrap_or(-1),
        status: status.trim().parse().unwrap_or(0),
        body: body.to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// What curl left of a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabinetReply {
    /// curl's exit code: 0 when the cabinet answered at all, anything else when it was not reached.
    pub exit: i32,
    /// The HTTP status of the answer; 0 when there was none.
    pub status: u16,
    /// The answer.
    pub body: String,
    /// curl's own words on what went wrong; never the code, which went through stdin.
    pub stderr: String,
}

/// Where a team's token for an agent lives.
#[must_use]
pub fn token_file(layout: &Layout, team: &str, agent: &str) -> PathBuf {
    layout.engine_dir.join(TOKENS_DIR).join(team).join(agent)
}

/// The sidecar of a token file.
#[must_use]
pub fn sidecar_file(token: &Path) -> PathBuf {
    token.with_extension(SIDECAR_EXTENSION)
}

/// The client fragment of a token file.
#[must_use]
pub fn fragment_file(token: &Path) -> PathBuf {
    beside(token, FRAGMENT_SUFFIX)
}

/// `path` with `suffix` added to its whole file name — `with_extension` would give the token and
/// its sidecar one temporary name.
fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// The files a claim left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptToken {
    /// The token itself.
    pub token: PathBuf,
    /// Where it came from.
    pub sidecar: PathBuf,
    /// For agents that read a JSON list of MCP servers.
    pub fragment: PathBuf,
}

/// Keeps a token: the token, its sidecar and the client fragment, each written beside itself and
/// renamed into place, reachable by the owner alone — mode 0600 in a 0700 directory, and on
/// Windows, where a mode means nothing, an access list cut down to the owner by `icacls`.
///
/// # Errors
///
/// The text of what went wrong. A file whose rights could not be narrowed is removed, not left.
pub fn keep_token(layout: &Layout, grant: &TokenGrant) -> Result<KeptToken, String> {
    let token = token_file(layout, &grant.team, &grant.agent);
    let kept = KeptToken {
        sidecar: sidecar_file(&token),
        fragment: fragment_file(&token),
        token,
    };
    let directory = kept
        .token
        .parent()
        .ok_or_else(|| "the token has no directory".to_owned())?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    restrict_directory(&layout.engine_dir.join(TOKENS_DIR))?;
    restrict_directory(directory)?;
    write_private(&kept.token, grant.token.as_bytes())?;
    write_private(&kept.sidecar, token_sidecar(grant).as_bytes())?;
    write_private(&kept.fragment, client_fragment(grant).as_bytes())?;
    Ok(kept)
}

/// Writes `bytes` beside `path`, narrows the rights, then renames over it: the file never exists
/// in its place with wider rights than the owner's.
///
/// # Errors
///
/// The text of what went wrong; a file whose rights could not be narrowed is not left behind.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = beside(path, ".new");
    let written = write_new(&temporary, bytes).and_then(|()| restrict_file(&temporary));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

/// A file only its owner reads and writes.
#[cfg(unix)]
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// A directory only its owner enters.
#[cfg(unix)]
const PRIVATE_DIR_MODE: u32 = 0o700;

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|error| error.to_string())
}

/// Narrows a directory to its owner.
///
/// # Errors
///
/// When the rights could not be set.
#[cfg(unix)]
pub fn restrict_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| error.to_string())
}

/// On Windows a mode is not a right: the access list is cut to the owner — inheritance off, one
/// full-control entry for the user running this.
#[cfg(not(unix))]
fn restrict_file(path: &Path) -> Result<(), String> {
    owner_only(path, "F")
}

/// A directory's entry is inherited by what is made in it (`(OI)(CI)`): a file created there
/// starts with the owner alone instead of the account's default list.
/// Narrows a directory to its owner.
///
/// # Errors
///
/// When the rights could not be set.
#[cfg(not(unix))]
pub fn restrict_directory(path: &Path) -> Result<(), String> {
    owner_only(path, "(OI)(CI)F")
}

/// The rights go to the account by its SID (`*S-1-5-…`), as `whoami` names it: a bare user name
/// can resolve to a same-named local account in a domain.
#[cfg(not(unix))]
fn owner_only(path: &Path, rights: &str) -> Result<(), String> {
    let whoami = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("whoami could not be started: {error}"))?;
    let sid = vibememory_core::claim::sid_of(&String::from_utf8_lossy(&whoami.stdout))
        .ok_or_else(|| "whoami named no security identifier for this account".to_owned())?;
    let status = Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:{rights}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("icacls could not be started: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "icacls could not narrow the rights of {}",
            path.display()
        ))
    }
}

/// What `disconnect` did for a team on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disconnected {
    /// The files taken off the machine: each token, its sidecar and its client fragment.
    pub removed: Vec<PathBuf>,
    /// The tokens that are gone from here, `(public id, cabinet)`: they still open the team until
    /// they are revoked there.
    pub revoke: Vec<(String, String)>,
    /// Files of the team's directory `connect` did not write — a token kept by hand, from before the
    /// cabinet — left where they are: nothing here says where they came from or where to revoke them.
    pub left: Vec<PathBuf>,
}

/// Takes the tokens of `team` off this machine: every token `connect` kept for the team, with its
/// sidecar and client fragment, and the team's directory once nothing else is in it. A machine of
/// a `memory` team keeps nothing else of it. The tokens still open the team until they are revoked
/// in the cabinet their sidecars name; the engine never touches `~/.claude.json`, so a client
/// registered there is removed by the person.
///
/// # Errors
///
/// A name that is not a team, or a file that cannot be removed.
pub fn disconnect(layout: &Layout, team: &str) -> Result<Disconnected, String> {
    if !vibememory_core::naming::is_slug(team) {
        return Err(format!("{team:?} is not a team"));
    }
    let mut done = Disconnected::default();
    for kept in crate::credentials::kept_tokens(layout)
        .into_iter()
        .filter(|kept| kept.team == team)
    {
        for path in [
            sidecar_file(&kept.file),
            fragment_file(&kept.file),
            kept.file.clone(),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => done.removed.push(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        done.revoke.push((kept.token_id, kept.cabinet));
    }
    let directory = layout.engine_dir.join(TOKENS_DIR).join(team);
    if let Ok(entries) = std::fs::read_dir(&directory) {
        done.left = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        done.left.sort();
    }
    if done.left.is_empty() {
        match std::fs::remove_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", directory.display())),
        }
    }
    Ok(done)
}

/// The subcommand of the engine's binary that Claude Code runs for the authorization header.
pub const HEADERS_COMMAND: &str = "mcp-headers";

/// The line that registers the team's memory server with Claude Code, in the user's own
/// `~/.claude.json`, with `binary` as the helper that prints the authorization header on every
/// connection (`claude_code_config`). The token stands nowhere but its file: not in the terminal,
/// the shell's history or a chat, not in the arguments of a process, not in Claude Code's
/// configuration — and never in `settings.json`, whose managed copy the engine carries between
/// machines. The configuration is one quoted word: it came from the cabinet, and the line is pasted
/// into a shell.
#[must_use]
pub fn claude_code_registration(grant: &TokenGrant, binary: &Path) -> String {
    // the team and the agent passed the claim's name check: letters, digits and dashes only
    let helper = format!(
        "{} {HEADERS_COMMAND} {} {}",
        shell_word(binary),
        grant.team,
        grant.agent
    );
    format!(
        "claude mcp add-json --scope user {} {}",
        server_name(&grant.team),
        shell_word_for(&claude_code_config(grant, &helper), PathSyntax::Posix)
    )
}

/// The authorization header of the token kept for `team` and `agent`, as the helper prints it.
///
/// # Errors
///
/// A name that is not one, or a token that is not kept here.
pub fn headers_of(layout: &Layout, team: &str, agent: &str) -> Result<String, String> {
    if !vibememory_core::naming::is_slug(team) || !vibememory_core::naming::is_slug(agent) {
        return Err(format!(
            "{team:?} and {agent:?} are not a team and an agent"
        ));
    }
    let file = token_file(layout, team, agent);
    let token = std::fs::read_to_string(&file)
        .map_err(|error| format!("no token kept for {team}/{agent}: {error}"))?;
    Ok(vibememory_core::claim::authorization_header(token.trim()))
}
