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
use std::path::PathBuf;
use std::process::ExitCode;

use vibememory_mcp::memories::{Memories, from_engine};
use vibememory_mcp::protocol;

/// The environment variable that names the engine directory, same as the hooks use.
const ENGINE_DIR_VAR: &str = "VIBEMEMORY_DIR";
/// Who the records say wrote them, when the client does not introduce itself.
const DEFAULT_AGENT: &str = "mcp";

fn main() -> ExitCode {
    let Some(engine_dir) = engine_dir() else {
        eprintln!("vibememory-mcp: no HOME and no {ENGINE_DIR_VAR}; cannot find the store");
        return ExitCode::FAILURE;
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

    serve(&memories, &agent)
}

/// Reads requests until stdin closes.
fn serve(memories: &dyn Memories, agent: &str) -> ExitCode {
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
            Ok(request) => protocol::handle(&request, agent, memories),
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

/// Where the engine keeps its things: the variable the hooks set, else `~/.vibememory`.
fn engine_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(ENGINE_DIR_VAR)
        && !dir.trim().is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".vibememory"))
}
