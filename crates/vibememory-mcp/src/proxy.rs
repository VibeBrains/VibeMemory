//! The local server in a directory routed to a team whose memory lives only on the host: every
//! request goes to the team's server over HTTPS with the member's token, and the answer comes back
//! as it is.
//!
//! The agent keeps one server, `vibememory`, whatever the directory: which memory it reaches is the
//! directory's route, not the agent's guess between a server per team. The host cannot see the
//! directory, so a write without a project gets the one the engine's naming rules give it here.
//!
//! curl carries the request, as it carries `connect`'s: there is no TLS in this binary. The token
//! reaches curl through a private file, never through its arguments, which every user of the
//! machine sees in the process list.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

/// The tools whose `project` defaults to the directory's: the ones that write.
const WRITES: &[&str] = &["memory_save", "memory_update", "memory_delete"];

/// How long one request to the host may take.
const REQUEST_TIMEOUT_SECONDS: &str = "30";

/// The team's server and the member's key to it.
#[derive(Debug, Clone)]
pub struct Remote {
    /// The address of the team's MCP endpoint.
    pub url: String,
    /// The `Authorization` header line, as `mcp-headers` gives it.
    pub authorization: String,
    /// The project of the directory the server was started in, if the naming rules give one.
    pub project: Option<String>,
    /// Where the private curl configuration goes while the server runs.
    pub scratch: PathBuf,
}

/// A request line with the directory's project put into a write that names none. Anything that is
/// not such a write — or not JSON at all — goes on exactly as it came.
#[must_use]
pub fn with_project(line: &str, project: Option<&str>) -> String {
    let Some(project) = project else {
        return line.to_owned();
    };
    let Ok(mut request) = serde_json::from_str::<Value>(line) else {
        return line.to_owned();
    };
    let is_write = request.get("method").and_then(Value::as_str) == Some("tools/call")
        && request
            .pointer("/params/name")
            .and_then(Value::as_str)
            .is_some_and(|name| WRITES.contains(&name));
    if !is_write {
        return line.to_owned();
    }
    let Some(arguments) = request
        .pointer_mut("/params/arguments")
        .and_then(Value::as_object_mut)
    else {
        return line.to_owned();
    };
    if arguments.contains_key("project") {
        return line.to_owned();
    }
    arguments.insert("project".to_owned(), Value::String(project.to_owned()));
    request.to_string()
}

/// Serves stdin to the team's server until stdin closes: one request per line, one answer per
/// line. A notification is answered by the host with no body, and so here with nothing.
///
/// # Errors
///
/// Input that cannot be read, or a curl configuration that cannot be written.
pub fn serve(remote: &Remote, input: impl BufRead, mut output: impl Write) -> Result<(), String> {
    let config = CurlConfig::write(&remote.scratch, &remote.authorization)?;
    for line in input.lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let body = with_project(&line, remote.project.as_deref());
        let answer = match post(&remote.url, &config.path, &body) {
            Ok(answer) => answer,
            Err(problem) => failed(&line, &problem),
        };
        if answer.trim().is_empty() {
            continue;
        }
        if writeln!(output, "{}", answer.trim_end()).is_err() || output.flush().is_err() {
            return Ok(());
        }
    }
    Ok(())
}

/// A JSON-RPC error for a request the host could not be asked, so the agent hears why instead of
/// waiting for ever; nothing for a notification.
fn failed(line: &str, problem: &str) -> String {
    let Some(id) = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|request| request.get("id").cloned())
    else {
        return String::new();
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": -32603, "message": format!("the team's memory server was not reached: {problem}") },
    })
    .to_string()
}

/// One request to the host. The answer's body, or why there is none.
fn post(url: &str, config: &Path, body: &str) -> Result<String, String> {
    let protocol = if url.starts_with("https://") {
        "=https"
    } else {
        "=http"
    };
    let mut child = Command::new("curl")
        .arg("-q")
        .args(["--proto", protocol, "--silent", "--show-error"])
        .args(["--max-time", REQUEST_TIMEOUT_SECONDS])
        .arg("--config")
        .arg(config)
        .args([
            "-H",
            "Content-Type: application/json",
            "-H",
            "Accept: application/json, text/event-stream",
            "--write-out",
            "\n%{http_code}",
            "--data-binary",
            "@-",
            url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "curl took no input".to_owned())?
        .write_all(body.as_bytes())
        .map_err(|error| error.to_string())?;
    let done = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    let stdout = String::from_utf8_lossy(&done.stdout);
    let (answer, status) = stdout.rsplit_once('\n').unwrap_or(("", &stdout));
    match status.trim() {
        "200" | "202" => Ok(answer.to_owned()),
        "401" | "403" => Err("the token was refused — it may be revoked; connect again".to_owned()),
        other if done.status.success() => Err(format!("the server answered {other}")),
        _ => Err(String::from_utf8_lossy(&done.stderr).trim().to_owned()),
    }
}

/// curl's configuration with the header that carries the token, readable by the owner alone and
/// gone when the server stops.
struct CurlConfig {
    path: PathBuf,
}

impl CurlConfig {
    fn write(scratch: &Path, authorization: &str) -> Result<Self, String> {
        std::fs::create_dir_all(scratch).map_err(|error| error.to_string())?;
        let path = scratch.join(format!(".curl-{}", std::process::id()));
        let quoted = authorization.replace('\\', "\\\\").replace('"', "\\\"");
        vibememory_cli::connect::write_private(
            &path,
            format!("header = \"{quoted}\"\n").as_bytes(),
        )?;
        Ok(Self { path })
    }
}

impl Drop for CurlConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
