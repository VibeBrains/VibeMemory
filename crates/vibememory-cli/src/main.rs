//! The binary. Everything it decides lives in the library next to it.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::path::PathBuf;
use std::process::ExitCode;

use vibememory_cli::config::Config;
use vibememory_cli::install::{Layout, State, apply, plan};
use vibememory_core::naming::PathSyntax;

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
        Some(other) => {
            eprintln!("unknown command {other:?}; try status, doctor or install");
            ExitCode::from(2)
        }
        None => {
            println!("vibememory {}", env!("CARGO_PKG_VERSION"));
            println!("commands: status, doctor, install [--dry-run]");
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
