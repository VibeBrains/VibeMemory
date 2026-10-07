//! The merge drivers driven by git itself: two branches that both appended to one transcript, and
//! one that both edited a memory document.
//!
//! Every repository here is configured by what `install` writes and every merge is run the way the
//! tick runs it — launchd's `PATH`, a home of its own. Anything kinder would test a driver nobody
//! runs.

// The test drives real git, so the purity gate is lifted here.
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
use std::path::Path;

use support::{TempDir, git, git_as_the_tick, git_repo_with_commit};
use vibememory_cli::install::{GITATTRIBUTES, Layout, git_settings, installed_binary};

/// Configures the store exactly as `install` does for an engine in `engine`: the same git settings,
/// the same `.gitattributes`, the driver lines `install` writes — and the binary at the path those
/// lines name. `engine` is where a quarantined version will land.
///
/// The drivers used to be registered here with the test binary's own absolute path. That is why
/// every test in this file stayed green while the line `install` really wrote — the bare name
/// `vibememory` — could not be found by git at all.
fn register_drivers_with_engine(store: &Path, engine: &Path) {
    let layout = Layout {
        config_dir: engine.join("claude"),
        engine_dir: engine.to_path_buf(),
        home: None,
    };
    let binary = installed_binary(&layout);
    fs::create_dir_all(binary.parent().expect("bin directory")).expect("create bin");
    fs::copy(env!("CARGO_BIN_EXE_vibememory"), &binary).expect("place the engine binary");
    for (key, value) in git_settings(&layout) {
        git(store, &["config", "--local", key, &value]);
    }
    fs::write(store.join(".gitattributes"), GITATTRIBUTES).expect("write attributes");
    git(store, &["add", ".gitattributes"]);
    git(store, &["commit", "--quiet", "-m", "attributes"]);
}

/// Merges `branch` the way the tick does and fails the test with what git and the driver said
/// when that does not work.
fn merge(store: &Path, branch: &str) {
    let home = store.join("..").join("home");
    fs::create_dir_all(&home).expect("home");
    let output = git_as_the_tick(store, &home, &["merge", "--quiet", "--no-edit", branch]);
    assert!(
        output.status.success(),
        "the merge the tick would run failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The common case: the quarantine directory is of no interest to the test.
fn register_drivers(store: &Path) {
    let engine = store.join("..").join("engine-default");
    register_drivers_with_engine(store, &engine);
}

fn record(uuid: &str, at: &str) -> String {
    format!("{{\"type\":\"user\",\"uuid\":\"{uuid}\",\"timestamp\":\"{at}\"}}\n")
}

fn commit_file(store: &Path, relative: &str, contents: &str, message: &str) {
    let path = store.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
    fs::write(&path, contents).expect("write");
    git(store, &["add", relative]);
    git(store, &["commit", "--quiet", "-m", message]);
}

fn read(store: &Path, relative: &str) -> String {
    fs::read_to_string(store.join(relative)).expect("read merged file")
}

#[test]
fn two_machines_that_both_appended_keep_every_record() {
    let temp = TempDir::new("driver-jsonl");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let relative = "projects/Project/session.jsonl";
    commit_file(
        &store,
        relative,
        &record("shared", "2026-09-05T08:00:00Z"),
        "base",
    );
    register_drivers(&store);

    git(&store, &["checkout", "--quiet", "-b", "other"]);
    let theirs = format!(
        "{}{}",
        record("shared", "2026-09-05T08:00:00Z"),
        record("theirs", "2026-09-05T08:00:02Z")
    );
    commit_file(&store, relative, &theirs, "their turn");

    git(&store, &["checkout", "--quiet", "-"]);
    let ours = format!(
        "{}{}",
        record("shared", "2026-09-05T08:00:00Z"),
        record("ours", "2026-09-05T08:00:01Z")
    );
    commit_file(&store, relative, &ours, "our turn");

    merge(&store, "other");

    let merged = read(&store, relative);
    assert!(merged.contains("\"ours\""), "our record survives: {merged}");
    assert!(
        merged.contains("\"theirs\""),
        "their record survives: {merged}"
    );
    assert!(
        !merged.contains("<<<<"),
        "the driver never leaves conflict markers: {merged}"
    );
    assert_eq!(merged.lines().count(), 3, "one shared record, not two");
}

#[test]
fn a_memory_document_edited_on_both_sides_keeps_ours_and_sets_theirs_aside() {
    let temp = TempDir::new("driver-keepboth");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let relative = "projects/Project/memory/note.md";
    commit_file(&store, relative, "base text\n", "base");
    register_drivers(&store);

    git(&store, &["checkout", "--quiet", "-b", "other"]);
    commit_file(&store, relative, "their text\n", "their edit");
    git(&store, &["checkout", "--quiet", "-"]);
    commit_file(&store, relative, "our text\n", "our edit");

    merge(&store, "other");

    let merged = read(&store, relative);
    assert_eq!(
        merged, "our text\n",
        "this machine's version stays in the file"
    );
    assert!(
        !merged.contains("<<<<"),
        "memory must never carry conflict markers: {merged}"
    );
}

#[test]
fn a_merge_leaves_no_temporary_files_behind() {
    let temp = TempDir::new("driver-clean");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let relative = "projects/Project/session.jsonl";
    commit_file(
        &store,
        relative,
        &record("a", "2026-09-05T08:00:00Z"),
        "base",
    );
    register_drivers(&store);

    git(&store, &["checkout", "--quiet", "-b", "other"]);
    commit_file(
        &store,
        relative,
        &format!(
            "{}{}",
            record("a", "2026-09-05T08:00:00Z"),
            record("b", "2026-09-05T08:00:02Z")
        ),
        "theirs",
    );
    git(&store, &["checkout", "--quiet", "-"]);
    commit_file(
        &store,
        relative,
        &format!(
            "{}{}",
            record("a", "2026-09-05T08:00:00Z"),
            record("c", "2026-09-05T08:00:01Z")
        ),
        "ours",
    );
    merge(&store, "other");

    let left: Vec<String> = fs::read_dir(store.join("projects").join("Project"))
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("vibememory-merge"))
        .collect();
    assert!(left.is_empty(), "temporary files left behind: {left:?}");
}

#[test]
fn a_driver_that_cannot_run_leaves_ours_exactly_as_git_wrote_it() {
    let temp = TempDir::new("driver-refuse");
    let ours = temp.path().join("ours.jsonl");
    let untouched = "what git put here\n";
    fs::write(&ours, untouched).expect("write");
    fs::write(temp.path().join("base.jsonl"), b"").expect("write");
    fs::write(temp.path().join("theirs.jsonl"), b"").expect("write");

    let status = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
        .args([
            "merge-driver",
            "no-such-driver",
            &temp.path().join("base.jsonl").display().to_string(),
            &ours.display().to_string(),
            &temp.path().join("theirs.jsonl").display().to_string(),
            "projects/Project/session.jsonl",
        ])
        .output()
        .expect("run the driver");

    assert!(
        !status.status.success(),
        "an unknown driver must fail, so the caller aborts instead of committing a guess"
    );
    assert_eq!(
        fs::read_to_string(&ours).expect("read"),
        untouched,
        "%A must be exactly as git wrote it when the driver refuses"
    );
}

#[test]
fn an_unreadable_input_leaves_ours_untouched() {
    let temp = TempDir::new("driver-unreadable");
    let ours = temp.path().join("ours.jsonl");
    let untouched = "what git put here\n";
    fs::write(&ours, untouched).expect("write");
    fs::write(temp.path().join("theirs.jsonl"), b"").expect("write");
    // A directory where git promised a file: `read` fails, and the driver must stop before it
    // writes anything at all.
    let base = temp.dir("base-is-a-directory");

    let outcome = vibememory_cli::merge_driver::run(
        vibememory_cli::merge_driver::Driver::Jsonl,
        &base,
        &ours,
        &temp.path().join("theirs.jsonl"),
        "projects/Project/session.jsonl",
        "mac-test-2026-09-05",
    );

    assert!(outcome.is_err(), "an unreadable input is a refusal");
    assert_eq!(
        fs::read_to_string(&ours).expect("read"),
        untouched,
        "%A must be exactly as git wrote it when the driver refuses"
    );
}

#[test]
fn the_losing_memory_version_is_written_into_the_quarantine() {
    let temp = TempDir::new("driver-quarantine");
    let store = temp.dir("store");
    let engine = temp.dir("engine");
    git_repo_with_commit(&store);
    let relative = "projects/Project/memory/note.md";
    commit_file(&store, relative, "base text\n", "base");
    register_drivers_with_engine(&store, &engine);

    git(&store, &["checkout", "--quiet", "-b", "other"]);
    commit_file(&store, relative, "their text\n", "their edit");
    git(&store, &["checkout", "--quiet", "-"]);
    commit_file(&store, relative, "our text\n", "our edit");
    merge(&store, "other");

    assert_eq!(read(&store, relative), "our text\n");
    let set_aside = vibememory_cli::memory::quarantined(&engine);
    assert_eq!(
        set_aside.len(),
        1,
        "the other machine's version must be on disk, not merely reported: {set_aside:?}"
    );
    let contents = fs::read_to_string(engine.join("quarantine").join(&set_aside[0]))
        .expect("read the quarantined file");
    assert_eq!(
        contents, "their text\n",
        "what was set aside must be the version that lost"
    );
}

#[test]
fn a_criss_cross_history_keeps_every_record() {
    let temp = TempDir::new("driver-crisscross");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let relative = "projects/Project/session.jsonl";
    commit_file(
        &store,
        relative,
        &record("base", "2026-09-05T08:00:00Z"),
        "base",
    );
    register_drivers(&store);
    git(&store, &["branch", "left"]);
    git(&store, &["branch", "right"]);

    // Two machines each merged the other once before, so the two branches now have two merge
    // bases and git has to invent a virtual ancestor. Recursive strategy calls the driver on that
    // invented base — which is a merge our driver produced, not a file any machine ever wrote.
    let mut lines = vec![record("base", "2026-09-05T08:00:00Z")];
    git(&store, &["checkout", "--quiet", "left"]);
    lines.push(record("left-1", "2026-09-05T08:01:00Z"));
    commit_file(&store, relative, &lines.concat(), "left one");

    git(&store, &["checkout", "--quiet", "right"]);
    let mut right = vec![record("base", "2026-09-05T08:00:00Z")];
    right.push(record("right-1", "2026-09-05T08:02:00Z"));
    commit_file(&store, relative, &right.concat(), "right one");

    // Each side takes the other's first record: this is what makes the history criss-cross.
    merge(&store, "left");
    git(&store, &["checkout", "--quiet", "left"]);
    merge(&store, "right");

    // And now each adds one more and they meet again.
    let after_left = read(&store, relative);
    commit_file(
        &store,
        relative,
        &format!("{after_left}{}", record("left-2", "2026-09-05T08:03:00Z")),
        "left two",
    );
    git(&store, &["checkout", "--quiet", "right"]);
    let after_right = read(&store, relative);
    commit_file(
        &store,
        relative,
        &format!("{after_right}{}", record("right-2", "2026-09-05T08:04:00Z")),
        "right two",
    );
    merge(&store, "left");

    let merged = read(&store, relative);
    for uuid in ["base", "left-1", "right-1", "left-2", "right-2"] {
        assert!(
            merged.contains(&format!("\"{uuid}\"")),
            "{uuid} was lost across a criss-cross merge: {merged}"
        );
    }
    assert!(!merged.contains("<<<<"), "no markers: {merged}");
    assert_eq!(
        merged.lines().count(),
        5,
        "each record exactly once, whatever ancestor git invented: {merged}"
    );
}
