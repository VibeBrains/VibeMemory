//! What one machine records about its links, and what another machine does with it.

// The test writes links and directories, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use support::{TempDir, git_repo_with_commit};
use vibememory_cli::links_file::{Observation, read, record};
use vibememory_cli::tick::{Machine, run};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::links::LinkSource;
use vibememory_core::naming::PathSyntax;

const CUTOFF: &str = "2026-09-05T00:00:00Z";
const STAMP: &str = "2026-09-05T12:00:00Z";
const ENC: &str = "-work-Project";

fn roots(local_root: &Path) -> Roots {
    let mut entries = BTreeMap::new();
    entries.insert("PROJECTS".to_owned(), local_root.display().to_string());
    Roots::new(entries, PathSyntax::Posix)
}

fn observation<'a>(
    enc: &'a str,
    name: &'a str,
    cwd: &'a str,
    source: LinkSource,
) -> Observation<'a> {
    Observation {
        enc,
        name,
        cwd,
        syntax: PathSyntax::Posix,
        source,
        confirmed_by: None,
    }
}

#[test]
fn a_session_that_ran_here_records_a_confirmed_link() {
    let temp = TempDir::new("links-observed");
    let store = temp.dir("store");
    let mut observed = observation(ENC, "Project", "{PROJECTS}/Project", LinkSource::Observed);
    observed.confirmed_by = Some("/x/.claude/projects/-work-Project/s.jsonl");
    record(&store, "mac-test", &observed).expect("record");

    let file = read(&store, "mac-test");
    assert_eq!(file.links.len(), 1);
    assert!(file.links[0].is_confirmed(), "a session proved this link");
    assert!(
        !file.links[0].predicted,
        "what actually happened is not a prediction"
    );
}

#[test]
fn a_confirmed_link_is_not_demoted_by_a_later_guess() {
    let temp = TempDir::new("links-demote");
    let store = temp.dir("store");
    let mut observed = observation(ENC, "Project", "{PROJECTS}/Project", LinkSource::Observed);
    observed.confirmed_by = Some("/x/.claude/projects/-work-Project/s.jsonl");
    record(&store, "mac-test", &observed).expect("first");

    // A later run cannot see the transcript path — the session is over, the file may be gone.
    record(
        &store,
        "mac-test",
        &observation(ENC, "Project", "{PROJECTS}/Project", LinkSource::Reconciled),
    )
    .expect("second");

    let file = read(&store, "mac-test");
    assert_eq!(file.links.len(), 1, "one record per encoded directory");
    assert!(
        file.links[0].is_confirmed() && !file.links[0].predicted,
        "a proven link must not turn back into a guess"
    );
    assert_eq!(
        file.links[0].source,
        LinkSource::Observed,
        "how we learned a link is part of what we know: a reconciler's guess about a directory a \
         session actually ran in says less than the session did"
    );
}

#[test]
fn the_reconciler_creates_the_link_another_machine_needs() {
    let temp = TempDir::new("links-reconcile");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let config_dir = temp.dir("claude");
    let projects_root = temp.dir("Projects");
    fs::create_dir_all(projects_root.join("Project")).expect("the working directory exists here");

    record(
        &store,
        "gpd-win",
        &observation(ENC, "Project", "{PROJECTS}/Project", LinkSource::Observed),
    )
    .expect("their record");

    let ticked = run(
        &Machine {
            store: &store,
            config_dir: &config_dir,
            machine_id: "mac-test",
            roots: &roots(&projects_root),
            naming: &vibememory_core::naming::NamingConfig::default(),
            desktop_store: None,
            max_deletions: vibememory_cli::guard::DEFAULT_MAX_DELETIONS_PER_TICK,
            deletions_released: false,
            team: None,
            routes: &vibememory_core::naming::StoreRoutes::default(),
        },
        STAMP,
        CUTOFF,
    );
    assert_eq!(ticked.linked, vec![ENC.to_owned()], "{ticked:?}");
    assert_eq!(
        fs::read_link(config_dir.join("projects").join(ENC)).expect("read link"),
        store.join("projects").join("Project"),
        "the link must exist before any session looks for it: the fallback scans are blind to \
         symlinks"
    );

    let ours = read(&store, "mac-test");
    assert_eq!(ours.links.len(), 1);
    assert!(
        ours.links[0].predicted,
        "a link nobody has used yet is a prediction, and must say so"
    );

    // Running again changes nothing.
    let again = run(
        &Machine {
            store: &store,
            config_dir: &config_dir,
            machine_id: "mac-test",
            roots: &roots(&projects_root),
            naming: &vibememory_core::naming::NamingConfig::default(),
            desktop_store: None,
            max_deletions: vibememory_cli::guard::DEFAULT_MAX_DELETIONS_PER_TICK,
            deletions_released: false,
            team: None,
            routes: &vibememory_core::naming::StoreRoutes::default(),
        },
        STAMP,
        CUTOFF,
    );
    assert!(again.linked.is_empty(), "{:?}", again.linked);
}

#[test]
fn a_working_directory_this_machine_does_not_have_is_left_alone() {
    let temp = TempDir::new("links-absent");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let config_dir = temp.dir("claude");
    let projects_root = temp.dir("Projects");
    // The other machine works on a project that does not exist here.
    record(
        &store,
        "gpd-win",
        &observation(ENC, "Project", "{PROJECTS}/Project", LinkSource::Observed),
    )
    .expect("their record");

    let ticked = run(
        &Machine {
            store: &store,
            config_dir: &config_dir,
            machine_id: "mac-test",
            roots: &roots(&projects_root),
            naming: &vibememory_core::naming::NamingConfig::default(),
            desktop_store: None,
            max_deletions: vibememory_cli::guard::DEFAULT_MAX_DELETIONS_PER_TICK,
            deletions_released: false,
            team: None,
            routes: &vibememory_core::naming::StoreRoutes::default(),
        },
        STAMP,
        CUTOFF,
    );
    assert!(
        ticked.linked.is_empty(),
        "a project this machine does not have needs no link: {:?}",
        ticked.linked
    );
    assert!(!config_dir.join("projects").join(ENC).exists());
}

#[test]
fn two_machines_that_name_one_directory_differently_are_reported_not_resolved() {
    let temp = TempDir::new("links-disagree");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    let config_dir = temp.dir("claude");
    let projects_root = temp.dir("Projects");
    fs::create_dir_all(projects_root.join("Project")).expect("dirs");

    record(
        &store,
        "mac-test",
        &observation(ENC, "OurName", "{PROJECTS}/Project", LinkSource::Observed),
    )
    .expect("ours");
    record(
        &store,
        "gpd-win",
        &observation(ENC, "TheirName", "{PROJECTS}/Project", LinkSource::Observed),
    )
    .expect("theirs");

    let ticked = run(
        &Machine {
            store: &store,
            config_dir: &config_dir,
            machine_id: "mac-test",
            roots: &roots(&projects_root),
            naming: &vibememory_core::naming::NamingConfig::default(),
            desktop_store: None,
            max_deletions: vibememory_cli::guard::DEFAULT_MAX_DELETIONS_PER_TICK,
            deletions_released: false,
            team: None,
            routes: &vibememory_core::naming::StoreRoutes::default(),
        },
        STAMP,
        CUTOFF,
    );
    assert_eq!(
        ticked.disagreements,
        vec![ENC.to_owned()],
        "one of the two is wrong, and guessing which would split the project into two stores"
    );
    assert!(ticked.linked.is_empty(), "nothing may be created meanwhile");
}
