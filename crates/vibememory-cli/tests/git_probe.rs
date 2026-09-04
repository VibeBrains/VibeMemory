//! What the engine observes about git, checked against a real git in a temporary directory.
//!
//! Every path here is built under the system temporary directory: nothing in this file may reach
//! the owner's repositories (`tests/test_safety.rs` enforces that mechanically).

// The test drives real git in a temporary directory, so the purity gate is lifted here.
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
use std::time::Duration;

use vibememory_cli::git::{probe, rev_parse_command, variables_to_remove, walk_for_dot_git};
use vibememory_core::naming::{DotGit, DotGitProbe, GitProbe};

use support::{TIMEOUT, TempDir, git};

/// Whether a path names the `.git` entry itself.
fn is_dot_git(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|name| name == ".git")
}

fn git_init(dir: &Path) {
    git(dir, &["init", "--quiet"]);
}

#[test]
fn a_plain_directory_is_not_a_repository() {
    let temp = TempDir::new("plain");
    let GitProbe::Ran {
        common_dir,
        dot_git,
    } = probe(temp.path(), TIMEOUT)
    else {
        panic!("git should have run");
    };
    assert_eq!(common_dir, None, "git must refuse outside a repository");
    // The temporary directory may itself sit inside something; what matters is that no `.git`
    // was found for the directory under test.
    assert!(
        matches!(dot_git, DotGitProbe::NotFound | DotGitProbe::Found(_)),
        "the walk must produce an answer, got {dot_git:?}"
    );
}

#[test]
fn a_repository_reports_its_common_dir_and_its_dot_git() {
    let temp = TempDir::new("repo");
    git_init(temp.path());

    let GitProbe::Ran {
        common_dir,
        dot_git,
    } = probe(temp.path(), TIMEOUT)
    else {
        panic!("git should have run");
    };
    let common_dir = common_dir.expect("git must answer inside a repository");
    assert!(
        is_dot_git(&common_dir),
        "common dir {common_dir:?} should be a .git directory"
    );
    let DotGitProbe::Found(DotGit::Dir { path }) = dot_git else {
        panic!("the walk should have found a .git directory");
    };
    assert!(is_dot_git(&path), "found {path:?}");
}

#[test]
fn a_subdirectory_belongs_to_the_repository_above_it() {
    let temp = TempDir::new("subdir");
    git_init(temp.path());
    let inner = temp.path().join("src").join("deep");
    fs::create_dir_all(&inner).expect("create subdirectory");

    let GitProbe::Ran {
        common_dir,
        dot_git,
    } = probe(&inner, TIMEOUT)
    else {
        panic!("git should have run");
    };
    assert!(
        common_dir.is_some(),
        "a subdirectory of a repository belongs to it"
    );
    assert!(
        matches!(dot_git, DotGitProbe::Found(DotGit::Dir { .. })),
        "the walk must find the .git above, got {dot_git:?}"
    );
}

#[test]
fn a_pointer_file_is_reported_with_its_text() {
    let temp = TempDir::new("pointer");
    let content = "gitdir: /somewhere/else/.git/worktrees/one\n";
    fs::write(temp.path().join(".git"), content).expect("write pointer");

    let DotGitProbe::Found(DotGit::File { path, content: got }) =
        walk_for_dot_git(temp.path(), TIMEOUT)
    else {
        panic!("the walk should have found a pointer file");
    };
    assert!(is_dot_git(&path), "found {path:?}");
    assert_eq!(got, content, "the pointer's text must survive verbatim");
}

#[test]
fn a_walk_that_cannot_finish_says_so_instead_of_guessing() {
    let temp = TempDir::new("deadline");
    let probe = walk_for_dot_git(temp.path(), Duration::ZERO);
    assert!(
        matches!(probe, DotGitProbe::Unknown { .. }),
        "an expired deadline must be unknown, not not-found: {probe:?}"
    );
}

#[test]
fn git_that_does_not_answer_in_time_is_unavailable() {
    let temp = TempDir::new("timeout");
    git_init(temp.path());
    let probe = probe(temp.path(), Duration::ZERO);
    assert!(
        matches!(probe, GitProbe::Unavailable { .. }),
        "a zero deadline must be unavailable, not a guess: {probe:?}"
    );
}

#[test]
fn every_git_variable_is_removed_except_the_exec_path() {
    let names = [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_EXEC_PATH",
        "GITHUB_TOKEN",
        "PATH",
        "GIT",
    ];
    let removed = variables_to_remove(names);
    assert_eq!(
        removed,
        vec!["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"],
        "GIT_EXEC_PATH must survive; names that merely start with GIT are not GIT_ variables"
    );
}

#[test]
fn the_spawned_command_carries_the_removals() {
    let temp = TempDir::new("env");
    let removals = vec!["GIT_DIR".to_owned(), "GIT_WORK_TREE".to_owned()];
    let command = rev_parse_command(temp.path(), &removals);
    let unset: Vec<String> = command
        .get_envs()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        unset, removals,
        "a variable the rule names must actually be unset for the child"
    );
}
