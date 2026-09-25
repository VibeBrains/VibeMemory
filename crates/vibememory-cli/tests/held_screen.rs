//! What the engine keeps back: a file that holds an agent token stays on this machine, the next
//! session hears of it once, and a file that no longer holds one goes as usual. A memory journal is
//! not screened here: its records are refused with a token when they are written.

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

use std::fs;

use support::{TempDir, token_case};
use vibememory_cli::held::{self, Held};
use vibememory_cli::merge_report::{clear_pending, read_pending};

const STAMP: &str = "2026-09-24T10:00:00Z";
const LATER: &str = "2026-09-24T11:00:00Z";
const TRANSCRIPT: &str = "projects/Project/11111111-1111-4111-8111-111111111111.jsonl";
const MEMORY: &str = "projects/Project/memory/note.md";
const JOURNAL: &str = "projects/Project/memory.jsonl";

#[test]
fn a_file_with_a_token_is_held_and_announced_once() {
    let temp = TempDir::new("held-screen");
    let (engine, store) = (temp.dir("engine"), temp.dir("store"));
    let line = token_case("transcriptToolResult");
    for relative in [TRANSCRIPT, MEMORY, JOURNAL] {
        let path = store.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &line).unwrap();
    }
    let paths = [TRANSCRIPT.to_owned(), MEMORY.to_owned(), JOURNAL.to_owned()];

    let passed = held::screen(&engine, &store, &paths, STAMP).expect("screen");
    assert_eq!(passed, [JOURNAL], "the journal alone is not screened here");
    let first = Held::read(&engine);
    assert_eq!(first.files[TRANSCRIPT].since, STAMP);
    assert!(first.files.contains_key(MEMORY));
    assert_eq!(
        first.files[TRANSCRIPT].tokens.iter().collect::<Vec<_>>(),
        ["tk_7q2m9x4a"]
    );
    let notes = read_pending(&engine);
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes.iter().all(|note| note.contains("tk_7q2m9x4a")),
        "{notes:?}"
    );
    assert!(
        !notes.iter().any(|note| note.contains("vmt_")),
        "the secret is never said: {notes:?}"
    );

    // A later tick: still held, not announced again.
    let passed = held::screen(&engine, &store, &paths, LATER).expect("screen");
    assert_eq!(passed, [JOURNAL]);
    assert_eq!(Held::read(&engine).files[TRANSCRIPT].since, STAMP);
    assert_eq!(read_pending(&engine).len(), 2);

    // The token is gone from the files: they go as usual.
    fs::write(store.join(TRANSCRIPT), "{\"uuid\":\"one\"}\n").unwrap();
    fs::write(store.join(MEMORY), "a note\n").unwrap();
    let passed = held::screen(&engine, &store, &paths, LATER).expect("screen");
    assert_eq!(passed, [TRANSCRIPT, MEMORY, JOURNAL]);
    assert!(Held::read(&engine).files.is_empty());
}

#[test]
fn the_stop_hook_holds_and_releases_a_live_transcript() {
    let temp = TempDir::new("held-snapshot");
    let engine = temp.dir("engine");
    let tokens = ["tk_7q2m9x4a".to_owned()].into_iter().collect();
    held::hold_found(&engine, TRANSCRIPT, tokens, STAMP).expect("hold");
    assert!(Held::read(&engine).files.contains_key(TRANSCRIPT));
    assert_eq!(read_pending(&engine).len(), 1);
    // The session hears it; every later Stop of the growing file holds it again, silently.
    clear_pending(&engine).expect("clear");
    let same = ["tk_7q2m9x4a".to_owned()].into_iter().collect();
    held::hold_found(&engine, TRANSCRIPT, same, LATER).expect("hold again");
    assert!(read_pending(&engine).is_empty(), "told once");
    assert_eq!(Held::read(&engine).files[TRANSCRIPT].since, STAMP);
    // Another token in it is news again.
    let more = ["tk_7q2m9x4a".to_owned(), "tk_b3c4d5e6".to_owned()]
        .into_iter()
        .collect();
    held::hold_found(&engine, TRANSCRIPT, more, LATER).expect("hold more");
    assert_eq!(read_pending(&engine).len(), 1);
    held::release(&engine, TRANSCRIPT).expect("release");
    assert!(Held::read(&engine).files.is_empty());
}

#[test]
fn the_owners_token_from_before_the_cabinet_is_recognised_once_it_is_kept() {
    let temp = TempDir::new("held-kept");
    let (engine, store) = (temp.dir("engine"), temp.dir("store"));
    // 64 hex characters, like any digest: no shape tells it apart, its value does
    let legacy = "0123456789abcdef".repeat(4);
    let kept = engine.join("tokens/personal");
    fs::create_dir_all(&kept).unwrap();
    fs::write(kept.join("claude-code"), format!("{legacy}\n")).unwrap();
    let path = store.join(TRANSCRIPT);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{{\"content\":\"Bearer {legacy}\"}}\n")).unwrap();

    let passed = held::screen(&engine, &store, &[TRANSCRIPT.to_owned()], STAMP).expect("screen");
    assert!(passed.is_empty());
    let held = Held::read(&engine);
    assert_eq!(
        held.files[TRANSCRIPT].tokens.iter().collect::<Vec<_>>(),
        ["personal/claude-code"],
        "named by where it is kept"
    );
    let written = fs::read_to_string(engine.join("held.json")).unwrap();
    assert!(
        !written.contains(&legacy),
        "the value is never written down"
    );
}

/// What git prints, run in `dir`.
fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn what_is_staged_is_what_was_checked() {
    let temp = TempDir::new("held-stage");
    let (engine, store) = (temp.dir("engine"), temp.dir("store"));
    support::git(&store, &["init", "--quiet"]);
    let note = "projects/Project/memory/other.md";
    let gone = "projects/Project/memory/gone.md";
    fs::create_dir_all(store.join("projects/Project/memory")).unwrap();
    fs::write(store.join(note), "checked\n").unwrap();
    fs::write(store.join(MEMORY), token_case("transcriptToolResult")).unwrap();
    fs::write(store.join(gone), "was here\n").unwrap();
    support::git(&store, &["add", gone]);
    fs::remove_file(store.join(gone)).unwrap();

    let paths = [note.to_owned(), MEMORY.to_owned(), gone.to_owned()];
    let staged = held::stage(
        &engine,
        &store,
        &paths,
        STAMP,
        std::time::Duration::from_secs(30),
    )
    .expect("stage");
    assert_eq!(staged, [note, gone]);
    // Written again after the check: the index keeps the bytes that were checked.
    fs::write(store.join(note), "written after the check\n").unwrap();
    assert_eq!(git_out(&store, &["show", &format!(":{note}")]), "checked\n");
    let listed = git_out(&store, &["ls-files"]);
    assert!(
        !listed.contains("gone.md"),
        "a gone file leaves the index: {listed}"
    );
    assert!(
        !listed.contains(MEMORY),
        "the file with a token is not staged: {listed}"
    );
    assert!(Held::read(&engine).files.contains_key(MEMORY));
}
