//! What the engine keeps back: a session file that holds an agent token stays on this machine, the
//! next session hears of it once, and a file that no longer holds one goes as usual.

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

#[test]
fn a_session_file_with_a_token_is_held_and_announced_once() {
    let temp = TempDir::new("held-screen");
    let (engine, store) = (temp.dir("engine"), temp.dir("store"));
    let line = token_case("transcriptToolResult");
    for relative in [TRANSCRIPT, MEMORY] {
        let path = store.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &line).unwrap();
    }
    let paths = [TRANSCRIPT.to_owned(), MEMORY.to_owned()];

    let passed = held::screen(&engine, &store, &paths, STAMP).expect("screen");
    assert_eq!(passed, [MEMORY], "only the session's own file is screened");
    let first = Held::read(&engine);
    assert_eq!(first.files[TRANSCRIPT].since, STAMP);
    assert_eq!(
        first.files[TRANSCRIPT].tokens.iter().collect::<Vec<_>>(),
        ["tk_7q2m9x4a"]
    );
    let notes = read_pending(&engine);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].contains(TRANSCRIPT) && notes[0].contains("tk_7q2m9x4a"));
    assert!(
        !notes[0].contains("vmt_"),
        "the secret is never said: {notes:?}"
    );

    // A later tick: still held, not announced again.
    let passed = held::screen(&engine, &store, &paths, LATER).expect("screen");
    assert_eq!(passed, [MEMORY]);
    assert_eq!(Held::read(&engine).files[TRANSCRIPT].since, STAMP);
    assert_eq!(read_pending(&engine).len(), 1);

    // The token is gone from the file: it goes as usual.
    fs::write(store.join(TRANSCRIPT), "{\"uuid\":\"one\"}\n").unwrap();
    let passed = held::screen(&engine, &store, &paths, LATER).expect("screen");
    assert_eq!(passed, [TRANSCRIPT, MEMORY]);
    assert!(Held::read(&engine).files.is_empty());
}

#[test]
fn the_stop_hook_holds_and_releases_a_live_transcript() {
    let temp = TempDir::new("held-snapshot");
    let engine = temp.dir("engine");
    let tokens = ["tk_7q2m9x4a".to_owned()].into_iter().collect();
    held::hold_snapshot(&engine, TRANSCRIPT, tokens, STAMP).expect("hold");
    assert!(Held::read(&engine).files.contains_key(TRANSCRIPT));
    assert_eq!(read_pending(&engine).len(), 1);
    // The session hears it; every later Stop of the growing file holds it again, silently.
    clear_pending(&engine).expect("clear");
    let same = ["tk_7q2m9x4a".to_owned()].into_iter().collect();
    held::hold_snapshot(&engine, TRANSCRIPT, same, LATER).expect("hold again");
    assert!(read_pending(&engine).is_empty(), "told once");
    assert_eq!(Held::read(&engine).files[TRANSCRIPT].since, STAMP);
    // Another token in it is news again.
    let more = ["tk_7q2m9x4a".to_owned(), "tk_b3c4d5e6".to_owned()]
        .into_iter()
        .collect();
    held::hold_snapshot(&engine, TRANSCRIPT, more, LATER).expect("hold more");
    assert_eq!(read_pending(&engine).len(), 1);
    held::release(&engine, TRANSCRIPT).expect("release");
    assert!(Held::read(&engine).files.is_empty());
}
