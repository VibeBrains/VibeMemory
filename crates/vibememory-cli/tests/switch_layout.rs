//! `switch`: link surgery on a miniature of the old layout, recorded so it can be undone.
//!
//! Unix only, and truthfully so: `switch` migrates the old macOS scheme, whose `CLAUDE.md` and
//! `settings.json` were file symlinks into the synced folder. Windows never had those — a file
//! symlink there needs privileges — and its migration belongs to the bootstrap.
#![cfg(unix)]
// The test builds links and directories, so the purity gate is lifted here.
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

use support::TempDir;
use vibememory_cli::switch::{Change, SwitchInput, rollback, switch};

const ENC: &str = "-work-VibeIDE";
/// The two levels Desktop puts between its store and a card: an account and an organisation.
const CARD_DIR: &str = "28c0ec84/9e0b6fd7";
const CARD: &str = "local_67c711c9.json";

/// The machine as the old scheme left it: links into the synced folder everywhere.
struct Machine {
    config_dir: PathBuf,
    store: PathBuf,
    from: PathBuf,
    desktop: PathBuf,
    engine: PathBuf,
}

fn old_machine(temp: &TempDir) -> Machine {
    let from = temp.dir("old-claude");
    let store = temp.dir("store");
    let config_dir = temp.dir("claude");
    let engine = temp.dir("engine");
    // The old shared store and the new store both hold the project.
    fs::create_dir_all(from.join("projects/-ALL-/VibeIDE")).expect("old store");
    fs::create_dir_all(store.join("projects/VibeIDE")).expect("new store");
    fs::create_dir_all(config_dir.join("projects")).expect("projects");
    std::os::unix::fs::symlink(
        from.join("projects/-ALL-/VibeIDE"),
        config_dir.join("projects").join(ENC),
    )
    .expect("project link");
    // A real directory the CLI created before any link existed.
    fs::create_dir_all(config_dir.join("projects/-real-Project")).expect("real dir");
    // The files and the skills, linked into the synced folder; the store has its copies.
    for name in ["CLAUDE.md", "settings.json"] {
        fs::write(from.join(name), format!("old {name}\n")).expect("old file");
        fs::create_dir_all(store.join("config")).expect("config");
        fs::write(store.join("config").join(name), format!("store {name}\n")).expect("store file");
        std::os::unix::fs::symlink(from.join(name), config_dir.join(name)).expect("file link");
    }
    fs::create_dir_all(from.join("skills")).expect("old skills");
    fs::create_dir_all(store.join("config/skills")).expect("store skills");
    std::os::unix::fs::symlink(from.join("skills"), config_dir.join("skills"))
        .expect("skills link");
    // The Desktop store, a link into the synced folder. Its cards do not sit at the top: Desktop
    // keeps them under the account and the organisation, and the switch has to carry that tree.
    fs::create_dir_all(from.join("claude-code-sessions").join(CARD_DIR)).expect("old desktop");
    fs::write(
        from.join("claude-code-sessions").join(CARD_DIR).join(CARD),
        "{\"sessionId\":\"local_67c711c9\"}",
    )
    .expect("old card");
    let desktop = temp.path().join("Claude").join("claude-code-sessions");
    fs::create_dir_all(desktop.parent().expect("parent")).expect("app dir");
    std::os::unix::fs::symlink(from.join("claude-code-sessions"), &desktop).expect("desktop link");
    Machine {
        config_dir,
        store,
        from,
        desktop,
        engine,
    }
}

fn input(m: &Machine, desktop_running: bool) -> SwitchInput<'_> {
    SwitchInput {
        config_dir: &m.config_dir,
        store: &m.store,
        from: &m.from,
        desktop_store: Some(&m.desktop),
        desktop_running,
        engine_dir: &m.engine,
    }
}

fn link_target(path: &Path) -> PathBuf {
    fs::read_link(path).expect("read link")
}

#[test]
fn a_dry_run_describes_everything_and_changes_nothing() {
    let temp = TempDir::new("switch-dry");
    let m = old_machine(&temp);
    let done = switch(&input(&m, false), true).expect("dry run");

    assert_eq!(done.changes.len(), 5, "{:?}", done.changes);
    assert_eq!(done.real_directories.len(), 1);
    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.from.join("projects/-ALL-/VibeIDE"),
        "a dry run re-aims nothing"
    );
    assert!(
        fs::symlink_metadata(&m.desktop).expect("stat").is_symlink(),
        "and moves nothing"
    );
    assert!(!m.engine.join("switch-rollback.json").exists());
}

#[test]
fn the_switch_re_aims_links_materializes_files_and_records_everything() {
    let temp = TempDir::new("switch-apply");
    let m = old_machine(&temp);
    let done = switch(&input(&m, false), false).expect("switch");

    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.store.join("projects/VibeIDE"),
        "the project link points into the store"
    );
    let settings = m.config_dir.join("settings.json");
    assert!(
        !fs::symlink_metadata(&settings).expect("stat").is_symlink(),
        "settings.json is a real file now"
    );
    assert_eq!(
        fs::read_to_string(&settings).expect("read"),
        "store settings.json\n",
        "holding the store's copy"
    );
    assert_eq!(
        link_target(&m.config_dir.join("skills")),
        m.store.join("config/skills")
    );
    assert!(
        m.desktop.is_dir() && !fs::symlink_metadata(&m.desktop).expect("stat").is_symlink(),
        "the Desktop store is a real directory"
    );
    assert!(
        m.desktop
            .with_file_name("claude-code-sessions.bak-vibememory")
            .exists(),
        "the old link is kept, not deleted"
    );
    assert!(
        m.config_dir.join("projects/-real-Project").is_dir(),
        "a real directory is left for import"
    );

    let recorded: Vec<Change> = serde_json::from_str(
        &fs::read_to_string(m.engine.join("switch-rollback.json")).expect("rollback file"),
    )
    .expect("parse");
    assert_eq!(
        recorded, done.changes,
        "the rollback file holds every change"
    );
}

#[test]
fn a_rollback_puts_the_machine_back_exactly() {
    let temp = TempDir::new("switch-rollback");
    let m = old_machine(&temp);
    switch(&input(&m, false), false).expect("switch");
    let undone = rollback(&m.engine).expect("rollback");
    assert_eq!(undone.len(), 5);

    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.from.join("projects/-ALL-/VibeIDE")
    );
    assert_eq!(
        link_target(&m.config_dir.join("settings.json")),
        m.from.join("settings.json"),
        "the file is a link into the old folder again"
    );
    assert_eq!(
        link_target(&m.config_dir.join("skills")),
        m.from.join("skills")
    );
    assert!(
        fs::symlink_metadata(&m.desktop).expect("stat").is_symlink(),
        "the Desktop store is the old link again"
    );
    assert!(
        !m.engine.join("switch-rollback.json").exists(),
        "nothing left to roll back"
    );
}

#[test]
fn the_desktop_store_is_not_touched_while_desktop_runs() {
    let temp = TempDir::new("switch-desktop-live");
    let m = old_machine(&temp);
    let done = switch(&input(&m, true), false).expect("switch");
    assert!(
        fs::symlink_metadata(&m.desktop).expect("stat").is_symlink(),
        "Desktop rewrites a card on every focus; pulling the directory from under it loses one"
    );
    assert!(
        done.skipped.iter().any(|(path, _)| path == &m.desktop),
        "and the person is told why: {:?}",
        done.skipped
    );
    assert!(
        done.changes
            .iter()
            .all(|c| !matches!(c, Change::DesktopStore { .. })),
        "everything else still happens"
    );
}

#[test]
fn a_link_into_somewhere_else_is_left_alone() {
    let temp = TempDir::new("switch-foreign");
    let m = old_machine(&temp);
    let elsewhere = temp.dir("elsewhere");
    let foreign = m.config_dir.join("projects").join("-foreign");
    std::os::unix::fs::symlink(&elsewhere, &foreign).expect("foreign link");

    let done = switch(&input(&m, false), false).expect("switch");
    assert_eq!(link_target(&foreign), elsewhere, "not ours to move");
    assert!(done.skipped.iter().any(|(path, _)| path == &foreign));
}

#[test]
fn a_project_the_store_does_not_hold_yet_keeps_its_old_link() {
    let temp = TempDir::new("switch-unmigrated");
    let m = old_machine(&temp);
    fs::remove_dir_all(m.store.join("projects/VibeIDE")).expect("not migrated yet");

    let done = switch(&input(&m, false), false).expect("switch");
    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.from.join("projects/-ALL-/VibeIDE"),
        "a link to nothing is a link the CLI replaces with a real directory"
    );
    assert!(
        done.skipped
            .iter()
            .any(|(_, why)| why.contains("migrate first"))
    );
}

/// The CLI's registry entry for a session, as `sessions/<pid>.json` holds it.
fn register_session(config_dir: &Path, pid: u32, cwd: &Path) {
    let dir = config_dir.join("sessions");
    fs::create_dir_all(&dir).expect("sessions dir");
    fs::write(
        dir.join(format!("{pid}.json")),
        format!(
            "{{\"pid\":\"{pid}\",\"sessionId\":\"s\",\"cwd\":\"{}\",\"kind\":\"interactive\"}}",
            cwd.display()
        ),
    )
    .expect("registry entry");
}

/// A project link named after a working directory, into the old shared store.
fn link_for(m: &Machine, cwd: &Path) -> PathBuf {
    let enc = vibememory_core::naming::encode_cwd(
        &fs::canonicalize(cwd)
            .expect("canonical")
            .display()
            .to_string(),
    )
    .expect("enc");
    let link = m.config_dir.join("projects").join(enc.as_str());
    std::os::unix::fs::symlink(m.from.join("projects/-ALL-/VibeIDE"), &link).expect("link");
    link
}

#[test]
fn a_link_of_a_session_running_on_this_machine_is_left_alone() {
    let temp = TempDir::new("switch-live");
    let m = old_machine(&temp);
    // This very process, registered as working in the directory: the switch must find the link
    // busy, because re-aiming it under a session writing through it loses the session.
    let cwd = temp.dir("work/VibeIDE");
    let live_link = link_for(&m, &cwd);
    register_session(&m.config_dir, std::process::id(), &cwd);

    let done = switch(&input(&m, false), false).expect("switch");
    assert_eq!(
        link_target(&live_link),
        m.from.join("projects/-ALL-/VibeIDE"),
        "the busy link is exactly as it was"
    );
    assert!(
        done.skipped
            .iter()
            .any(|(p, why)| p == &live_link && why.contains("running")),
        "{:?}",
        done.skipped
    );
    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.store.join("projects/VibeIDE"),
        "every other link still moves"
    );
}

#[test]
fn a_registry_entry_of_a_dead_process_does_not_block_anything() {
    let temp = TempDir::new("switch-dead");
    let m = old_machine(&temp);
    let cwd = temp.dir("work/VibeIDE");
    let link = link_for(&m, &cwd);
    // The registry keeps entries of processes that crashed; a pid nobody has is not a session.
    register_session(&m.config_dir, 999_999, &cwd);

    switch(&input(&m, false), false).expect("switch");
    assert_eq!(link_target(&link), m.store.join("projects/VibeIDE"));
}

#[test]
fn a_live_desktop_spawned_session_means_desktop_is_running() {
    let temp = TempDir::new("switch-desktop-registry");
    let config_dir = temp.dir("claude");
    let dir = config_dir.join("sessions");
    fs::create_dir_all(&dir).expect("sessions dir");
    // This very process, registered the way Desktop registers the sessions it spawns.
    fs::write(
        dir.join(format!("{}.json", std::process::id())),
        format!(
            "{{\"pid\":\"{}\",\"cwd\":\"/x\",\"entrypoint\":\"claude-desktop\"}}",
            std::process::id()
        ),
    )
    .expect("registry entry");
    assert!(
        vibememory_cli::switch::desktop_session_live(&config_dir),
        "a session Desktop spawned cannot be alive while Desktop is not"
    );

    // The same entry with a dead pid says nothing.
    fs::write(
        dir.join(format!("{}.json", std::process::id())),
        "{\"pid\":\"999999\",\"cwd\":\"/x\",\"entrypoint\":\"claude-desktop\"}",
    )
    .expect("registry entry");
    assert!(!vibememory_cli::switch::desktop_session_live(&config_dir));
}

#[test]
fn the_new_desktop_store_carries_the_cards_the_old_one_held() {
    let temp = TempDir::new("switch-desktop-cards");
    let m = old_machine(&temp);
    let done = switch(&input(&m, false), false).expect("switch");

    let carried = m.desktop.join(CARD_DIR).join(CARD);
    assert!(
        carried.is_file(),
        "the card is where Desktop reads it, not left behind in the old store"
    );
    assert_eq!(
        fs::read_to_string(&carried).expect("card"),
        "{\"sessionId\":\"local_67c711c9\"}",
        "and it is the same card, byte for byte"
    );
    assert!(
        matches!(
            done.changes
                .iter()
                .find(|change| matches!(change, Change::DesktopStore { .. })),
            Some(Change::DesktopStore { cards: 1, .. })
        ),
        "the switch says how many it carried: {:?}",
        done.changes
    );
    // The old store keeps its copy: the switch moves the machine, it does not empty the archive.
    assert!(
        m.from
            .join("claude-code-sessions")
            .join(CARD_DIR)
            .join(CARD)
            .is_file(),
        "the card is still in the folder the switch moved aside"
    );
}

#[test]
fn running_the_switch_again_does_not_forget_how_to_undo_the_first_run() {
    let temp = TempDir::new("switch-twice");
    let m = old_machine(&temp);
    let first = switch(&input(&m, false), false).expect("first");
    assert!(!first.changes.is_empty());

    // The second run finds everything already done and changes nothing — the state a watcher
    // relaunched by launchd puts the machine in. Writing that empty result over the file would
    // leave the machine switched with no way back.
    let second = switch(&input(&m, false), false).expect("second");
    assert!(
        second.changes.is_empty(),
        "nothing is left to do: {:?}",
        second.changes
    );

    let recorded: Vec<Change> = serde_json::from_str(
        &fs::read_to_string(m.engine.join("switch-rollback.json")).expect("rollback file"),
    )
    .expect("valid rollback file");
    assert_eq!(
        recorded, first.changes,
        "the second run must keep what the first one recorded"
    );

    // And the rollback still works after that second run.
    let undone = rollback(&m.engine).expect("rollback");
    assert_eq!(undone.len(), first.changes.len());
    assert_eq!(
        link_target(&m.config_dir.join("projects").join(ENC)),
        m.from.join("projects/-ALL-/VibeIDE"),
        "the machine is back where it started"
    );
}
