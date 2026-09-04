//! Which project names the store already holds — read from the committed tree and from the disk,
//! in a temporary directory that stands in for a real store.

// The test builds a real repository, so the purity gate is lifted here.
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

use support::{TIMEOUT, TempDir, git, git_repo_with_commit};
use vibememory_cli::store::{PROJECTS_DIR, existing};

/// Creates `projects/<name>/.keep` and returns nothing: the store keeps a file in every project
/// directory so that git carries the directory at all.
fn add_project(store: &Path, name: &str) {
    let dir = store.join(PROJECTS_DIR).join(name);
    fs::create_dir_all(&dir).expect("create project dir");
    fs::write(dir.join(".keep"), b"").expect("write .keep");
}

fn names(store: &Path) -> Vec<String> {
    existing(store, TIMEOUT)
        .names
        .iter()
        .map(|name| name.as_str().to_owned())
        .collect()
}

#[test]
fn a_store_that_does_not_exist_yet_holds_nothing() {
    let temp = TempDir::new("store-empty");
    let found = existing(&temp.path().join("no-such-store"), TIMEOUT);
    assert!(found.names.is_empty() && found.rejected.is_empty());
}

#[test]
fn directories_on_disk_are_found_without_any_commit() {
    let temp = TempDir::new("store-disk");
    add_project(temp.path(), "VibeIDE");
    add_project(temp.path(), "VibeMemory");
    assert_eq!(names(temp.path()), vec!["VibeIDE", "VibeMemory"]);
}

#[test]
fn a_stray_file_under_projects_is_not_a_project() {
    let temp = TempDir::new("store-stray");
    fs::create_dir_all(temp.path().join(PROJECTS_DIR)).expect("create projects");
    fs::write(temp.path().join(PROJECTS_DIR).join("README.md"), b"x").expect("write file");
    add_project(temp.path(), "VibeIDE");
    assert_eq!(names(temp.path()), vec!["VibeIDE"]);
}

#[test]
fn the_committed_tree_shows_a_collision_the_disk_cannot() {
    let temp = TempDir::new("store-collision");
    git_repo_with_commit(temp.path());
    add_project(temp.path(), "VibeIDE");
    git(temp.path(), &["add", "-A"]);
    git(temp.path(), &["commit", "--quiet", "-m", "one"]);

    // The second spelling is committed directly into the tree: on a case-insensitive file system
    // it cannot exist next to the first one on disk, but git keeps both, and the other machine
    // will fetch both.
    git(
        temp.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "100644,e69de29bb2d1d6434b8b29ae775ad8c2e48c5391,projects/vibeide/.keep",
        ],
    );
    git(temp.path(), &["commit", "--quiet", "-m", "two"]);

    let found = existing(temp.path(), TIMEOUT);
    let names: Vec<&str> = found
        .names
        .iter()
        .map(vibememory_core::naming::StoreName::as_str)
        .collect();
    assert_eq!(
        names,
        vec!["VibeIDE"],
        "one identity keeps one spelling, and the committed one wins"
    );
}

#[test]
fn a_name_that_lives_only_in_the_committed_tree_is_still_taken() {
    let temp = TempDir::new("store-tree-only");
    git_repo_with_commit(temp.path());
    add_project(temp.path(), "VibeIDE");
    git(temp.path(), &["add", "-A"]);
    git(temp.path(), &["commit", "--quiet", "-m", "one"]);
    // The working copy is cleaned, as it would be on a machine that has fetched but not checked
    // the project out. The name is taken all the same: the other machine already uses it.
    fs::remove_dir_all(temp.path().join(PROJECTS_DIR)).expect("remove projects");

    assert_eq!(names(temp.path()), vec!["VibeIDE"]);
}

#[test]
fn an_illegal_name_is_reported_and_does_not_stop_the_rest() {
    let temp = TempDir::new("store-illegal");
    add_project(temp.path(), "VibeIDE");
    add_project(temp.path(), "CON");

    let found = existing(temp.path(), TIMEOUT);
    let names: Vec<&str> = found
        .names
        .iter()
        .map(vibememory_core::naming::StoreName::as_str)
        .collect();
    assert_eq!(names, vec!["VibeIDE"], "the legal name survives");
    assert_eq!(found.rejected.len(), 1, "the illegal one is reported");
    assert_eq!(found.rejected[0].name, "CON");
    assert!(
        !found.rejected[0].reason.trim().is_empty(),
        "doctor needs a reason to show"
    );
}
