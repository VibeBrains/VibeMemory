//! `vibememory-mcp`: the memory of the store, offered over MCP on stdio.
//!
//! One line in, one line out. Nothing is printed to stdout that is not a JSON-RPC response —
//! stdout *is* the protocol here, and a stray `println!` would break the client. Anything the
//! operator should see goes to stderr.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::io::{BufRead as _, Write as _};
use std::process::ExitCode;

use vibememory_mcp::git_memories::GitMemories;
use vibememory_mcp::memories::{Memories, from_engine, project_here};
use vibememory_mcp::protocol;
use vibememory_mcp::tools::Caller;

/// Who the records say wrote them, when the client does not introduce itself.
const DEFAULT_AGENT: &str = "mcp";

/// The shortest token accepted: a guessable token on a public port is no token at all.
const MIN_TOKEN_LENGTH: usize = 32;

/// The machine the host's writes are signed with, when `--machine` does not name one.
const DEFAULT_HOST_MACHINE: &str = "host";

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

    // The client's name if it gave one at launch; otherwise a name that at least says it came
    // over MCP rather than pretending to be Claude Code.
    let agent = flag("--agent").unwrap_or_else(|| DEFAULT_AGENT.to_owned());

    // On the store's host there is no engine and no working copy — only the bare repository.
    // A client reaches it over ssh (or through the HTTP front), and has no directory of ours to
    // name a project by, so every write names its own.
    if let Some(repo) = flag("--git-store") {
        let machine = flag("--machine").unwrap_or_else(|| DEFAULT_HOST_MACHINE.to_owned());
        let memories = GitMemories::new(repo.into(), machine);
        // Over HTTP, behind the TLS proxy: the token is read from a file, never from the command
        // line, where every user of the machine could read it in the process list.
        if let Some(address) = flag("--http") {
            let token = match flag("--token-file").map(std::fs::read_to_string) {
                Some(Ok(token)) => token.trim().to_owned(),
                Some(Err(error)) => {
                    eprintln!("vibememory-mcp: the token file cannot be read: {error}");
                    return ExitCode::FAILURE;
                }
                None => {
                    eprintln!("vibememory-mcp: --http needs --token-file");
                    return ExitCode::FAILURE;
                }
            };
            if token.len() < MIN_TOKEN_LENGTH {
                eprintln!(
                    "vibememory-mcp: the token is shorter than {MIN_TOKEN_LENGTH} characters"
                );
                return ExitCode::FAILURE;
            }
            let listener = match std::net::TcpListener::bind(&address) {
                Ok(listener) => listener,
                Err(error) => {
                    eprintln!("vibememory-mcp: cannot listen on {address}: {error}");
                    return ExitCode::FAILURE;
                }
            };
            eprintln!("vibememory-mcp: serving MCP over HTTP on {address}");
            vibememory_mcp::http::serve(&listener, &token, &memories);
            return ExitCode::FAILURE;
        }
        let caller = Caller {
            agent: &agent,
            project: None,
        };
        return serve(&memories, &caller);
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
    let caller = Caller {
        agent: &agent,
        project: project.as_deref(),
    };
    serve(&memories, &caller)
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
