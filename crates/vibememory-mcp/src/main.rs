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

use vibememory_mcp::memories::{Memories, from_engine, project_here};
use vibememory_mcp::protocol;
use vibememory_mcp::tools::Caller;

/// Who the records say wrote them, when the client does not introduce itself.
const DEFAULT_AGENT: &str = "mcp";

fn main() -> ExitCode {
    // Answered before anything else, and without touching the store: `install` asks it to prove
    // that what it just placed actually runs. "Installed" is not the same as "works" — on macOS a
    // binary written over in place is killed by the kernel with no message at all.
    if std::env::args().any(|argument| argument == "--version") {
        println!("vibememory-mcp {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
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

    // The client's name if it gave one at launch; otherwise a name that at least says it came
    // over MCP rather than pretending to be Claude Code.
    let agent = std::env::args()
        .skip_while(|argument| argument != "--agent")
        .nth(1)
        .unwrap_or_else(|| DEFAULT_AGENT.to_owned());

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
