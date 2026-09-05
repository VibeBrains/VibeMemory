//! The outbox: prompt history and background tasks travelling between machines by copy.

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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use support::TempDir;
use vibememory_cli::outbox::{HISTORY_FILE, TASKS_DIR, import, publish};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::PathSyntax;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn roots(local_root: &Path) -> Roots {
    let mut entries = BTreeMap::new();
    entries.insert("PROJECTS".to_owned(), local_root.display().to_string());
    Roots::new(entries, PathSyntax::Posix)
}

fn history_line(project: &str, prompt: &str) -> String {
    format!("{{\"display\":\"{prompt}\",\"project\":\"{project}\",\"pastedContents\":{{}}}}")
}

fn read_history(dir: &Path) -> Vec<String> {
    fs::read_to_string(dir.join(HISTORY_FILE))
        .map(|text| text.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

#[test]
fn a_published_history_carries_paths_the_other_machine_can_read() {
    let temp = TempDir::new("outbox-publish");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let local = projects.join("VibeIDE");
    fs::write(
        config.join(HISTORY_FILE),
        format!("{}\n", history_line(&local.display().to_string(), "hello")),
    )
    .expect("write history");

    let moved = publish(&config, &store, "mac-test", &roots(&projects)).expect("publish");
    assert_eq!(moved.history_out, 1);

    let published = read_history(&store.join("machines").join("mac-test"));
    assert!(
        published[0].contains("{PROJECTS}/VibeIDE"),
        "a local path means nothing on the other machine: {}",
        published[0]
    );
    assert!(
        published[0].contains("hello"),
        "the prompt itself must survive: {}",
        published[0]
    );
}

#[test]
fn an_imported_history_comes_back_as_local_paths_and_keeps_what_was_here() {
    let temp = TempDir::new("outbox-import");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::write(
        config.join(HISTORY_FILE),
        format!(
            "{}\n",
            history_line(&projects.join("Mine").display().to_string(), "ours")
        ),
    )
    .expect("write local history");
    let theirs = store.join("machines").join("gpd-win");
    fs::create_dir_all(&theirs).expect("dirs");
    fs::write(
        theirs.join(HISTORY_FILE),
        format!("{}\n", history_line("{PROJECTS}/Theirs", "theirs")),
    )
    .expect("write their history");

    let moved = import(
        &config,
        &store,
        "mac-test",
        &roots(&projects),
        &BTreeSet::new(),
    )
    .expect("import");
    assert_eq!(moved.history_in, 1);

    let here = read_history(&config);
    assert_eq!(
        here.len(),
        2,
        "nothing that was here may be dropped: {here:?}"
    );
    assert!(here[0].contains("ours"));
    assert!(
        here[1].contains(&projects.join("Theirs").display().to_string()),
        "their line must name a path this machine has: {}",
        here[1]
    );
}

#[test]
fn importing_twice_adds_nothing_the_second_time() {
    let temp = TempDir::new("outbox-twice");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let theirs = store.join("machines").join("gpd-win");
    fs::create_dir_all(&theirs).expect("dirs");
    fs::write(
        theirs.join(HISTORY_FILE),
        format!("{}\n", history_line("{PROJECTS}/Theirs", "theirs")),
    )
    .expect("write");

    let first = import(
        &config,
        &store,
        "mac-test",
        &roots(&projects),
        &BTreeSet::new(),
    )
    .expect("first");
    let second = import(
        &config,
        &store,
        "mac-test",
        &roots(&projects),
        &BTreeSet::new(),
    )
    .expect("second");
    assert_eq!(first.history_in, 1);
    assert_eq!(
        second.history_in, 0,
        "the tick runs every two minutes; the history may not grow by a copy each time"
    );
    assert_eq!(read_history(&config).len(), 1);
}

#[test]
fn a_line_no_root_covers_travels_unchanged_rather_than_being_dropped() {
    let temp = TempDir::new("outbox-untranslatable");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    fs::write(
        config.join(HISTORY_FILE),
        format!("{}\n", history_line("/somewhere/else", "still a prompt")),
    )
    .expect("write");

    publish(&config, &store, "mac-test", &roots(&projects)).expect("publish");
    let published = read_history(&store.join("machines").join("mac-test"));
    assert!(
        published[0].contains("/somewhere/else"),
        "a path nobody can translate is still a prompt worth keeping: {}",
        published[0]
    );
}

#[test]
fn a_history_the_cli_is_writing_right_now_is_left_alone() {
    let temp = TempDir::new("outbox-locked");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let theirs = store.join("machines").join("gpd-win");
    fs::create_dir_all(&theirs).expect("dirs");
    fs::write(
        theirs.join(HISTORY_FILE),
        format!("{}\n", history_line("{PROJECTS}/Theirs", "theirs")),
    )
    .expect("write");
    // The CLI's own lock, fresh: it is writing a prompt this very moment.
    fs::create_dir(config.join("history.jsonl.lock")).expect("take the lock");

    let outcome = import(
        &config,
        &store,
        "mac-test",
        &roots(&projects),
        &BTreeSet::new(),
    );
    assert!(
        outcome.is_err(),
        "writing under the CLI's lock would lose the prompt it is writing"
    );
    assert!(
        read_history(&config).is_empty(),
        "and nothing may have been written"
    );
}

#[test]
fn tasks_travel_but_not_for_a_session_running_here() {
    let temp = TempDir::new("outbox-tasks");
    let config = temp.dir("claude");
    let store = temp.dir("store");
    let projects = temp.dir("Projects");
    let theirs = store.join("machines").join("gpd-win").join(TASKS_DIR);
    for session in [SESSION, "22222222-2222-4222-8222-222222222222"] {
        fs::create_dir_all(theirs.join(session)).expect("dirs");
        fs::write(theirs.join(session).join("1.json"), b"{}\n").expect("write");
    }

    // The first session is running here: its task files are being written right now, and their
    // copy is older by construction.
    let live: BTreeSet<String> = [SESSION.to_owned()].into_iter().collect();
    let moved = import(&config, &store, "mac-test", &roots(&projects), &live).expect("import");

    assert_eq!(moved.tasks_in, 1, "only the session that is not live here");
    assert!(!config.join(TASKS_DIR).join(SESSION).exists());
    assert!(
        config
            .join(TASKS_DIR)
            .join("22222222-2222-4222-8222-222222222222")
            .join("1.json")
            .exists()
    );
}
