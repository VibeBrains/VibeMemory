//! The merge drivers driven by git itself: two branches that both appended to one transcript, and
//! one that both edited a memory document.

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

use support::{TempDir, git, git_repo_with_commit};

/// Registers the drivers in the repository, pointing at the binary this test run built.
fn register_drivers(store: &Path) {
    let binary = env!("CARGO_BIN_EXE_vibememory");
    for (key, driver) in [
        ("merge.vibememory-jsonl.driver", "jsonl"),
        ("merge.vibememory-keepboth.driver", "keepboth"),
    ] {
        let value = format!("{binary} merge-driver {driver} %O %A %B %P");
        git(store, &["config", "--local", key, &value]);
    }
    fs::write(
        store.join(".gitattributes"),
        "* -text\n* merge=vibememory-keepboth\n**/*.jsonl merge=vibememory-jsonl\n",
    )
    .expect("write attributes");
    git(store, &["add", ".gitattributes"]);
    git(store, &["commit", "--quiet", "-m", "attributes"]);
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

    git(&store, &["merge", "--quiet", "--no-edit", "other"]);

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

    git(&store, &["merge", "--quiet", "--no-edit", "other"]);

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
    git(&store, &["merge", "--quiet", "--no-edit", "other"]);

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
