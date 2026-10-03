//! Hand-offs read from the store's working copy, and the project the answer is about.

// The test writes a store on disk, so the purity gate is lifted here.
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

use serde_json::{Value, json};
use vibememory_mcp::memories::StoreMemories;
use vibememory_mcp::tools::{self, Caller};

/// A hand-off whose fields sit under `metadata`, as the engine's own notes write them.
const WINDOWS: &str = "---\nname: session-windows\ndescription: \"the windows build\"\nmetadata:\n  \
                       type: project\n  status: open\n  updated: 2026-10-01\n---\n\nwork\n";

/// A hand-off of the plainest shape: the marker alone.
const ENGINE: &str = "---\nstatus: open\n---\n\nwork\n";

/// A directory of this test, removed at the end.
struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vibememory-handoffs-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp");
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A store whose project holds `files` under `memory/sessions/`.
fn store_with(temp: &Temp, project: &str, files: &[(&str, &str)]) -> StoreMemories {
    let sessions = temp
        .0
        .join("projects")
        .join(project)
        .join("memory/sessions");
    fs::create_dir_all(&sessions).expect("sessions");
    for (name, text) in files {
        fs::write(sessions.join(name), text).expect("hand-off");
    }
    StoreMemories::new(temp.0.clone(), "mac-test".to_owned())
}

fn call(store: &StoreMemories, caller: &Caller<'_>, arguments: &Value) -> Value {
    tools::call("handoff_list", arguments, caller, store).expect("hand-offs")
}

#[test]
fn only_open_hand_offs_are_read_and_they_come_with_their_body() {
    let temp = Temp::new("open");
    // A server started in the project's folder, as a client in it starts one.
    let caller = Caller::owner("handoff-test", Some("Project"));
    let store = store_with(
        &temp,
        "Project",
        &[
            // Created in reverse alphabetical order, so the sorted answer is not the arrival order.
            ("windows.md", WINDOWS),
            ("engine.md", ENGINE),
            ("shipped.md", "---\nstatus: done\n---\n\ndone\n"),
            // A body line identical to the marker must not count: only the frontmatter decides.
            (
                "prose.md",
                "---\nstatus: done\n---\n\nthe frontmatter I have to write next time:\n\n\
                 status: open\n",
            ),
            // Not a hand-off at all: another extension, and no frontmatter to read.
            ("notes.txt", "---\nstatus: open\n---\n"),
            ("README.md", "# nothing here\n"),
        ],
    );

    let answer = call(&store, &caller, &json!({}));
    assert_eq!(answer["project"], "Project");
    let found = answer["handoffs"].as_array().expect("an array");
    let names: Vec<&str> = found
        .iter()
        .map(|one| one["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, vec!["engine", "windows"], "the open ones, by name");
    assert_eq!(
        found[0],
        json!({
            "project": "Project",
            "name": "engine",
            "description": null,
            "status": "open",
            "updated": null,
            "size": ENGINE.len(),
            "body": "work",
        }),
        "a note without fields of its own still comes with its text"
    );
    assert_eq!(
        found[1],
        json!({
            "project": "Project",
            "name": "windows",
            "description": "the windows build",
            "status": "open",
            "updated": "2026-10-01",
            "size": WINDOWS.len(),
            "body": "work",
        }),
        "the fields an agent wrote come back as they were written"
    );
}

#[test]
fn a_project_without_hand_offs_answers_none() {
    let temp = Temp::new("none");
    let caller = Caller::owner("handoff-test", Some("Project"));
    // The project exists — a session made it — and holds no hand-offs at all.
    fs::create_dir_all(temp.0.join("projects/Project")).expect("project");
    let store = StoreMemories::new(temp.0.clone(), "mac-test".to_owned());

    let answer = call(&store, &caller, &json!({}));
    assert_eq!(answer, json!({ "project": "Project", "handoffs": [] }));
}

#[test]
fn a_server_outside_any_project_reads_every_project_it_may_use() {
    let temp = Temp::new("every");
    let caller = Caller::owner("handoff-test", None);
    store_with(&temp, "Project", &[("engine.md", ENGINE)]);
    store_with(&temp, "Second", &[("windows.md", WINDOWS)]);
    let store = StoreMemories::new(temp.0.clone(), "mac-test".to_owned());

    let answer = call(&store, &caller, &json!({}));
    assert!(answer["project"].is_null(), "{answer}");
    let projects: Vec<&str> = answer["handoffs"]
        .as_array()
        .expect("an array")
        .iter()
        .map(|one| one["project"].as_str().expect("a project"))
        .collect();
    assert_eq!(projects, vec!["Project", "Second"]);

    // Naming one narrows the answer to it, and names it.
    let only = call(&store, &caller, &json!({ "project": "Second" }));
    assert_eq!(only["project"], "Second");
    assert_eq!(
        only["handoffs"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|one| one["name"].as_str().expect("a name"))
            .collect::<Vec<_>>(),
        vec!["windows"]
    );
}

#[test]
fn a_project_the_store_does_not_hold_is_refused() {
    let temp = Temp::new("missing");
    let caller = Caller::owner("handoff-test", Some("Project"));
    store_with(&temp, "Project", &[("engine.md", ENGINE)]);
    let store = StoreMemories::new(temp.0.clone(), "mac-test".to_owned());

    let refused = tools::call(
        "handoff_list",
        &json!({ "project": "Missing" }),
        &caller,
        &store,
    );
    let why = refused.expect_err("refused");
    assert!(why.contains("the store holds no project Missing"), "{why}");
}
