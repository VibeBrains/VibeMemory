//! A machine in a team with sessions on: the personal store and the team's store each take in and
//! link only the projects routed to them, and a failing team does not count against the personal
//! store's state — each store keeps its own tick state beside its clone.

// The test drives real git and writes files, so the purity gate is lifted here.
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
use std::path::{Path, PathBuf};

use support::{TempDir, git, git_repo_with_commit};
use vibememory_cli::guard::TickState;
use vibememory_cli::tick::{Machine, Ticked, run};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::{PathSyntax, StoreRoutes, encode_cwd};

const CUTOFF: &str = "2026-09-05T00:00:00Z";
const STAMP: &str = "2026-09-05T10:00:00Z";
const TEAM_SESSION: &str = "11111111-1111-4111-8111-111111111111";
const OWN_SESSION: &str = "22222222-2222-4222-8222-222222222222";

struct Setup {
    engine: PathBuf,
    config_dir: PathBuf,
    team_cwd: String,
    own_cwd: String,
    routes: StoreRoutes,
    /// The temporary directory as the root `T`, so records carry portable paths as on a real machine
    root: String,
}

/// A store clone with one commit, where the tick expects it.
fn store(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git_repo_with_commit(dir);
    fs::write(dir.join(".keep"), "").unwrap();
    git(dir, &["add", ".keep"]);
    git(dir, &["commit", "--quiet", "-m", "start"]);
}

/// A session directory Claude Code left in the config directory's `projects`, before any link.
fn session_dir(config_dir: &Path, cwd: &str, session: &str) {
    let dir = config_dir
        .join("projects")
        .join(encode_cwd(cwd).unwrap().as_str());
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{session}.jsonl")),
        format!("{}\n", serde_json::json!({ "uuid": "u1", "cwd": cwd })),
    )
    .unwrap();
}

fn setup(temp: &TempDir) -> Setup {
    let engine = temp.dir("engine");
    store(&engine.join("store"));
    store(&engine.join("stores/acme/store"));
    let work = fs::canonicalize(temp.dir("work/acme")).unwrap();
    let team_cwd = work.join("api").display().to_string();
    fs::create_dir_all(&team_cwd).unwrap();
    let own_cwd = fs::canonicalize(temp.dir("private/diary"))
        .unwrap()
        .display()
        .to_string();
    let config_dir = temp.dir("claude");
    session_dir(&config_dir, &team_cwd, TEAM_SESSION);
    session_dir(&config_dir, &own_cwd, OWN_SESSION);
    // the machine's prompt history: the personal store's, never a team's
    fs::write(
        config_dir.join("history.jsonl"),
        format!(
            "{}\n",
            serde_json::json!({ "display": "hi", "timestamp": 1, "project": own_cwd })
        ),
    )
    .unwrap();
    let routes =
        StoreRoutes::compile(&[("acme".to_owned(), vec![format!("{}/**", work.display())])])
            .unwrap();
    let root = fs::canonicalize(temp.path()).unwrap().display().to_string();
    Setup {
        engine,
        config_dir,
        team_cwd,
        own_cwd,
        routes,
        root,
    }
}

fn tick(setup: &Setup, team: Option<&str>) -> Ticked {
    let store = match team {
        None => setup.engine.join("store"),
        Some(id) => setup.engine.join("stores").join(id).join("store"),
    };
    let store = fs::canonicalize(store).unwrap();
    let roots = Roots::new(
        std::collections::BTreeMap::from([("T".to_owned(), setup.root.clone())]),
        PathSyntax::Posix,
    );
    let naming = vibememory_core::naming::NamingConfig::default();
    let machine = Machine {
        store: &store,
        config_dir: &setup.config_dir,
        machine_id: if team.is_some() {
            "alice-mac"
        } else {
            "mac-main"
        },
        roots: &roots,
        naming: &naming,
        desktop_store: None,
        max_deletions: vibememory_cli::guard::DEFAULT_MAX_DELETIONS_PER_TICK,
        deletions_released: false,
        team,
        routes: &setup.routes,
    };
    run(&machine, STAMP, CUTOFF)
}

/// Where a session directory of `cwd` points once linked, or `None` while it is a real directory.
fn link_of(setup: &Setup, cwd: &str) -> Option<PathBuf> {
    fs::read_link(
        setup
            .config_dir
            .join("projects")
            .join(encode_cwd(cwd).unwrap().as_str()),
    )
    .ok()
}

#[test]
fn each_store_takes_in_only_its_own_projects() {
    let temp = TempDir::new("tick-teams-routing");
    let setup = setup(&temp);

    let personal = tick(&setup, None);
    assert_eq!(personal.imported_directories.len(), 1, "{personal:?}");
    let own = link_of(&setup, &setup.own_cwd).expect("the personal project is linked");
    assert!(own.starts_with(fs::canonicalize(setup.engine.join("store")).unwrap()));
    assert_eq!(
        link_of(&setup, &setup.team_cwd),
        None,
        "the team's project waits for its run"
    );

    let team = tick(&setup, Some("acme"));
    assert_eq!(team.imported_directories.len(), 1, "{team:?}");
    assert_eq!(
        team.published,
        vibememory_cli::outbox::Moved::default(),
        "no history goes to the team: {team:?}"
    );
    assert!(
        !setup
            .engine
            .join("stores/acme/store/machines/alice-mac/history.jsonl")
            .exists(),
        "the team's clone holds no history of the machine"
    );
    let linked = link_of(&setup, &setup.team_cwd).expect("the team's project is linked");
    assert!(linked.starts_with(fs::canonicalize(setup.engine.join("stores/acme/store")).unwrap()));
    let moved = fs::canonicalize(setup.engine.join("stores/acme/store/projects"))
        .unwrap()
        .join(linked.file_name().unwrap())
        .join(format!("{TEAM_SESSION}.jsonl"));
    assert!(
        moved.is_file(),
        "the team's transcript is in the team's clone"
    );

    // a second personal run leaves the team's project where it is
    let again = tick(&setup, None);
    assert!(again.imported_directories.is_empty(), "{again:?}");
    assert_eq!(link_of(&setup, &setup.team_cwd), Some(linked));
}

#[test]
fn a_failing_team_does_not_count_against_the_personal_store() {
    let temp = TempDir::new("tick-teams-state");
    let setup = setup(&temp);
    let team_store = setup.engine.join("stores/acme/store");
    // the team's host goes away with a commit waiting: its push fails, the personal store has none
    let bare = temp.dir("acme.git");
    git(
        &bare,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );
    git(&team_store, &["branch", "-M", "main"]);
    git(
        &team_store,
        &["remote", "add", "origin", &bare.display().to_string()],
    );
    git(&team_store, &["push", "--quiet", "-u", "origin", "main"]);
    fs::remove_dir_all(&bare).unwrap();
    fs::write(team_store.join("change"), "x").unwrap();
    git(&team_store, &["add", "change"]);
    git(&team_store, &["commit", "--quiet", "-m", "local change"]);

    let team = tick(&setup, Some("acme"));
    assert!(!team.problems.is_empty(), "{team:?}");
    let personal = tick(&setup, None);
    assert!(personal.problems.is_empty(), "{personal:?}");

    assert_eq!(
        TickState::read(&setup.engine.join("stores/acme")).consecutive_failures,
        1
    );
    assert_eq!(TickState::read(&setup.engine).consecutive_failures, 0);
}

#[test]
fn a_team_run_links_no_project_of_another_store() {
    let temp = TempDir::new("tick-teams-reconcile");
    let setup = setup(&temp);
    // the personal project has no session directory here yet; a teammate's machine recorded a link
    // for the same working directory in the team's clone
    fs::remove_dir_all(
        setup
            .config_dir
            .join("projects")
            .join(encode_cwd(&setup.own_cwd).unwrap().as_str()),
    )
    .unwrap();
    let team_store = fs::canonicalize(setup.engine.join("stores/acme/store")).unwrap();
    let enc = encode_cwd(&setup.own_cwd).unwrap();
    vibememory_cli::links_file::record(
        &team_store,
        "bob-mac",
        &vibememory_cli::links_file::Observation {
            enc: enc.as_str(),
            name: "diary",
            cwd: &setup.own_cwd.replacen(&setup.root, "{T}", 1),
            syntax: PathSyntax::Posix,
            source: vibememory_core::links::LinkSource::Observed,
            confirmed_by: None,
        },
    )
    .unwrap();

    let team = tick(&setup, Some("acme"));
    assert!(team.linked.is_empty(), "{team:?}");
    assert_eq!(link_of(&setup, &setup.own_cwd), None);
}
