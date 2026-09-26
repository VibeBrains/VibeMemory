//! Which store the hooks put a session in: a team's clone for a directory routed to a connected
//! team, the personal store for the rest — and no store at all for a directory of a team that is
//! not connected here, so a team's work never lands in the personal store.

// The test writes files, so the purity gate is lifted here.
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

use support::TempDir;
use vibememory_cli::config::Config;
use vibememory_cli::install::Layout;
use vibememory_cli::stores::{for_cwd, for_file};
use vibememory_core::naming::PathSyntax;
use vibememory_core::team_store::StoreRecord;

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");

/// The record `connect` writes, from the fixture's `written` case, for team `syncteam`.
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

#[test]
fn a_session_goes_to_the_store_its_directory_is_routed_to() {
    let temp = TempDir::new("stores-routing");
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    };
    let base = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let config = Config::parse(
        &serde_json::json!({
            "machineId": "mac-main",
            "stores": {
                "syncteam": { "cwd": [format!("{base}/work/**")] },
                "absent": { "cwd": [format!("{base}/other/**")] },
            }
        })
        .to_string(),
        PathSyntax::Posix,
    )
    .unwrap();
    let state = layout.team_state_dir("syncteam");
    fs::create_dir_all(layout.team_store("syncteam")).unwrap();
    fs::write(state.join("store.json"), record().to_json()).unwrap();
    fs::create_dir_all(layout.store()).unwrap();

    let team = for_cwd(
        &layout,
        &config,
        &format!("{base}/work/app"),
        PathSyntax::Posix,
    )
    .unwrap();
    assert_eq!(team.team.as_deref(), Some("syncteam"));
    assert_eq!(team.machine_id, "alice-laptop");
    assert_eq!(team.clone, layout.team_store("syncteam"));

    let own = for_cwd(
        &layout,
        &config,
        &format!("{base}/diary"),
        PathSyntax::Posix,
    )
    .unwrap();
    assert_eq!(own.team, None);
    assert_eq!(own.machine_id, "mac-main");

    let refused = for_cwd(
        &layout,
        &config,
        &format!("{base}/other/x"),
        PathSyntax::Posix,
    );
    assert!(
        refused.is_err_and(|reason| reason.contains("absent") && reason.contains("not connected"))
    );

    let file = layout.team_store("syncteam").join("projects/app/s.jsonl");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "").unwrap();
    let (owner, relative) = for_file(&layout, &config, &fs::canonicalize(&file).unwrap()).unwrap();
    assert_eq!(owner.team.as_deref(), Some("syncteam"));
    assert_eq!(relative, std::path::PathBuf::from("projects/app/s.jsonl"));
}
