//! The local server started in a team's project writes into the team's clone as the member
//! `connect` recorded; anywhere else it keeps to the personal store — and in a project of a team
//! that is not connected it does not start at all.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::fs;
use std::path::PathBuf;

use vibememory_core::team_store::StoreRecord;
use vibememory_mcp::memories::for_directory;

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_teams_project_writes_into_the_teams_clone_as_the_member() {
    let root = std::env::temp_dir().join(format!("vibememory-mcp-team-{}", std::process::id()));
    let temp = Temp(
        fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(root.file_name().unwrap()),
    );
    let base = &temp.0;
    let engine = base.join("engine");
    fs::create_dir_all(engine.join("store")).unwrap();
    let file: serde_json::Value = serde_json::from_str(RECORDS).unwrap();
    let written = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "written")
        .unwrap();
    let record = StoreRecord::parse(written["text"].as_str().unwrap()).unwrap();
    let state = engine.join("stores").join(&record.team);
    fs::create_dir_all(state.join("store")).unwrap();
    fs::write(state.join("store.json"), record.to_json()).unwrap();
    fs::write(
        engine.join("config.json"),
        serde_json::json!({
            "machineId": "mac-test",
            "stores": {
                record.team.clone(): { "cwd": [format!("{}/work/**", base.display())] },
                "absent": { "cwd": [format!("{}/other/**", base.display())] },
            }
        })
        .to_string(),
    )
    .unwrap();
    for dir in ["work/app", "diary", "other/x"] {
        fs::create_dir_all(base.join(dir)).unwrap();
    }

    let (team, member) = for_directory(&engine, &base.join("work/app")).unwrap();
    assert_eq!(team.store(), state.join("store"));
    assert_eq!(member.as_deref(), Some(record.member.as_str()));

    let (own, member) = for_directory(&engine, &base.join("diary")).unwrap();
    assert_eq!(own.store(), engine.join("store"));
    assert_eq!(member, None);

    assert!(for_directory(&engine, &base.join("other/x")).is_err());
}
