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

use vibememory_mcp::memories::{
    DirectoryProject, Memories, StoreMemories, directory_project, project_here,
};

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
        project_here(&engine, &engine.join("store"), &known).expect("resolve"),
        Some("Known".to_owned())
    );
    // A client may start its servers anywhere; a default that creates projects would scatter
    // memory into stores nobody meant to sync.
    assert_eq!(
        project_here(&engine, &engine.join("store"), &stray).expect("resolve"),
        None
    );

    // The tool says why: the rules name it, the store does not hold it.
    assert_eq!(
        directory_project(&engine, &engine.join("store"), &stray).expect("resolve"),
        DirectoryProject::Unheld {
            name: "scratch".to_owned()
        }
    );
    // Through the store, as a shared server answers an agent that names its own folder.
    let store = StoreMemories::new(engine.join("store"), "mac-test".to_owned());
    assert!(matches!(
        store.project_of_directory(&known.to_string_lossy()),
        Ok(DirectoryProject::Held { ref name, .. }) if name == "Known"
    ));
    assert!(store.project_of_directory("relative/dir").is_err());
    drop(temp);
}

/// A folder reached through a symlink is the folder it points at.
///
/// The owner's projects live on another disk and are opened through a link in the home directory.
/// Git answers about the real path while our own walk for `.git` follows the path as written, so
/// every `project_resolve` from the IDE came back «repository layout mismatch» — and the agent, told
/// that the project could not be named, asked the owner to type the path by hand (18.09.2026).
#[test]
#[cfg(unix)]
fn a_directory_reached_through_a_symlink_names_the_project_it_points_at() {
    let root = std::env::temp_dir().join(format!("vibememory-mcp-link-{}", std::process::id()));
    let temp = Temp(root.clone());
    let engine = root.join("engine");
    fs::create_dir_all(engine.join("store").join("projects").join("Known")).expect("store");
    fs::write(
        engine.join("config.json"),
        r#"{"machineId":"mac-test","remote":"ssh://git@host/store.git"}"#,
    )
    .expect("config");
    let known = root.join("work").join("Known");
    fs::create_dir_all(&known).expect("known");
    let link = root.join("link-to-known");
    std::os::unix::fs::symlink(&known, &link).expect("symlink");

    let store = StoreMemories::new(engine.join("store"), "mac-test".to_owned());
    assert!(
        matches!(
            store.project_of_directory(&link.to_string_lossy()),
            Ok(DirectoryProject::Held { ref name, .. }) if name == "Known"
        ),
        "the link must resolve to the same project as the folder it points at"
    );
    drop(temp);
}
