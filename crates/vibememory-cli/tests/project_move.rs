//! `project move`: a project of the personal store into a team's and back. Into the team its
//! memory is shared and its sessions stay this machine's own; back, only this member's sessions and
//! memory versions return — a teammate's stay in the team.

// The test runs git and writes files, so the purity gate is lifted here.
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
use std::path::PathBuf;

use support::{TempDir, git_repo_with_commit};
use vibememory_cli::config::Config;
use vibememory_cli::install::Layout;
use vibememory_cli::project_move::{MOVED_FILE, to_personal, to_team};
use vibememory_core::naming::{PathSyntax, encode_cwd};
use vibememory_core::team_store::StoreRecord;

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");
const STAMP: &str = "2026-09-05T10:00:00Z";
const OWN: &str = "11111111-1111-4111-8111-111111111111";
const LATER: &str = "33333333-3333-4333-8333-333333333333";
const THEIRS: &str = "22222222-2222-4222-8222-222222222222";

/// The record of team `syncteam`, member `alice`, machine `alice-laptop`.
fn record() -> StoreRecord {
    let file: serde_json::Value = serde_json::from_str(RECORDS).unwrap();
    let written = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "written")
        .unwrap();
    StoreRecord::parse(written["text"].as_str().unwrap()).unwrap()
}

struct Setup {
    layout: Layout,
    cwd: String,
    base: String,
}

fn config(setup: &Setup, routed: bool) -> Config {
    let stores = if routed {
        serde_json::json!({ "syncteam": { "cwd": [format!("{}/work/**", setup.base)] } })
    } else {
        serde_json::json!({ "syncteam": {} })
    };
    Config::parse(
        &serde_json::json!({ "machineId": "mac-main", "stores": stores }).to_string(),
        PathSyntax::Posix,
    )
    .unwrap()
}

fn journal_line(member: &str, id: &str) -> String {
    serde_json::json!({ "id": id, "member": member }).to_string()
}

fn setup(temp: &TempDir) -> Setup {
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    };
    fs::create_dir_all(layout.store()).unwrap();
    git_repo_with_commit(&layout.store());
    let record = record();
    let clone = layout.team_store(&record.team);
    fs::create_dir_all(&clone).unwrap();
    git_repo_with_commit(&clone);
    fs::write(
        layout.team_state_dir(&record.team).join("store.json"),
        record.to_json(),
    )
    .unwrap();
    let cwd = format!("{base}/work/app");
    fs::create_dir_all(&cwd).unwrap();
    let project = layout.store().join("projects/app");
    fs::create_dir_all(project.join("memory")).unwrap();
    fs::write(project.join(format!("{OWN}.jsonl")), "{}\n").unwrap();
    fs::write(
        project.join("memory.jsonl"),
        format!("{}\n", journal_line("alice", "m1")),
    )
    .unwrap();
    fs::write(project.join("memory/MEMORY.md"), "index\n").unwrap();
    let link = layout
        .config_dir
        .join("projects")
        .join(encode_cwd(&cwd).unwrap().as_str());
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    vibememory_cli::dir_link::create(&project, &link).unwrap();
    Setup { layout, cwd, base }
}

fn link_target(setup: &Setup) -> PathBuf {
    fs::canonicalize(
        setup
            .layout
            .config_dir
            .join("projects")
            .join(encode_cwd(&setup.cwd).unwrap().as_str()),
    )
    .unwrap()
}

#[test]
fn into_the_team_memory_is_shared_and_sessions_stay_local() {
    let temp = TempDir::new("project-move-in");
    let setup = setup(&temp);
    let refused = to_team(
        &setup.layout,
        &config(&setup, false),
        &setup.cwd,
        &setup.cwd,
        "syncteam",
        STAMP,
    );
    assert!(refused.is_err_and(|reason| reason.contains("vibememory route add")));

    let report = to_team(
        &setup.layout,
        &config(&setup, true),
        &setup.cwd,
        &setup.cwd,
        "syncteam",
        STAMP,
    )
    .unwrap();
    assert_eq!(report.name, "app");
    let clone = fs::canonicalize(setup.layout.team_store("syncteam")).unwrap();
    assert_eq!(link_target(&setup), clone.join("projects/app"));
    assert!(clone.join("projects/app/memory.jsonl").is_file());
    assert!(vibememory_cli::local_only::is_local(
        &clone,
        &format!("projects/app/{OWN}.jsonl")
    ));
    assert!(!vibememory_cli::local_only::is_local(
        &clone,
        "projects/app/memory.jsonl"
    ));
    assert!(
        setup
            .layout
            .store()
            .join("projects/app")
            .join(MOVED_FILE)
            .is_file(),
        "the archive is marked"
    );
}

#[test]
fn back_comes_only_what_is_this_members() {
    let temp = TempDir::new("project-move-back");
    let setup = setup(&temp);
    to_team(
        &setup.layout,
        &config(&setup, true),
        &setup.cwd,
        &setup.cwd,
        "syncteam",
        STAMP,
    )
    .unwrap();
    let clone = setup.layout.team_store("syncteam");
    let project = clone.join("projects/app");
    // later in the team: a session of this member, one of a teammate, and memory by both
    fs::write(project.join(format!("{LATER}.jsonl")), "{}\n").unwrap();
    fs::write(project.join(format!("{THEIRS}.jsonl")), "{}\n").unwrap();
    let journal = format!(
        "{}\n{}\n{}\n",
        journal_line("alice", "m1"),
        journal_line("alice", "m2"),
        journal_line("bob", "b1")
    );
    fs::write(project.join("memory.jsonl"), journal).unwrap();
    for (machine, session) in [("alice-laptop", LATER), ("bob-pc", THEIRS)] {
        let dir = clone.join("machines").join(machine);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("tails.json"),
            serde_json::json!({ "sessions": { session: { "lastUuid": null, "lines": 1, "resumeLeaf": null, "at": STAMP } } })
                .to_string(),
        )
        .unwrap();
    }
    // still routed to the team: refused
    assert!(to_personal(&setup.layout, &config(&setup, true), &setup.cwd, &setup.cwd).is_err());

    let report = to_personal(
        &setup.layout,
        &config(&setup, false),
        &setup.cwd,
        &setup.cwd,
    )
    .unwrap();
    let archive = setup.layout.store().join("projects/app");
    assert_eq!(link_target(&setup), fs::canonicalize(&archive).unwrap());
    assert!(
        archive.join(format!("{LATER}.jsonl")).is_file(),
        "this member's later session came back"
    );
    assert!(
        !archive.join(format!("{THEIRS}.jsonl")).exists(),
        "the teammate's stayed in the team"
    );
    let personal_journal = fs::read_to_string(archive.join("memory.jsonl")).unwrap();
    assert!(personal_journal.contains("\"m2\""));
    assert!(!personal_journal.contains("\"b1\""));
    assert_eq!(
        personal_journal.matches("\"m1\"").count(),
        1,
        "nothing twice"
    );
    assert_eq!(report.memory_versions, 1);
    assert!(!archive.join(MOVED_FILE).exists());
}

#[test]
fn a_live_session_stops_the_move() {
    let temp = TempDir::new("project-move-live");
    let setup = setup(&temp);
    let machine = setup.layout.store().join("machines/mac-other");
    fs::create_dir_all(&machine).unwrap();
    fs::write(
        machine.join("live.json"),
        serde_json::json!({ "sessions": { OWN: { "at": STAMP, "cwd": setup.cwd } } }).to_string(),
    )
    .unwrap();
    let refused = to_team(
        &setup.layout,
        &config(&setup, true),
        &setup.cwd,
        &setup.cwd,
        "syncteam",
        STAMP,
    );
    assert!(refused.is_err_and(|reason| reason.contains("live")));
    assert_eq!(
        link_target(&setup),
        fs::canonicalize(setup.layout.store().join("projects/app")).unwrap(),
        "the link did not move"
    );
}
