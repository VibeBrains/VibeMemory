//! `install` for a machine in a team with sessions on: the team's clone gets the merge drivers the
//! personal store has, and this machine's directory in it — applied by `connect` right after the
//! clone, and planned again by every `install`.

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
use std::process::Command;

use support::{TempDir, git_repo_with_commit};
use vibememory_cli::install::{Layout, State, Step, apply, plan_team};
use vibememory_core::team_store::StoreRecord;

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");

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

fn setting(clone: &std::path::Path, key: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(clone)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[test]
fn a_teams_clone_gets_the_merge_drivers_and_the_machines_directory() {
    let temp = TempDir::new("install-teams");
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    };
    let record = record();
    let clone = layout.team_store(&record.team);
    fs::create_dir_all(&clone).unwrap();
    git_repo_with_commit(&clone);
    // a clone `connect` makes has no identity of its own: the harness's is only for its commit
    support::git(&clone, &["config", "--unset", "user.name"]);
    support::git(&clone, &["config", "--unset", "user.email"]);
    // as `connect` clones it: with the store's settings from the first moment
    for (key, value, _why) in vibememory_cli::install::GIT_SETTINGS {
        support::git(&clone, &["config", key, value]);
    }
    fs::write(
        layout.team_state_dir(&record.team).join("store.json"),
        record.to_json(),
    )
    .unwrap();

    let before = plan_team(&layout, &record.team);
    let driver = before
        .iter()
        .find(|action| {
            matches!(&action.step, Step::GitSetting { team: Some(_), key, .. } if *key == "merge.vibememory-jsonl.driver")
        })
        .expect("the jsonl driver is planned for the team's clone");
    assert_eq!(driver.state, State::Missing);

    let applied = apply(&layout, &before, false);
    assert!(applied.failed.is_empty(), "{:?}", applied.failed);
    assert!(setting(&clone, "merge.vibememory-jsonl.driver").is_some());
    // every commit of the engine into the team's store is signed by this machine's name in it
    assert_eq!(
        setting(&clone, "user.name").as_deref(),
        Some(record.store_name.as_str())
    );
    assert_eq!(
        setting(&clone, "user.email").as_deref(),
        Some("vibememory@vibememory.invalid")
    );
    assert!(
        clone
            .join("machines")
            .join(&record.store_name)
            .join(".keep")
            .is_file()
    );
    // the personal store is not touched by the team's plan
    assert!(!layout.store().exists());

    // every `install` plans the team's clone too
    let config = vibememory_cli::config::Config::parse(
        r#"{"machineId":"mac-main"}"#,
        vibememory_core::naming::PathSyntax::Posix,
    )
    .unwrap();
    assert!(
        vibememory_cli::install::plan(&layout, &config, &[])
            .iter()
            .any(|action| matches!(&action.step, Step::StoreDir { team: Some(team), .. } if *team == record.team)),
        "the full plan holds the team's steps"
    );

    let after = plan_team(&layout, &record.team);
    assert!(
        after.iter().all(|action| action.state == State::Satisfied),
        "{after:?}"
    );
}
