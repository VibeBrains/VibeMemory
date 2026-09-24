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

use vibememory_core::claim::{TokenGrant, client_fragment, server_name, token_sidecar};
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
/// is not an error here: they are the exit code and stdout, for
/// [`vibememory_core::claim::read_answer`], and curl's own words on stderr for a person.
pub fn ask_cabinet(cabinet: &str, code: &str) -> Result<CabinetReply, String> {
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
            "--fail",
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
    let body = serde_json::json!({ "code": code }).to_string();
    child
        .stdin
        .take()
        .ok_or_else(|| "curl took no input".to_owned())?
        .write_all(body.as_bytes())
        .map_err(|error| format!("the code could not be handed to curl: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl did not finish: {error}"))?;
    Ok(CabinetReply {
        exit: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// What curl left of a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CabinetReply {
    /// curl's exit code: 0, 22 for a refusal, anything else for a cabinet not reached.
    pub exit: i32,
    /// The answer.
    pub stdout: String,
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
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
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

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<(), String> {
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
#[cfg(not(unix))]
fn restrict_directory(path: &Path) -> Result<(), String> {
    owner_only(path, "(OI)(CI)F")
}

#[cfg(not(unix))]
fn owner_only(path: &Path, rights: &str) -> Result<(), String> {
    let user = std::env::var("USERNAME").map_err(|_| "USERNAME is not set".to_owned())?;
    let status = Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{user}:{rights}")])
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

/// The line that registers the team's memory server with Claude Code, in the user's own
/// `~/.claude.json`. The token is read from its file when the line runs: it never stands in the
/// terminal, the shell's history or a chat — and never in `settings.json`, whose managed copy the
/// engine carries between machines. The address is quoted like the path: it came from the cabinet,
/// and the line is pasted into a shell.
#[must_use]
pub fn claude_code_registration(grant: &TokenGrant, token: &Path) -> String {
    format!(
        "claude mcp add --scope user --transport http {} {} --header \"Authorization: Bearer $(cat {})\"",
        server_name(&grant.team),
        // an address, not a path: quoted as it is, with no separator turned around
        shell_word_for(&grant.mcp_url, PathSyntax::Posix),
        shell_word(token)
    )
}
