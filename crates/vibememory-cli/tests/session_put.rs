//! `session put`: another agent's session lands in the store the way a Claude Code session does
//! after `hook stop` — against a real git repository in a temporary directory.

// The test drives real git, so the purity gate is lifted here.
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
use std::path::{Path, PathBuf};

use support::{TempDir, git_repo_with_commit};
use vibememory_cli::config::Config;
use vibememory_cli::foreign_session::{Placed, Request, delivered, put};
use vibememory_cli::hook::stop::Live;
use vibememory_cli::install::Layout;
use vibememory_core::naming::PathSyntax;

const SESSIONS: &str = include_str!("../../../fixtures/foreign/foreignSessions.json");
const STAMP: &str = "2026-09-30T09:00:00Z";
const AGENT: &str = "dsh-desktop";
const SESSION: &str = "session-22222222-2222-4222-8222-000000000001";

/// The text of one fixture case.
fn case(id: &str) -> String {
    let file: serde_json::Value = serde_json::from_str(SESSIONS).unwrap();
    file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

struct Machine {
    temp: TempDir,
    layout: Layout,
    config: Config,
    /// A project directory outside git: its name is the directory's.
    work: String,
}

fn machine(label: &str) -> Machine {
    let temp = TempDir::new(label);
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    };
    fs::create_dir_all(layout.store()).unwrap();
    git_repo_with_commit(&layout.store());
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let work = format!("{base}/work/Promed");
    fs::create_dir_all(&work).unwrap();
    let config = Config::parse(
        &serde_json::json!({
            "machineId": "mac-main",
            "ignoreCwd": [format!("{base}/scratch")],
        })
        .to_string(),
        PathSyntax::Posix,
    )
    .unwrap();
    Machine {
        temp,
        layout,
        config,
        work,
    }
}

impl Machine {
    fn file(&self, text: &str) -> PathBuf {
        let path = self.temp.path().join("converted.jsonl");
        fs::write(&path, text).unwrap();
        path
    }

    fn put(&self, cwd: &str, agent: &str, source: &Path, ended: bool) -> Result<Placed, String> {
        let bytes = fs::read(source).unwrap();
        put(
            &self.layout,
            &self.config,
            &Request {
                agent,
                session: SESSION,
                cwd,
                portable_cwd: cwd,
                origin: &source.display().to_string(),
                bytes: &bytes,
                ended,
                stamp: STAMP,
            },
        )
    }

    fn committed(&self, relative: &str) -> String {
        let output = std::process::Command::new("git")
            .args(["show", &format!("HEAD:{relative}")])
            .current_dir(self.layout.store())
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn live(&self) -> Live {
        let path = self.layout.store().join("machines/mac-main/live.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap_or_default()).unwrap_or_default()
    }
}

#[test]
fn a_session_lands_in_its_projects_agents_directory_and_is_committed() {
    let machine = machine("put-lands");
    let text = case("dshConvertedSession");
    let source = machine.file(&text);

    let placed = machine.put(&machine.work, AGENT, &source, false).unwrap();
    assert_eq!(placed.project, "Promed");
    assert_eq!(
        placed.relative,
        format!("projects/Promed/agents/{AGENT}/{SESSION}.jsonl")
    );
    assert!(placed.committed);
    assert!(placed.held.is_empty());
    assert_eq!((placed.checked.lines, placed.checked.spoken), (3, 2));
    assert_eq!(machine.committed(&placed.relative), text);
    assert!(
        machine.live().sessions.contains_key(SESSION),
        "the tick must not commit it under the agent while it is being handed over"
    );
    assert_eq!(
        delivered(&machine.layout.store())[AGENT].sessions,
        1,
        "status counts it"
    );

    let again = machine.put(&machine.work, AGENT, &source, true).unwrap();
    assert!(!again.committed, "the same file makes no second commit");
    assert!(
        !machine.live().sessions.contains_key(SESSION),
        "--end takes the session off the live list"
    );
}

#[test]
fn a_file_that_breaks_the_format_is_refused_whole_and_names_the_line() {
    let machine = machine("put-refused");
    let source = machine.file(&case("noUuid"));
    let refused = machine
        .put(&machine.work, AGENT, &source, false)
        .unwrap_err();
    assert!(refused.contains("line 1: uuid"), "{refused}");
    assert!(
        !machine.layout.store().join("projects").exists(),
        "nothing of a refused file reaches the store"
    );
}

#[test]
fn a_name_that_could_be_a_path_is_refused_before_anything_is_read() {
    let machine = machine("put-names");
    let source = machine.file(&case("dshConvertedSession"));
    for agent in ["DSH", "../claude", "-agent", ""] {
        let refused = machine
            .put(&machine.work, agent, &source, false)
            .unwrap_err();
        assert!(refused.starts_with("agent "), "{agent}: {refused}");
    }
}

#[test]
fn an_ignored_directory_keeps_its_sessions_and_says_so() {
    let machine = machine("put-ignored");
    let source = machine.file(&case("dshConvertedSession"));
    let scratch = machine.work.replace("work/Promed", "scratch");
    fs::create_dir_all(&scratch).unwrap();
    let refused = machine.put(&scratch, AGENT, &source, false).unwrap_err();
    assert!(refused.contains("ignoreCwd"), "{refused}");
}

#[test]
fn the_command_names_what_it_needs() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
        .args(["session", "put", "--agent", AGENT])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--from <file.jsonl>"));
}
