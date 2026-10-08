//! `vibememory rule …` from a terminal, against a store laid out as an installed machine has it: a rule added, its
//! history read from the store's git, and the rule moved to another level.

// The test writes files and runs the engine's binary and git, so the purity gate is lifted here.
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
use std::path::{Path, PathBuf};
use std::process::Command;

use support::{TempDir, git, git_repo_with_commit};

struct Machine {
    temp: TempDir,
    store: PathBuf,
    work: PathBuf,
}

/// A machine with its engine configured and a project `app` in the personal store. The engine directory is not
/// `$HOME/.vibememory`, so nothing a test runs reaches the system's scheduler.
fn machine(label: &str) -> Machine {
    let temp = TempDir::new(label);
    let engine = temp.dir("engine");
    fs::write(engine.join("config.json"), r#"{"machineId": "mac-test"}"#).unwrap();
    let store = temp.dir("engine/store");
    git_repo_with_commit(&store);
    fs::create_dir_all(store.join("projects/app")).unwrap();
    let work = fs::canonicalize(temp.dir("work/app")).unwrap();
    Machine { temp, store, work }
}

fn engine(m: &Machine, args: &[&str], stdin: &str) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_vibememory"))
        .args(args)
        .env("HOME", m.temp.dir("home"))
        .env("VIBEMEMORY_DIR", m.temp.path().join("engine"))
        .env("CLAUDE_CONFIG_DIR", m.temp.dir("home/.claude"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the engine binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("stdin is read");
    let out = child.wait_with_output().expect("the engine answers");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

fn dir(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn a_rule_is_added_its_history_read_and_it_moves_to_the_project() {
    let m = machine("rule-command");
    let (ok, said) = engine(
        &m,
        &[
            "rule",
            "add",
            "--level",
            "personal",
            "--id",
            "tests",
            "--title",
            "Tests",
            dir(&m.work),
        ],
        "Run cargo test.\n",
    );
    assert!(ok, "{said}");
    let personal = m.store.join("config/rules/tests.md");
    assert!(personal.is_file());

    let (ok, said) = engine(&m, &["rule", "history", "tests", dir(&m.work)], "");
    assert!(ok && said.contains("not committed yet"), "{said}");
    git(&m.store, &["add", "."]);
    git(&m.store, &["commit", "--quiet", "-m", "rules"]);
    let (ok, said) = engine(&m, &["rule", "history", "tests", dir(&m.work)], "");
    assert!(
        ok && said.contains(" rules\n") && said.contains("now: Tests"),
        "{said}"
    );

    let (ok, said) = engine(
        &m,
        &[
            "rule",
            "move",
            "tests",
            "--level",
            "personal",
            "--to",
            "project",
            dir(&m.work),
        ],
        "",
    );
    assert!(ok, "{said}");
    assert!(!personal.exists(), "the rule left its old level");
    let moved = fs::read_to_string(m.store.join("projects/app/rules/tests.md")).unwrap();
    assert!(
        moved.contains("level: project") && moved.contains("Run cargo test."),
        "{moved}"
    );
}

#[test]
fn a_flag_the_new_level_has_not_is_dropped_and_said() {
    let m = machine("rule-command-flags");
    let (ok, said) = engine(
        &m,
        &[
            "rule",
            "add",
            "--level",
            "personal",
            "--id",
            "secrets",
            "--title",
            "Secrets",
            "--absolute",
            dir(&m.work),
        ],
        "No secrets in the store.\n",
    );
    assert!(ok, "{said}");
    let (ok, said) = engine(
        &m,
        &[
            "rule",
            "move",
            "secrets",
            "--level",
            "personal",
            "--to",
            "project",
            dir(&m.work),
        ],
        "",
    );
    assert!(ok && said.contains("absolute dropped"), "{said}");
    let (ok, said) = engine(
        &m,
        &[
            "rule",
            "move",
            "secrets",
            "--level",
            "project",
            "--to",
            "project",
            dir(&m.work),
        ],
        "",
    );
    assert!(
        !ok && said.contains("already at the project level"),
        "{said}"
    );
    let moved = fs::read_to_string(m.store.join("projects/app/rules/secrets.md")).unwrap();
    assert!(!moved.contains("absolute"), "{moved}");
}

#[test]
fn a_teams_rule_from_a_terminal_is_a_proposal_signed_by_the_member() {
    let m = machine("rule-command-team");
    let work = dir(&m.work).to_owned();
    fs::write(
        m.temp.path().join("engine/config.json"),
        format!(r#"{{"machineId": "mac-test", "stores": {{"syncteam": {{"cwd": ["{work}"]}}}}}}"#),
    )
    .unwrap();
    let state = m.temp.dir("engine/stores/syncteam");
    let records: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/claim/storeRecords.json")).unwrap();
    let record = records["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "written")
        .unwrap()["text"]
        .as_str()
        .unwrap();
    fs::write(state.join("store.json"), record).unwrap();
    let team = m.temp.dir("engine/stores/syncteam/store");
    git_repo_with_commit(&team);
    fs::create_dir_all(team.join("projects/app")).unwrap();

    let (ok, said) = engine(
        &m,
        &[
            "rule", "add", "--level", "team", "--id", "review", "--title", "Review", &work,
        ],
        "Every change goes through review.\n",
    );
    assert!(ok && said.contains("proposed to team syncteam"), "{said}");
    assert!(
        !team.join("rules/review.md").exists(),
        "a member does not write the team's rules"
    );
    let proposal = fs::read_to_string(team.join("proposals/rules/review.alice.md")).unwrap();
    assert!(
        proposal.contains("Every change goes through review."),
        "{proposal}"
    );

    // moved to the team, a rule is a proposal there and stays in force where it was until it is accepted
    let (ok, said) = engine(
        &m,
        &[
            "rule", "add", "--level", "personal", "--id", "style", "--title", "Style", &work,
        ],
        "One thought a line.\n",
    );
    assert!(ok, "{said}");
    let (ok, said) = engine(
        &m,
        &[
            "rule", "move", "style", "--level", "personal", "--to", "team", &work,
        ],
        "",
    );
    assert!(ok && said.contains("stays until then"), "{said}");
    assert!(team.join("proposals/rules/style.alice.md").is_file());
    assert!(m.store.join("config/rules/style.md").is_file());
}
