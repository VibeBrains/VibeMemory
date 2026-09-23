//! `vibememory-mcp`: the memory of the store, offered over MCP.
//!
//! On stdio — one line in, one line out — nothing is printed to stdout that is not a JSON-RPC
//! response: stdout *is* the protocol there, and a stray `println!` would break the client.
//! Anything the operator should see goes to stderr. `access check` is the one command whose
//! answer is stdout: the cabinet reads its first line.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::io::{BufRead as _, Write as _};
use std::process::ExitCode;

use vibememory_mcp::access;
use vibememory_mcp::git_memories::GitMemories;
use vibememory_mcp::host::Host;
use vibememory_mcp::http;
use vibememory_mcp::memories::{Memories, from_engine, project_here};
use vibememory_mcp::protocol;
use vibememory_mcp::tools::Caller;

/// Who the records say wrote them, when a stdio client does not introduce itself.
const DEFAULT_AGENT: &str = "mcp";

/// The machine the host's writes are signed with, when `--machine` does not name one.
const DEFAULT_HOST_MACHINE: &str = "host";

/// Exit code of `access check` when the file is read and the snapshot breaks a rule.
const CHECK_REFUSED: u8 = 1;
/// Exit code of `access check` when the file cannot be read at all, or the command is misused.
const CHECK_UNREADABLE: u8 = 2;

/// The value after a flag on the command line.
fn flag(name: &str) -> Option<String> {
    std::env::args()
        .skip_while(|argument| argument != name)
        .nth(1)
}

fn main() -> ExitCode {
    // Answered before anything else, and without touching the store: `install` asks it to prove
    // that what it just placed actually runs. "Installed" is not the same as "works" — on macOS a
    // binary written over in place is killed by the kernel with no message at all.
    if std::env::args().any(|argument| argument == "--version") {
        println!("vibememory-mcp {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("access") {
        return access_command(arguments.get(1..).unwrap_or_default());
    }

    // Over HTTP every request brings its own token, and the token says who it is; nothing about
    // the caller comes from the command line.
    if let Some(address) = flag("--http") {
        return serve_http(&address);
    }

    // The client's name if it gave one at launch; otherwise a name that at least says it came
    // over MCP rather than pretending to be Claude Code.
    let agent = flag("--agent").unwrap_or_else(|| DEFAULT_AGENT.to_owned());

    // The owner over ssh, on the store's host: no engine and no working copy there — only the bare
    // repository — and no directory of ours to name a project by, so every write names its own.
    if let Some(repo) = flag("--git-store") {
        let machine = flag("--machine").unwrap_or_else(|| DEFAULT_HOST_MACHINE.to_owned());
        let memories = GitMemories::new(repo.into(), machine);
        return serve(&memories, &Caller::owner(&agent, None));
    }

    // The engine's own rule for where it lives, not a second copy of it: the server used to read
    // `HOME` itself, which on Windows exists only inside Git Bash.
    let engine_dir = match vibememory_cli::install::engine_dir_from_environment() {
        Ok(engine_dir) => engine_dir,
        Err(problem) => {
            eprintln!("vibememory-mcp: {problem}; cannot find the store");
            return ExitCode::FAILURE;
        }
    };
    let config_dir = engine_dir.clone();
    let memories = match from_engine(&engine_dir, &config_dir) {
        Ok(memories) => memories,
        Err(problem) => {
            eprintln!("vibememory-mcp: {problem}");
            return ExitCode::FAILURE;
        }
    };

    // The project of the directory the client started us in. Measured 2026-09-13: Claude Code
    // starts its stdio servers in the session's project directory. Not finding one is not a
    // failure — searches still work, and a write then has to name its project.
    let project = match std::env::current_dir()
        .map_err(|error| error.to_string())
        .and_then(|cwd| project_here(&engine_dir, &cwd))
    {
        Ok(project) => project,
        Err(problem) => {
            eprintln!("vibememory-mcp: no project for this directory: {problem}");
            None
        }
    };
    serve(&memories, &Caller::owner(&agent, project.as_deref()))
}

/// `access check <file>`: the checker the cabinet runs on every snapshot before it publishes it.
/// First line of stdout — `ok` or the code of the first rule broken; second — what exactly.
fn access_command(rest: &[String]) -> ExitCode {
    let [command, file] = rest else {
        println!("usage\nvibememory-mcp access check <file>");
        return ExitCode::from(CHECK_UNREADABLE);
    };
    if command != "check" {
        println!("usage\nvibememory-mcp access check <file>");
        return ExitCode::from(CHECK_UNREADABLE);
    }
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!("snapshotUnreadable\n{file}: {error}");
            return ExitCode::from(CHECK_UNREADABLE);
        }
    };
    match access::check(&bytes) {
        Ok(snapshot) => {
            println!(
                "ok\n{} teams, {} tokens, {} machine keys",
                snapshot.teams.len(),
                snapshot.tokens.len(),
                snapshot.keys.len()
            );
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            println!("{}\n{}", refusal.code, refusal.detail);
            ExitCode::from(CHECK_REFUSED)
        }
    }
}

/// Serves MCP over HTTP behind the TLS proxy, with the rights of the access snapshot.
fn serve_http(address: &str) -> ExitCode {
    let (Some(access), Some(teams)) = (flag("--access"), flag("--teams")) else {
        eprintln!("vibememory-mcp: --http needs --access <snapshot> and --teams <directory>");
        return ExitCode::FAILURE;
    };
    let connections = match flag("--connections").map(|value| value.parse::<usize>()) {
        None => http::DEFAULT_CONNECTIONS,
        Some(Ok(connections)) if connections > 0 => connections,
        Some(_) => {
            eprintln!("vibememory-mcp: --connections takes a whole number above zero");
            return ExitCode::FAILURE;
        }
    };
    let host = match Host::open(access.into(), teams.into(), flag("--cabinet")) {
        Ok(host) => host,
        Err(problem) => {
            eprintln!("vibememory-mcp: the access snapshot cannot be used: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match std::net::TcpListener::bind(address) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("vibememory-mcp: cannot listen on {address}: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!(
        "vibememory-mcp: serving MCP over HTTP on {address}, {connections} connections at once"
    );
    http::serve(&listener, &host, connections);
    ExitCode::FAILURE
}

/// Reads requests until stdin closes.
fn serve(memories: &dyn Memories, caller: &Caller<'_>) -> ExitCode {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("vibememory-mcp: {error}");
                return ExitCode::FAILURE;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match protocol::parse(&line) {
            Ok(request) => protocol::handle(&request, caller, memories),
            Err(detail) => Some(protocol::malformed(&detail)),
        };
        let Some(response) = response else {
            continue; // a notification: answered by saying nothing
        };
        let Ok(text) = serde_json::to_string(&response) else {
            eprintln!("vibememory-mcp: a response could not be encoded");
            continue;
        };
        if writeln!(stdout, "{text}").is_err() || stdout.flush().is_err() {
            // The client went away mid-answer; that is its business, not a failure of ours.
            return ExitCode::SUCCESS;
        }
    }
    ExitCode::SUCCESS
}
