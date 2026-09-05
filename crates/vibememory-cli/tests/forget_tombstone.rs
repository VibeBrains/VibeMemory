//! `forget` writes a statement into the outbox instead of deleting anything.

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

use support::TempDir;
use vibememory_cli::forget::{forget, read};

const RELATIVE: &str = "projects/Project/session.jsonl";

#[test]
fn a_forget_records_a_tombstone_and_deletes_nothing() {
    let temp = TempDir::new("forget-record");
    let store = temp.dir("store");
    let transcript = store.join(RELATIVE);
    fs::create_dir_all(transcript.parent().expect("parent")).expect("dirs");
    fs::write(&transcript, b"{}\n").expect("write");

    forget(
        &store,
        "mac-test",
        "session-1",
        RELATIVE,
        "2026-09-05T09:00:00Z",
    )
    .expect("forget");

    let forgotten = read(&store, "mac-test");
    assert_eq!(forgotten.sessions["session-1"].path, RELATIVE);
    assert!(
        transcript.exists(),
        "the file is removed by each machine's tick, not by this command: deleting it here would \
         meet the other copy as a tree conflict no driver is asked about"
    );
}

#[test]
fn forgetting_twice_keeps_the_first_moment() {
    let temp = TempDir::new("forget-twice");
    let store = temp.dir("store");
    forget(
        &store,
        "mac-test",
        "session-1",
        RELATIVE,
        "2026-09-05T09:00:00Z",
    )
    .expect("first");
    forget(
        &store,
        "mac-test",
        "session-1",
        RELATIVE,
        "2026-09-06T09:00:00Z",
    )
    .expect("second");

    let forgotten = read(&store, "mac-test");
    assert_eq!(
        forgotten.sessions["session-1"].at, "2026-09-05T09:00:00Z",
        "an old tombstone must not look new: another machine compares against that moment"
    );
}

#[test]
fn one_forget_does_not_erase_another() {
    let temp = TempDir::new("forget-many");
    let store = temp.dir("store");
    forget(&store, "mac-test", "one", RELATIVE, "2026-09-05T09:00:00Z").expect("one");
    forget(&store, "mac-test", "two", RELATIVE, "2026-09-05T09:00:01Z").expect("two");
    assert_eq!(read(&store, "mac-test").sessions.len(), 2);
}

#[test]
fn a_broken_outbox_file_is_not_a_deletion() {
    let temp = TempDir::new("forget-broken");
    let store = temp.dir("store");
    let dir = store.join("machines").join("mac-test");
    fs::create_dir_all(&dir).expect("dirs");
    fs::write(dir.join("forgotten.json"), b"{ not json").expect("write");

    assert!(
        read(&store, "mac-test").sessions.is_empty(),
        "an unreadable file must mean nothing to forget, never everything"
    );
    // And a later forget still works, rewriting what could not be read.
    forget(&store, "mac-test", "one", RELATIVE, "2026-09-05T09:00:00Z").expect("forget");
    assert_eq!(read(&store, "mac-test").sessions.len(), 1);
}
