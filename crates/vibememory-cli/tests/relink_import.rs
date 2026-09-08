//! Re-aiming a link and importing a real directory — both refused while any machine says a
//! session is working there.

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

use std::fs;
use std::path::Path;

use support::TempDir;
use vibememory_cli::hook::stop::{Live, LiveSession};
use vibememory_cli::relink::{Refusal, import_real_directory, relink};

const ENC: &str = "-work-Project";
const CWD: &str = "{PROJECTS}/Project";

fn mark_live(store: &Path, machine: &str, session: &str, cwd: &str) {
    let dir = store.join("machines").join(machine);
    fs::create_dir_all(&dir).expect("dirs");
    let mut live = Live::default();
    live.sessions.insert(
        session.to_owned(),
        LiveSession {
            at: "2026-09-05T12:00:00Z".to_owned(),
            cwd: cwd.to_owned(),
        },
    );
    fs::write(
        dir.join("live.json"),
        serde_json::to_string(&live).expect("encode"),
    )
    .expect("write");
}

fn make_link(config_dir: &Path, target: &Path) {
    let link = config_dir.join("projects").join(ENC);
    fs::create_dir_all(link.parent().expect("parent")).expect("dirs");
    fs::create_dir_all(target).expect("target");
    std::os::unix::fs::symlink(target, &link).expect("symlink");
}

#[test]
fn a_link_is_re_aimed_when_nothing_is_using_it() {
    let temp = TempDir::new("relink-quiet");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    make_link(&config_dir, &store.join("projects").join("OldName"));

    let target = relink(&config_dir, &store, ENC, "NewName", CWD).expect("relink");
    assert_eq!(target, store.join("projects").join("NewName"));
    assert_eq!(
        fs::read_link(config_dir.join("projects").join(ENC)).expect("read link"),
        target
    );
}

#[test]
fn a_link_is_never_re_aimed_under_a_session_on_another_machine() {
    let temp = TempDir::new("relink-live");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    let old = store.join("projects").join("OldName");
    make_link(&config_dir, &old);
    // The laptop in the other room is working in this very directory. A pid on this machine would
    // have said nothing about that.
    mark_live(&store, "gpd-win", "session-1", CWD);

    let refusal = relink(&config_dir, &store, ENC, "NewName", CWD).expect_err("must refuse");
    assert_eq!(
        refusal,
        Refusal::SessionLive {
            machine: "gpd-win".to_owned(),
            session: "session-1".to_owned(),
        }
    );
    assert!(
        refusal.message().contains("gpd-win"),
        "the person must be told where to look: {}",
        refusal.message()
    );
    assert_eq!(
        fs::read_link(config_dir.join("projects").join(ENC)).expect("read link"),
        old,
        "the link must be exactly as it was"
    );
}

#[test]
fn a_live_session_in_a_different_directory_does_not_block_anything() {
    let temp = TempDir::new("relink-elsewhere");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    make_link(&config_dir, &store.join("projects").join("OldName"));
    mark_live(&store, "gpd-win", "session-1", "{PROJECTS}/Something-Else");

    relink(&config_dir, &store, ENC, "NewName", CWD).expect("another project is not this one");
}

#[test]
fn a_real_directory_is_copied_into_the_store_and_replaced_by_a_link() {
    let temp = TempDir::new("import-real");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    let real = config_dir.join("projects").join(ENC);
    fs::create_dir_all(real.join("memory")).expect("dirs");
    fs::write(real.join("session.jsonl"), b"{\"uuid\":\"one\"}\n").expect("write");
    fs::write(real.join("memory").join("note.md"), b"a note\n").expect("write");

    let imported = import_real_directory(
        &config_dir,
        &store,
        ENC,
        "Project",
        CWD,
        &temp.dir("engine"),
        "2026-09-08T00-00-00Z",
    )
    .expect("import");
    assert_eq!(imported.copied.len(), 2, "{:?}", imported.copied);

    let target = store.join("projects").join("Project");
    assert_eq!(
        fs::read(target.join("session.jsonl")).expect("read"),
        b"{\"uuid\":\"one\"}\n",
        "the transcript must be in the store now"
    );
    assert_eq!(
        fs::read(target.join("memory").join("note.md")).expect("read"),
        b"a note\n",
        "and so must everything under it"
    );
    assert!(
        fs::symlink_metadata(&real).expect("stat").is_symlink(),
        "and the CLI must find a link where its directory used to be"
    );
}

#[test]
fn a_file_the_store_already_has_is_not_overwritten_by_the_local_copy() {
    let temp = TempDir::new("import-keep");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    let real = config_dir.join("projects").join(ENC);
    fs::create_dir_all(&real).expect("dirs");
    fs::write(real.join("session.jsonl"), b"local only\n").expect("write");
    let target = store.join("projects").join("Project");
    fs::create_dir_all(&target).expect("dirs");
    fs::write(target.join("session.jsonl"), b"merged from two machines\n").expect("write");

    let imported = import_real_directory(
        &config_dir,
        &store,
        ENC,
        "Project",
        CWD,
        &temp.dir("engine"),
        "2026-09-08T00-00-00Z",
    )
    .expect("import");
    assert_eq!(imported.kept, vec!["session.jsonl".to_owned()]);
    assert_eq!(
        fs::read(target.join("session.jsonl")).expect("read"),
        b"merged from two machines\n",
        "what is in the store came through a merge and knows more than this machine's copy"
    );
}

#[test]
fn a_real_directory_is_not_imported_under_a_live_session() {
    let temp = TempDir::new("import-live");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    let real = config_dir.join("projects").join(ENC);
    fs::create_dir_all(&real).expect("dirs");
    fs::write(real.join("session.jsonl"), b"being written right now\n").expect("write");
    mark_live(&store, "mac-test", "session-1", CWD);

    let refusal = import_real_directory(
        &config_dir,
        &store,
        ENC,
        "Project",
        CWD,
        &temp.dir("engine"),
        "2026-09-08T00-00-00Z",
    )
    .expect_err("must refuse");
    assert!(matches!(refusal, Refusal::SessionLive { .. }));
    assert!(
        real.join("session.jsonl").exists(),
        "the directory being written must be left exactly as it is"
    );
}

#[test]
fn asking_to_import_a_link_says_so_instead_of_doing_something_surprising() {
    let temp = TempDir::new("import-link");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    make_link(&config_dir, &store.join("projects").join("Project"));

    let refusal = import_real_directory(
        &config_dir,
        &store,
        ENC,
        "Project",
        CWD,
        &temp.dir("engine"),
        "2026-09-08T00-00-00Z",
    )
    .expect_err("must refuse");
    assert!(matches!(refusal, Refusal::NotApplicable { .. }));
}

#[test]
fn a_local_file_that_differs_from_the_store_is_set_aside_not_destroyed() {
    let temp = TempDir::new("import-quarantine");
    let config_dir = temp.dir("claude");
    let store = temp.dir("store");
    let engine = temp.dir("engine");
    let real = config_dir.join("projects").join(ENC);
    fs::create_dir_all(real.join("memory")).expect("dirs");
    // The local version is the newer one: this session wrote it, the store's copy is older.
    fs::write(
        real.join("memory").join("MEMORY.md"),
        "the newer local index\n",
    )
    .expect("write");
    let target = store.join("projects").join("Project");
    fs::create_dir_all(target.join("memory")).expect("dirs");
    fs::write(
        target.join("memory").join("MEMORY.md"),
        "the older store copy\n",
    )
    .expect("write");

    let imported = import_real_directory(
        &config_dir,
        &store,
        ENC,
        "Project",
        CWD,
        &engine,
        "2026-09-08T00-00-00Z",
    )
    .expect("import");

    assert_eq!(
        fs::read_to_string(target.join("memory").join("MEMORY.md")).expect("read"),
        "the older store copy\n",
        "the store's version still wins: it came through a merge"
    );
    assert_eq!(imported.quarantined.len(), 1, "{imported:?}");
    let waiting = vibememory_cli::memory::quarantined(&engine);
    assert_eq!(waiting.len(), 1, "{waiting:?}");
    let saved = fs::read_to_string(engine.join("quarantine").join(&waiting[0])).expect("read");
    assert_eq!(
        saved, "the newer local index\n",
        "the directory is deleted right after the copy, so a discarded version is gone for ever"
    );
}
