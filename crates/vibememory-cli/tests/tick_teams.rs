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
/// An hour after `STAMP`: when a pause started at `STAMP` asks the host again.
const RECHECK: &str = "2026-09-05T11:00:00Z";
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
    tick_at(setup, team, STAMP)
}

/// One run at a given moment; a pause started now asks the host again an hour later.
fn tick_at(setup: &Setup, team: Option<&str>, stamp: &str) -> Ticked {
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
        recheck_at: RECHECK,
    };
    run(&machine, stamp, CUTOFF)
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

/// The files git tracks in a clone.
fn tracked(clone: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(clone)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
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

    // the session from before the project went to the team stays on this machine: in the clone
    // for `--resume`, out of git
    let clone = fs::canonicalize(setup.engine.join("stores/acme/store")).unwrap();
    let old = format!(
        "projects/{}/{TEAM_SESSION}.jsonl",
        linked.file_name().unwrap().to_string_lossy()
    );
    assert!(vibememory_cli::local_only::is_local(&clone, &old));
    assert!(
        tracked(&clone).iter().all(|path| path != &old),
        "{:?}",
        tracked(&clone)
    );

    // a session started after that is the team's
    let new = format!(
        "projects/{}/33333333-3333-4333-8333-333333333333.jsonl",
        linked.file_name().unwrap().to_string_lossy()
    );
    fs::write(
        clone.join(&new),
        format!(
            "{}\n",
            serde_json::json!({ "uuid": "n1", "cwd": setup.team_cwd })
        ),
    )
    .unwrap();
    let later = tick(&setup, Some("acme"));
    assert!(later.problems.is_empty(), "{later:?}");
    assert!(tracked(&clone).contains(&new), "{:?}", tracked(&clone));
    // the machine's own records reach the team: its teammates read the links and the heartbeats
    assert!(
        tracked(&clone).contains(&"machines/alice-mac/links.json".to_owned()),
        "{:?}",
        tracked(&clone)
    );
    assert!(!tracked(&clone).contains(&old));

    // the team's store carries sessions and memory only: a file under `config/` is not sent
    fs::create_dir_all(clone.join("config")).unwrap();
    fs::write(clone.join("config/settings.json"), "{}").unwrap();
    // a session given to the team explicitly goes with the next run
    vibememory_cli::local_only::release(&clone, TEAM_SESSION).unwrap();
    let shared = tick(&setup, Some("acme"));
    assert!(shared.problems.is_empty(), "{shared:?}");
    assert!(tracked(&clone).contains(&old), "{:?}", tracked(&clone));
    assert!(!tracked(&clone).contains(&"config/settings.json".to_owned()));

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

#[test]
fn a_host_refusal_for_the_teams_reasons_pauses_pushes_and_an_hour_later_it_asks_again() {
    let temp = TempDir::new("tick-teams-pause");
    let setup = setup(&temp);
    let team_store = setup.engine.join("stores/acme/store");
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
    // the host's answer, as its pre-receive writes it
    let hook = bare.join("hooks/pre-receive");
    fs::write(
        &hook,
        "#!/bin/sh\necho 'vibememory: quota' >&2\necho '262144000 bytes over a quota of 209715200' >&2\nexit 1\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(team_store.join("change"), "x").unwrap();
    git(&team_store, &["add", "change"]);
    git(&team_store, &["commit", "--quiet", "-m", "local change"]);

    let refused = tick(&setup, Some("acme"));
    assert!(
        refused.problems.is_empty(),
        "a pause is not a failure: {refused:?}"
    );
    let pause = refused.pause.clone().expect("paused");
    assert_eq!(pause.code, "quota");
    assert_eq!(pause.lines, ["262144000 bytes over a quota of 209715200"]);
    let state = TickState::read(&setup.engine.join("stores/acme"));
    assert_eq!(state.pause.as_ref(), Some(&pause));
    assert_eq!(state.consecutive_failures, 0);
    assert_eq!(
        TickState::read(&setup.engine).pause,
        None,
        "the personal store is not paused"
    );

    // before the recheck nothing is pushed, and the pause stays as it began
    fs::remove_file(&hook).unwrap();
    let waiting = tick_at(&setup, Some("acme"), "2026-09-05T10:30:00Z");
    assert!(!waiting.pushed);
    assert_eq!(
        waiting.pause.as_ref().map(|pause| pause.since.as_str()),
        Some(STAMP)
    );

    // the plan grew: the recheck pushes and the pause ends
    let again = tick_at(&setup, Some("acme"), RECHECK);
    assert!(again.pushed, "{again:?}");
    assert_eq!(again.pause, None);
    assert_eq!(
        TickState::read(&setup.engine.join("stores/acme")).pause,
        None
    );
}

#[test]
fn a_push_too_big_for_the_host_goes_in_portions() {
    let temp = TempDir::new("tick-teams-portions");
    let setup = setup(&temp);
    let team_store = setup.engine.join("stores/acme/store");
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
    // the host takes no pack over 4 KiB
    git(&bare, &["config", "receive.maxInputSize", "4096"]);
    // thirty commits of bytes that do not compress: far over the limit together, well under it
    // one by one
    let mut noise: u64 = 0x9e37_79b9_7f4a_7c15;
    for commit in 0..30 {
        let bytes: Vec<u8> = (0..1024)
            .map(|_| {
                noise ^= noise << 13;
                noise ^= noise >> 7;
                noise ^= noise << 17;
                noise.to_le_bytes()[0]
            })
            .collect();
        fs::write(team_store.join(format!("blob-{commit}")), bytes).unwrap();
        git(&team_store, &["add", "."]);
        git(
            &team_store,
            &["commit", "--quiet", "-m", &format!("offline {commit}")],
        );
    }

    let ticked = tick(&setup, Some("acme"));
    assert!(ticked.problems.is_empty(), "{ticked:?}");
    assert!(ticked.pushed);
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&team_store)
        .output()
        .unwrap();
    let remote = std::process::Command::new("git")
        .args(["rev-parse", "main"])
        .current_dir(&bare)
        .output()
        .unwrap();
    assert_eq!(head.stdout, remote.stdout, "every commit reached the host");
}
