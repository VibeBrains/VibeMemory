//! The project a server takes from the directory it was started in.

// The test builds an engine directory on disk, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::fs;
use std::path::PathBuf;

use vibememory_mcp::memories::project_here;

/// A directory of this test run, removed at the end.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn only_a_project_the_store_already_holds_becomes_the_default() {
    let root = std::env::temp_dir().join(format!("vibememory-mcp-here-{}", std::process::id()));
    let temp = Temp(root.clone());
    let engine = root.join("engine");
    fs::create_dir_all(engine.join("store").join("projects").join("Known")).expect("store");
    fs::write(
        engine.join("config.json"),
        r#"{"machineId":"mac-test","remote":"ssh://git@host/store.git"}"#,
    )
    .expect("config");
    let known = root.join("work").join("Known");
    let stray = root.join("work").join("scratch");
    fs::create_dir_all(&known).expect("known");
    fs::create_dir_all(&stray).expect("stray");

    assert_eq!(
        project_here(&engine, &known).expect("resolve"),
        Some("Known".to_owned())
    );
    // A client may start its servers anywhere; a default that creates projects would scatter
    // memory into stores nobody meant to sync.
    assert_eq!(project_here(&engine, &stray).expect("resolve"), None);
    drop(temp);
}
