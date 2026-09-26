//! `store reclone` and leaving a team with sessions: the clone made anew keeps what the team's
//! store may hold and leaves the rest in the refused clone; leaving brings this machine's moved
//! projects back, takes down the team's links and archives the clone without its key.

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

use support::{TempDir, git, git_repo_with_commit};
use vibememory_cli::config::Config;
use vibememory_cli::install::Layout;
use vibememory_cli::team_ops::{leave, reclone_from};
use vibememory_core::naming::{PathSyntax, encode_cwd};
use vibememory_core::team_store::StoreRecord;

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");
const STAMP: &str = "2026-09-05T10:00:00Z";
const SESSION: &str = "11111111-1111-4111-8111-111111111111";

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

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    }
}

/// A team's host stand-in with one commit, and this machine's clone of it with its record.
fn connected(temp: &TempDir, layout: &Layout) -> std::path::PathBuf {
    let record = record();
    let seed = temp.dir("seed");
    git_repo_with_commit(&seed);
    fs::write(seed.join(".gitattributes"), "* merge=union\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "--quiet", "-m", "host"]);
    let bare = temp.path().join("host.git");
    git(
        temp.path(),
        &[
            "clone",
            "--quiet",
            "--bare",
            &seed.display().to_string(),
            &bare.display().to_string(),
        ],
    );
    let state = layout.team_state_dir(&record.team);
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("store.json"), record.to_json()).unwrap();
    git(
        &state,
        &["clone", "--quiet", &bare.display().to_string(), "store"],
    );
    bare
}

#[test]
fn a_reclone_keeps_what_the_team_may_hold_and_sets_the_rest_aside() {
    let temp = TempDir::new("team-ops-reclone");
    let layout = layout(&temp);
    let bare = connected(&temp, &layout);
    let record = record();
    let clone = layout.team_store(&record.team);
    let own = format!("machines/{}/links.json", record.store_name);
    for (path, text) in [
        (format!("projects/app/{SESSION}.jsonl"), "{}\n"),
        (own.clone(), "{}"),
        ("history.jsonl".to_owned(), "{}\n"),
        ("config/settings.json".to_owned(), "{}"),
    ] {
        fs::create_dir_all(clone.join(&path).parent().unwrap()).unwrap();
        fs::write(clone.join(&path), text).unwrap();
    }
    vibememory_cli::local_only::keep(&clone, &[format!("projects/app/{SESSION}.jsonl")]).unwrap();
    let state = layout.team_state_dir(&record.team);
    vibememory_cli::guard::TickState {
        pause: Some(vibememory_cli::guard::StorePause {
            code: "pathDenied".to_owned(),
            lines: vec!["history.jsonl".to_owned()],
            since: STAMP.to_owned(),
            recheck_at: STAMP.to_owned(),
        }),
        ..vibememory_cli::guard::TickState::default()
    }
    .write(&state)
    .unwrap();

    let done = reclone_from(&layout, &record.team, STAMP, &bare.display().to_string()).unwrap();

    assert!(
        clone.join(".gitattributes").is_file(),
        "cloned anew from the host"
    );
    assert!(
        clone
            .join(format!("projects/app/{SESSION}.jsonl"))
            .is_file()
    );
    assert!(clone.join(&own).is_file());
    assert!(!clone.join("history.jsonl").exists());
    assert!(!clone.join("config/settings.json").exists());
    assert!(
        done.rejected.join("history.jsonl").is_file(),
        "the refused file stays aside"
    );
    assert!(vibememory_cli::local_only::is_local(
        &clone,
        &format!("projects/app/{SESSION}.jsonl")
    ));
    assert_eq!(vibememory_cli::guard::TickState::read(&state).pause, None);
}

#[test]
fn leaving_brings_moved_projects_back_and_archives_the_clone_without_its_key() {
    let temp = TempDir::new("team-ops-leave");
    let layout = layout(&temp);
    connected(&temp, &layout);
    let record = record();
    fs::create_dir_all(layout.store()).unwrap();
    git_repo_with_commit(&layout.store());
    let state = layout.team_state_dir(&record.team);
    fs::write(state.join("key"), "private").unwrap();
    fs::write(state.join("key.pub"), "public").unwrap();
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let config = Config::parse(
        &serde_json::json!({ "machineId": "mac-main", "stores": { "syncteam": { "cwd": [format!("{base}/work/**")] } } }).to_string(),
        PathSyntax::Posix,
    )
    .unwrap();
    // a project of the personal store, moved into the team
    let cwd = format!("{base}/work/app");
    fs::create_dir_all(&cwd).unwrap();
    let project = layout.store().join("projects/app");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join(format!("{SESSION}.jsonl")), "{}\n").unwrap();
    let link = layout
        .config_dir
        .join("projects")
        .join(encode_cwd(&cwd).unwrap().as_str());
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    vibememory_cli::dir_link::create(&project, &link).unwrap();
    vibememory_cli::project_move::to_team(&layout, &config, &cwd, &cwd, "syncteam", STAMP).unwrap();
    // and a project born in the team
    let born_cwd = format!("{base}/work/born");
    let born = layout.team_store("syncteam").join("projects/born");
    fs::create_dir_all(&born).unwrap();
    let born_link = layout
        .config_dir
        .join("projects")
        .join(encode_cwd(&born_cwd).unwrap().as_str());
    vibememory_cli::dir_link::create(&born, &born_link).unwrap();

    let left = leave(&layout, &config, "syncteam", STAMP).unwrap();

    assert_eq!(left.brought_back, ["app"]);
    assert_eq!(left.unlinked, ["born"]);
    assert_eq!(
        fs::canonicalize(&link).unwrap(),
        fs::canonicalize(&project).unwrap(),
        "the moved project is the personal store's again"
    );
    assert!(
        fs::symlink_metadata(&born_link).is_err(),
        "the team's own project is unlinked"
    );
    assert!(!state.exists());
    assert!(
        left.archive.join("store/projects/born").is_dir(),
        "the team's files stay archived"
    );
    assert!(!left.archive.join("key").exists() && !left.archive.join("key.pub").exists());
    assert_eq!(left.key_id, record.key_id);
}
