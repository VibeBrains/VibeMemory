//! `SessionEnd` of a session the hook could not link: its transcript is in a real directory the tick has yet to
//! import, and it has nothing to commit — but it was put on the live list, and has to come off it.

// The test writes files and runs the engine's binary, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fs;
use std::io::Write as _;
use std::path::Path;

use support::TempDir;

const MACHINE: &str = "mac-main";
const OLD: &str = "11111111-1111-4111-8111-111111111111";
const NEW: &str = "22222222-2222-4222-8222-222222222222";

/// Runs one hook of the engine the way Claude Code does: the event on stdin, the directories in the environment
/// The engine directory is not `$HOME/.vibememory`: nothing a test runs may reach the system's scheduler
fn hook(home: &Path, subcommand: &str, input: &str) {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
        .args(["hook", subcommand])
        .env("HOME", home)
        .env("VIBEMEMORY_DIR", home.join("engine"))
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the engine binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("the hook reads its input");
    let out = child.wait_with_output().expect("the hook answers");
    assert!(out.status.success(), "a hook exits 0: {out:?}");
}

fn live(home: &Path) -> String {
    fs::read_to_string(
        home.join("engine/store/machines")
            .join(MACHINE)
            .join("live.json"),
    )
    .unwrap_or_default()
}

#[test]
fn a_session_that_ended_in_a_real_directory_is_no_longer_live() {
    let temp = TempDir::new("hook-session-end-real");
    let home = temp.path().to_path_buf();
    fs::create_dir_all(home.join("engine/store")).unwrap();
    fs::write(
        home.join("engine/config.json"),
        format!(r#"{{"machineId": "{MACHINE}"}}"#),
    )
    .unwrap();
    let work = temp.dir("work");
    let work = fs::canonicalize(&work).unwrap();
    let enc: String = work
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // An earlier session left a real directory: the hook cannot link it, the tick imports it later
    let project = home.join(".claude/projects").join(&enc);
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{OLD}.jsonl")), "{\"uuid\":\"old\"}\n").unwrap();

    let transcript = project.join(format!("{NEW}.jsonl"));
    let event = |name: &str| {
        format!(
            r#"{{"session_id":"{NEW}","transcript_path":"{}","cwd":"{}","hook_event_name":"{name}"}}"#,
            transcript.display(),
            work.display()
        )
    };
    hook(&home, "session-start", &event("SessionStart"));
    assert!(
        live(&home).contains(NEW),
        "SessionStart puts the session on the live list: {}",
        live(&home)
    );
    assert!(
        !fs::symlink_metadata(&project).unwrap().is_symlink(),
        "a directory holding transcripts stays real: the tick imports it"
    );

    fs::write(&transcript, "{\"uuid\":\"new\"}\n").unwrap();
    hook(&home, "session-end", &event("SessionEnd"));
    assert!(
        !live(&home).contains(NEW),
        "an ended session left on the list counts as running for an hour and holds the import back: {}",
        live(&home)
    );
}

#[test]
fn a_session_that_ended_without_writing_is_no_longer_live() {
    let temp = TempDir::new("hook-session-end-empty");
    let home = temp.path().to_path_buf();
    fs::create_dir_all(home.join("engine/store")).unwrap();
    fs::write(
        home.join("engine/config.json"),
        format!(r#"{{"machineId": "{MACHINE}"}}"#),
    )
    .unwrap();
    let work = fs::canonicalize(temp.dir("work")).unwrap();
    let transcript = home
        .join(".claude/projects/-never-written")
        .join(format!("{NEW}.jsonl"));
    let event = |name: &str| {
        format!(
            r#"{{"session_id":"{NEW}","transcript_path":"{}","cwd":"{}","hook_event_name":"{name}"}}"#,
            transcript.display(),
            work.display()
        )
    };
    hook(&home, "session-start", &event("SessionStart"));
    assert!(live(&home).contains(NEW), "{}", live(&home));
    hook(&home, "session-end", &event("SessionEnd"));
    assert!(!live(&home).contains(NEW), "{}", live(&home));
}
