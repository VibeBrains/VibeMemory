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
use vibememory_cli::hook::session_start;
use vibememory_cli::hook::stop::{
    PUSH_DEBOUNCE, commit_snapshot, push_if_due, record_end, record_progress,
};
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
        Some("hook") => match args.next().as_deref() {
            Some("session-start") => session_start_hook(),
            Some("stop") => session_progress_hook(false),
            Some("session-end") => session_progress_hook(true),
            other => {
                eprintln!("unknown hook {other:?}; try session-start, stop or session-end");
                ExitCode::from(2)
            }
        },
        Some(other) => {
            eprintln!("unknown command {other:?}; try status, doctor or install");
            ExitCode::from(2)
        }
        None => {
            println!("vibememory {}", env!("CARGO_PKG_VERSION"));
            println!("commands: status, doctor, install [--dry-run], hook session-start");
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
