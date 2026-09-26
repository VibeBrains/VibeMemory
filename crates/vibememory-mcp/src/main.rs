//! `vibememory-mcp`: the memory of the store, offered over MCP, and the host's own commands.
//!
//! On stdio — one line in, one line out — nothing is printed to stdout that is not a JSON-RPC
//! response: stdout *is* the protocol there, and a stray `println!` would break the client.
//! Anything the operator should see goes to stderr. The commands whose answer is stdout are
//! `access check` and `access teams`, which scripts and the cabinet read, and the key's `status`.
//!
//! On the host: `shell <key>` is the forced command of every machine key, `pre-receive` the hook
//! of every team store, `access-apply` applies the access snapshot, `status` writes the report.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::io::Write as _;
use std::process::ExitCode;

use vibememory_mcp::access::{self, Mode};
use vibememory_mcp::git_memories::GitMemories;
use vibememory_mcp::host::Host;
use vibememory_mcp::hostops::{self, ApplyPaths, HostPaths};
use vibememory_mcp::http;
use vibememory_mcp::layout;
use vibememory_mcp::memories::{Memories, for_directory, project_here};
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
/// How the `access` commands are called.
const ACCESS_USAGE: &str =
    "vibememory-mcp access check <file> | vibememory-mcp access teams <file> [--mode memory|sync]";
/// What `sshd` puts the command a key's client asked for in.
const ORIGINAL_COMMAND: &str = "SSH_ORIGINAL_COMMAND";

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
    match arguments.first().map(String::as_str) {
        Some("access") => return access_command(arguments.get(1..).unwrap_or_default()),
        Some("shell") => {
            // The key comes from the key line's forced command, never from the client: what the
            // client asked for is only ever read from the variable sshd sets.
            let Some(key) = arguments.get(1) else {
                eprintln!("vibememory-mcp: usage: vibememory-mcp shell <key id>");
                return ExitCode::FAILURE;
            };
            let original = std::env::var(ORIGINAL_COMMAND).unwrap_or_default();
            return hostops::shell(key, &original, &host_paths());
        }
        Some("pre-receive") => {
            let reserve = match flag("--reserve-bytes").map(|value| value.parse::<u64>()) {
                None => layout::DEFAULT_RESERVE_BYTES,
                Some(Ok(bytes)) => bytes,
                Some(Err(_)) => {
                    eprintln!("vibememory-mcp: --reserve-bytes takes a whole number of bytes");
                    return ExitCode::FAILURE;
                }
            };
            let max_content_bytes = match flag("--max-content-bytes")
                .map(|value| value.parse::<u64>())
            {
                None => layout::DEFAULT_MAX_CONTENT_BYTES,
                Some(Ok(bytes)) => bytes,
                Some(Err(_)) => {
                    eprintln!("vibememory-mcp: --max-content-bytes takes a whole number of bytes");
                    return ExitCode::FAILURE;
                }
            };
            let limits = hostops::PushLimits {
                reserve_bytes: reserve,
                max_content_bytes,
            };
            return hostops::pre_receive(&host_paths(), limits);
        }
        Some("access-apply") => {
            let apply_paths = ApplyPaths {
                authorized_keys: flag("--authorized-keys")
                    .unwrap_or_else(|| layout::AUTHORIZED_KEYS.to_owned())
                    .into(),
                store_init: flag("--store-init")
                    .unwrap_or_else(|| layout::STORE_INIT.to_owned())
                    .into(),
            };
            let catch_up = std::env::args().any(|arg| arg == "--catch-up");
            return hostops::access_apply(&host_paths(), &apply_paths, catch_up);
        }
        Some("status") => return hostops::status(&host_paths()),
        Some("export") => return hostops::export(&host_paths()),
        _ => {}
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
    // The directory the client started us in. Measured 2026-09-13: Claude Code starts its stdio
    // servers in the session's project directory — which also decides the store: a team's project
    // writes into the team's clone, signed as the member.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(problem) => {
            eprintln!("vibememory-mcp: no working directory: {problem}");
            return ExitCode::FAILURE;
        }
    };
    let (memories, member) = match for_directory(&engine_dir, &cwd) {
        Ok(chosen) => chosen,
        Err(problem) => {
            eprintln!("vibememory-mcp: {problem}");
            return ExitCode::FAILURE;
        }
    };
    // Not finding a project is not a failure — searches still work, and a write then has to name
    // its project.
    let project = match project_here(&engine_dir, memories.store(), &cwd) {
        Ok(project) => project,
        Err(problem) => {
            eprintln!("vibememory-mcp: no project for this directory: {problem}");
            None
        }
    };
    let caller = match member.as_deref() {
        Some(member) => Caller::member_of_team(&agent, member, project.as_deref()),
        None => Caller::owner(&agent, project.as_deref()),
    };
    serve(&memories, &caller)
}

/// Where the host's commands find the snapshot and the team stores: the host's layout, or another
/// copy of it named by `--access` and `--teams`.
fn host_paths() -> HostPaths {
    HostPaths {
        access: flag("--access")
            .unwrap_or_else(|| layout::ACCESS_FILE.to_owned())
            .into(),
        teams: flag("--teams")
            .unwrap_or_else(|| layout::TEAMS_DIR.to_owned())
            .into(),
    }
}

/// `access check <file>` and `access teams <file> [--mode memory|sync]`.
fn access_command(rest: &[String]) -> ExitCode {
    match rest {
        [command, file] if command == "check" => check_command(file),
        [command, file, options @ ..] if command == "teams" => teams_command(file, options),
        _ => say(CHECK_UNREADABLE, "usage", ACCESS_USAGE),
    }
}

/// `access check <file>`: the checker the cabinet runs on every snapshot before it publishes it.
/// First line of stdout — `ok` or the code of the first rule broken; second — what exactly.
fn check_command(file: &str) -> ExitCode {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) => {
            return say(
                CHECK_UNREADABLE,
                "snapshotUnreadable",
                &format!("{file}: {error}"),
            );
        }
    };
    match access::check(&bytes) {
        Ok(snapshot) => say(
            0,
            "ok",
            &format!(
                "{} teams, {} tokens, {} machine keys",
                snapshot.teams.len(),
                snapshot.tokens.len(),
                snapshot.keys.len()
            ),
        ),
        Err(refusal) => say(CHECK_REFUSED, refusal.code, &refusal.detail),
    }
}

/// `access teams <file> [--mode memory|sync]`: the slugs of the live teams with a store under
/// `teams/`, one per line — the nightly backup bundles the `memory` ones. The teams are those of the
/// snapshot in force: `<file>`, unless the copy of the snapshot applied last beside it is newer. A
/// snapshot that does not pass the check lists nothing and fails: a backup of a guessed list would
/// look complete.
fn teams_command(file: &str, options: &[String]) -> ExitCode {
    let mode = match options {
        [] => None,
        [flag, value] if flag == "--mode" && value == "memory" => Some(Mode::Memory),
        [flag, value] if flag == "--mode" && value == "sync" => Some(Mode::Sync),
        _ => return say(CHECK_UNREADABLE, "usage", ACCESS_USAGE),
    };
    let snapshot = match hostops::read_in_force(std::path::Path::new(file), hostops::journal) {
        Ok(snapshot) => snapshot,
        Err(why) => {
            eprintln!("vibememory-mcp: the access snapshot cannot be used: {why}");
            return ExitCode::from(CHECK_UNREADABLE);
        }
    };
    let mut out = std::io::stdout().lock();
    for (slug, _) in snapshot.teams.iter().filter(|(_, team)| {
        !team.adopted && team.deleted.is_none() && mode.is_none_or(|mode| team.mode == Some(mode))
    }) {
        if writeln!(out, "{slug}").is_err() {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// The two lines of an answer, and its exit code. A reader that stops after the first line —
/// `| head -n 1` — closes the pipe before the second: that must not turn into a panic, whose exit
/// code would say something else than the verdict.
fn say(code: u8, first: &str, second: &str) -> ExitCode {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{first}").and_then(|()| writeln!(out, "{second}"));
    let _ = out.flush();
    ExitCode::from(code)
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
    match protocol::serve_lines(
        stdin.lock(),
        std::io::stdout(),
        caller,
        memories,
        &mut |_, _| {},
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("vibememory-mcp: {problem}");
            ExitCode::FAILURE
        }
    }
}
