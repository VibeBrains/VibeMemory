//! The binary. Everything it decides lives in the library next to it.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::path::PathBuf;
use std::process::ExitCode;

use vibememory_cli::config::Config;
use vibememory_cli::hook::parse_input;
use vibememory_cli::hook::prompt_gate::{Gate, decide};
use vibememory_cli::hook::session_start;
use vibememory_cli::hook::stop::{
    PUSH_DEBOUNCE, commit_snapshot, push_if_due, record_end, record_progress,
};
use vibememory_cli::hook::stop::{Tail, Tails};
use vibememory_cli::install::{Layout, State, apply, plan};
use vibememory_cli::store::existing;
use vibememory_core::naming::{
    IgnoreReason, NamingInput, PathSyntax, Resolution, canonical_cwd, enc_from_transcript_path,
    resolve_store_name,
};

/// The engine's own directory, under the home directory unless told otherwise.
const ENGINE_DIR_VAR: &str = "VIBEMEMORY_DIR";
/// Where Claude Code keeps its configuration.
const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("status") => report(false),
        Some("doctor") => report(true),
        Some("install") => {
            let dry_run = args.any(|arg| arg == "--dry-run");
            install(dry_run)
        }
        Some("forget") => forget_command(args.next().as_deref()),
        Some("merge-driver") => merge_driver_command(&args.collect::<Vec<String>>()),
        Some("hook") => match args.next().as_deref() {
            Some("session-start") => session_start_hook(),
            Some("stop") => session_progress_hook(false),
            Some("session-end") => session_progress_hook(true),
            Some("user-prompt-submit") => prompt_gate_hook(),
            other => {
                eprintln!(
                    "unknown hook {other:?}; try session-start, stop, session-end or \
                     user-prompt-submit"
                );
                ExitCode::from(2)
            }
        },
        Some(other) => {
            eprintln!("unknown command {other:?}; try status, doctor or install");
            ExitCode::from(2)
        }
        None => {
            println!("vibememory {}", env!("CARGO_PKG_VERSION"));
            println!(
                "commands: status, doctor, install [--dry-run], hook <event>, \
                 merge-driver <jsonl|keepboth> %O %A %B %P, forget <session-id>"
            );
            ExitCode::SUCCESS
        }
    }
}

/// The paths this machine works with. Both are overridable so that nothing but a deliberate run
/// ever touches the real directories.
fn layout() -> Layout {
    let home = std::env::var("HOME").unwrap_or_default();
    Layout {
        config_dir: std::env::var(CONFIG_DIR_VAR)
            .map_or_else(|_| PathBuf::from(&home).join(".claude"), PathBuf::from),
        engine_dir: std::env::var(ENGINE_DIR_VAR)
            .map_or_else(|_| PathBuf::from(&home).join(".vibememory"), PathBuf::from),
    }
}

/// Reads the configuration, or explains why the engine will not start on it.
fn read_config(layout: &Layout) -> Result<Config, String> {
    let path = layout.engine_dir.join("config.json");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    Config::parse(&text, PathSyntax::Posix).map_err(|error| error.to_string())
}

/// `status` prints where the machine stands; `doctor` does the same and fails when something is
/// not as it must be.
fn report(strict: bool) -> ExitCode {
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let actions = plan(&layout, &config, &[]);
    let mut wrong = 0;
    for action in &actions {
        let mark = match &action.state {
            State::Satisfied => "ok      ",
            State::Missing => {
                wrong += 1;
                "missing "
            }
            State::Conflict { .. } => {
                wrong += 1;
                "conflict"
            }
            State::Unknown { .. } => {
                wrong += 1;
                "unknown "
            }
        };
        println!("{mark} {}", action.step.describe());
        match &action.state {
            State::Conflict { found } => println!("         found: {found}"),
            State::Unknown { reason } => println!("         {reason}"),
            _ => {}
        }
    }
    if strict && wrong > 0 {
        eprintln!("{wrong} of {} steps are not in place", actions.len());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Brings the machine to the planned state, or says what it would do.
fn install(dry_run: bool) -> ExitCode {
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let applied = apply(&layout, &plan(&layout, &config, &[]), dry_run);
    let verb = if dry_run { "would do" } else { "did" };
    for what in &applied.performed {
        println!("{verb}: {what}");
    }
    for what in &applied.untouched {
        println!("already in place: {what}");
    }
    for (what, found) in &applied.refused {
        println!("left alone: {what} — found {found}");
    }
    for (what, error) in &applied.failed {
        eprintln!("failed: {what} — {error}");
    }
    if applied.is_complete() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// How long the hook waits for git before deciding without it. The CLI gives a `SessionStart`
/// hook ten seconds; spending most of that on one command would risk the session.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Set by the CLI when every working directory of a run shares one project directory.
const PROJECT_DIR_NAME_VAR: &str = "CLAUDE_CODE_PROJECT_DIR_NAME";

/// `SessionStart`: put the link in place before the CLI creates a real directory.
///
/// Always exits 0. A hook that fails takes the session with it, and no synchronisation is worth
/// that; whatever went wrong is said through `additionalContext` instead.
fn session_start_hook() -> ExitCode {
    let mut text = String::new();
    if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        return say(&format!("VibeMemory could not read its input: {error}"));
    }
    let input = match parse_input(&text) {
        Ok(input) => input,
        Err(error) => return say(&error.message()),
    };

    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => return say(&format!("VibeMemory is not configured: {error}")),
    };

    let syntax = session_start::host_syntax();
    // The encoded directory name comes from the transcript path the CLI itself reports: the
    // encoding is its answer, never our guess.
    let enc = match enc_from_transcript_path(&input.transcript_path, syntax) {
        Ok(enc) => enc,
        Err(error) => {
            return say(&format!(
                "VibeMemory could not read the transcript path: {error}"
            ));
        }
    };
    let cwd = canonical_cwd(&input.cwd, syntax);
    let names = existing(&layout.store(), GIT_TIMEOUT);
    // The CLI shares one project directory between every working directory of a run when this is
    // set, so the core has to see it: without it the engine would name a store per directory and
    // link them all to the same place.
    let project_dir_name = std::env::var(PROJECT_DIR_NAME_VAR).ok();
    let naming = NamingInput {
        cwd: &cwd,
        syntax,
        project_dir_name: project_dir_name.as_deref(),
    };
    let resolution = resolve_store_name(&naming, || probe_git(&cwd), &config.naming, &names.names);

    let decision = match resolution {
        Ok(Resolution::Named { name, .. }) => {
            session_start::decide(&layout, &enc, Some(&name), None)
        }
        Ok(Resolution::Ignored { reason }) => {
            let reason = match reason {
                IgnoreReason::Pattern(pattern) => format!("ignoreCwd matched {pattern}"),
                IgnoreReason::ProjectDirName(name) => {
                    format!("CLAUDE_CODE_PROJECT_DIR_NAME is {name}")
                }
            };
            session_start::decide(&layout, &enc, None, Some(&reason))
        }
        Err(error) => return say(&format!("VibeMemory could not name this project: {error}")),
    };

    if let Err(error) = session_start::perform(&layout, &decision) {
        return say(&format!(
            "VibeMemory could not put the link in place: {error}"
        ));
    }
    match decision.additional_context() {
        Some(message) => say(&message),
        None => ExitCode::SUCCESS,
    }
}

/// Runs git for the naming rules, in the canonical working directory.
fn probe_git(cwd: &str) -> vibememory_core::naming::GitProbe {
    vibememory_cli::git::probe(std::path::Path::new(cwd), GIT_TIMEOUT)
}

/// Says one sentence to the session and exits 0, which is the only exit code a hook may use.
fn say(message: &str) -> ExitCode {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": message,
        }
    });
    println!("{payload}");
    ExitCode::SUCCESS
}

/// `Stop` and `SessionEnd`: commit what the session has written so far.
///
/// Both events do the same work, because neither can be relied on alone: a session whose turn
/// fails produces `SessionEnd` and no `Stop` (measured on 2.1.258), while a long session produces
/// many `Stop`s and one `SessionEnd` at the very end. Committing twice costs nothing — an
/// unchanged snapshot makes no commit — and missing one costs the session's history.
///
/// Exits 0 like every hook. The snapshot is cut at the last newline, so what is committed is
/// always a whole number of records; the live file itself is never handed to git.
fn session_progress_hook(ended: bool) -> ExitCode {
    let mut text = String::new();
    if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        return say(&format!("VibeMemory could not read its input: {error}"));
    }
    let input = match parse_input(&text) {
        Ok(input) => input,
        Err(error) => return say(&error.message()),
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => return say(&format!("VibeMemory is not configured: {error}")),
    };

    // The transcript is reached through the link, so its real path is inside the store — unless
    // this session is one the hook could not link, and then there is nothing to commit here.
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        return ExitCode::SUCCESS;
    };
    let transcript = std::path::PathBuf::from(&input.transcript_path);
    let Ok(real) = std::fs::canonicalize(&transcript) else {
        return ExitCode::SUCCESS;
    };
    let Ok(relative) = real.strip_prefix(&store) else {
        // A session in a real directory: the tick imports it, and saying so on every stop would
        // be noise, since SessionStart already said it once.
        return ExitCode::SUCCESS;
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    let stamp = vibememory_cli::clock::now();

    if let Err(error) = commit_snapshot(&store, &real, &relative, &stamp) {
        return say(&format!(
            "VibeMemory could not commit this session: {error}"
        ));
    }
    let cwd = portable_cwd(&config, &input.cwd);
    if let Err(error) = record_progress(
        &store,
        &config.machine_id,
        &input.session_id,
        &cwd,
        &real,
        &stamp,
    ) {
        return say(&format!("VibeMemory could not record progress: {error}"));
    }
    if ended && let Err(error) = record_end(&store, &config.machine_id, &input.session_id) {
        return say(&format!("VibeMemory could not close this session: {error}"));
    }
    // A push that does not happen costs nothing here: the commit is already on this disk, and the
    // tick pushes again in two minutes.
    let _ = push_if_due(&store, &layout.engine_dir, epoch_seconds(), PUSH_DEBOUNCE);
    ExitCode::SUCCESS
}

/// The working directory in the form other machines can read, or the local one when no root
/// covers it — a path nobody can translate is still better in a log than nothing.
fn portable_cwd(config: &Config, cwd: &str) -> String {
    let roots = vibememory_core::desktop::roots::Roots::new(
        config.roots.clone().into_iter().collect(),
        PathSyntax::Posix,
    );
    roots.to_portable(cwd).unwrap_or_else(|_| cwd.to_owned())
}

/// Seconds since the epoch, for the push debounce.
fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `UserPromptSubmit`: hold a prompt back only when another machine is provably ahead.
///
/// Every failure path here lets the prompt through. A gate that stops somebody because a file
/// could not be read is a gate that gets switched off, and then it guards nothing.
fn prompt_gate_hook() -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_err() {
        return ExitCode::SUCCESS;
    }
    let Ok(input) = parse_input(&text) else {
        return ExitCode::SUCCESS;
    };
    let layout = layout();
    let Ok(config) = read_config(&layout) else {
        return ExitCode::SUCCESS;
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        return ExitCode::SUCCESS;
    };

    // An unreadable local transcript must never become a block: `0 lines` would make every other
    // machine look ahead. Not knowing our own state is exactly the doubt this gate resolves in
    // favour of the person typing.
    let local_lines = match std::fs::read(&input.transcript_path) {
        Ok(bytes) => vibememory_core::merge::jsonl::survey(&bytes).lines,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => return ExitCode::SUCCESS,
    };
    let others = other_machines_tails(&store, &config.machine_id, &input.session_id);

    let gate = decide(local_lines, &others);
    match gate {
        Gate::Allow => ExitCode::SUCCESS,
        Gate::Block { .. } => {
            let Some(reason) = gate.reason() else {
                return ExitCode::SUCCESS;
            };
            let payload = serde_json::json!({
                "decision": "block",
                "reason": reason,
            });
            println!("{payload}");
            ExitCode::SUCCESS
        }
    }
}

/// What the other machines report about this session, from what the tick has already fetched.
/// Anything unreadable is simply absent: it may not turn into a block.
fn other_machines_tails(
    store: &std::path::Path,
    own_machine: &str,
    session_id: &str,
) -> Vec<(String, Tail)> {
    let Ok(entries) = std::fs::read_dir(store.join("machines")) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let machine = entry.file_name().to_string_lossy().into_owned();
        if machine == own_machine {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("tails.json")) else {
            continue;
        };
        let Ok(tails) = serde_json::from_str::<Tails>(&text) else {
            continue;
        };
        if let Some(tail) = tails.sessions.get(session_id) {
            found.push((machine, tail.clone()));
        }
    }
    found
}

/// `merge-driver jsonl|keepboth %O %A %B %P` — what git spawns while merging.
///
/// Exit 0 means the result is in `%A`; exit 1 means the merge could not be done and `%A` was left
/// exactly as git wrote it, so the caller can abort with a clean working tree.
fn merge_driver_command(args: &[String]) -> ExitCode {
    let [name, base, ours, theirs, path] = args else {
        eprintln!("usage: vibememory merge-driver <jsonl|keepboth> %O %A %B %P");
        return ExitCode::FAILURE;
    };
    let Some(driver) = vibememory_cli::merge_driver::Driver::parse(name) else {
        eprintln!("unknown driver {name:?}: expected jsonl or keepboth");
        return ExitCode::FAILURE;
    };
    let layout = layout();
    let machine = read_config(&layout).map_or_else(|_| "unknown".to_owned(), |c| c.machine_id);
    let stamp = format!(
        "{machine}-{}",
        vibememory_cli::clock::now().replace(':', "-")
    );

    match vibememory_cli::merge_driver::run(
        driver,
        std::path::Path::new(base),
        std::path::Path::new(ours),
        std::path::Path::new(theirs),
        path,
        &stamp,
    ) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            // The message goes to git's stderr, which ends up in the tick's log. `%A` is
            // untouched, so `git merge --abort` restores the tree exactly.
            eprintln!("vibememory merge-driver: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `forget <session-id>` — ask every machine to drop a session.
///
/// Writes a tombstone into this machine's outbox and stops there. The removal itself belongs to
/// the tick: deleting the file here would meet the other machine's copy as a tree conflict that
/// no merge driver is ever asked about.
fn forget_command(session_id: Option<&str>) -> ExitCode {
    let Some(session_id) = session_id else {
        eprintln!("usage: vibememory forget <session-id>");
        return ExitCode::from(2);
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        eprintln!("the store is not there yet; run `vibememory install` first");
        return ExitCode::FAILURE;
    };
    let path = find_transcript(&store, session_id).unwrap_or_default();
    match vibememory_cli::forget::forget(
        &store,
        &config.machine_id,
        session_id,
        &path,
        &vibememory_cli::clock::now(),
    ) {
        Ok(()) => {
            println!("recorded: {session_id} will be removed on every machine at its next tick");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("forget: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Where a session's transcript sits inside the store, if this machine has it.
fn find_transcript(store: &std::path::Path, session_id: &str) -> Option<String> {
    let projects = store.join("projects");
    for project in std::fs::read_dir(projects).ok()?.filter_map(Result::ok) {
        let candidate = project.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            let relative = candidate.strip_prefix(store).ok()?;
            return Some(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    None
}
