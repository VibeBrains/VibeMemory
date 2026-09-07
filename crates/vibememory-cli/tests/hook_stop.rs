//! `Stop` commits a snapshot of a transcript that is still being written, and records how far the
//! session has got — against a real git repository in a temporary directory.

// The test drives real git, so the purity gate is lifted here.
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
use support::{TempDir, git, git_repo_with_commit};

use vibememory_cli::hook::stop::{
    Live, Tails, commit_snapshot, push_if_due, record_end, record_live, record_progress,
};

const STAMP: &str = "2026-09-05T09:00:00Z";
const RELATIVE: &str = "projects/Project/session.jsonl";

fn store(temp: &TempDir) -> std::path::PathBuf {
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    store
}

/// Two whole records plus the beginning of a third, as a live transcript looks when a hook runs.
fn live_transcript() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(br#"{"type":"user","uuid":"aaaa","timestamp":"2026-09-05T08:00:00Z"}"#);
    bytes.push(b'\n');
    bytes.extend_from_slice(
        br#"{"type":"assistant","uuid":"bbbb","timestamp":"2026-09-05T08:00:01Z"}"#,
    );
    bytes.push(b'\n');
    bytes.extend_from_slice(br#"{"type":"user","uu"#);
    bytes
}

fn show_committed(store: &Path, relative: &str) -> String {
    let output = std::process::Command::new("git")
        .args(["show", &format!("HEAD:{relative}")])
        .current_dir(store)
        .output()
        .expect("git show");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn a_half_written_record_is_never_committed() {
    let temp = TempDir::new("stop-snapshot");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write transcript");

    let stopped = commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("commit");
    assert!(stopped.committed);
    assert_eq!(stopped.truncated, 18, "the unfinished record is left out");

    let committed = show_committed(&store, RELATIVE);
    assert_eq!(committed.lines().count(), 2, "only whole records travel");
    assert!(
        !committed.contains("\"uu\""),
        "half of a record must never reach another machine: {committed}"
    );
}

#[test]
fn the_live_file_is_never_staged() {
    let temp = TempDir::new("stop-unstaged");
    let store = store(&temp);
    // A file inside the store's working tree that nobody asked to commit. `git add -A` would
    // sweep it in; the engine must not.
    fs::write(store.join("stray.txt"), b"not ours\n").expect("write stray");
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write transcript");

    commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("commit");

    let output = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .current_dir(&store)
        .output()
        .expect("ls-tree");
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(tree.contains(RELATIVE), "the snapshot is committed: {tree}");
    assert!(
        !tree.contains("stray.txt"),
        "nothing else may be swept in: {tree}"
    );
}

#[test]
fn a_session_that_added_nothing_makes_no_commit() {
    let temp = TempDir::new("stop-idle");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write transcript");

    assert!(
        commit_snapshot(&store, &transcript, RELATIVE, STAMP)
            .expect("first")
            .committed
    );
    let second = commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("second");
    assert!(
        !second.committed,
        "an unchanged transcript may not produce an empty commit every two minutes"
    );
}

#[test]
fn a_session_that_never_wrote_anything_is_not_an_error() {
    let temp = TempDir::new("stop-nofile");
    let store = store(&temp);
    let stopped = commit_snapshot(&store, &temp.path().join("absent.jsonl"), RELATIVE, STAMP)
        .expect("a missing transcript is normal");
    assert!(!stopped.committed && stopped.blob.is_none());
}

#[test]
fn progress_records_the_leaf_and_the_line_count() {
    let temp = TempDir::new("stop-progress");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write transcript");

    record_progress(
        &store,
        "mac-test",
        "session-1",
        "{PROJECTS}/Project",
        &transcript,
        STAMP,
    )
    .expect("record");

    let dir = store.join("machines").join("mac-test");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(dir.join("live.json")).expect("read live"))
            .expect("parse live");
    assert_eq!(live.sessions["session-1"].at, STAMP);
    assert_eq!(live.sessions["session-1"].cwd, "{PROJECTS}/Project");

    let tails: Tails =
        serde_json::from_str(&fs::read_to_string(dir.join("tails.json")).expect("read tails"))
            .expect("parse tails");
    let tail = &tails.sessions["session-1"];
    assert_eq!(tail.lines, 2, "the half-written record is not a line");
    assert_eq!(tail.last_uuid.as_deref(), Some("bbbb"));
}

#[test]
fn progress_of_one_session_does_not_erase_another() {
    let temp = TempDir::new("stop-two");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");

    record_progress(&store, "mac-test", "one", "/a", &transcript, STAMP).expect("first");
    record_progress(&store, "mac-test", "two", "/b", &transcript, STAMP).expect("second");

    let dir = store.join("machines").join("mac-test");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(dir.join("live.json")).expect("read"))
            .expect("parse");
    assert_eq!(
        live.sessions.len(),
        2,
        "two sessions run at once all the time"
    );
}

#[test]
fn a_corrupt_progress_file_does_not_stop_the_session() {
    let temp = TempDir::new("stop-corrupt");
    let store = store(&temp);
    let dir = store.join("machines").join("mac-test");
    fs::create_dir_all(&dir).expect("create");
    fs::write(dir.join("live.json"), b"{ this is not json").expect("write");
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");

    record_progress(&store, "mac-test", "one", "/a", &transcript, STAMP)
        .expect("a broken convenience file may not fail a hook");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(dir.join("live.json")).expect("read"))
            .expect("parse");
    assert_eq!(live.sessions.len(), 1, "it is rewritten from what is known");
}

#[test]
fn the_store_repository_is_left_on_a_clean_index() {
    let temp = TempDir::new("stop-clean");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");
    commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("commit");

    // Nothing may be left staged behind us: the next commit of the tick would carry it blindly.
    git(&store, &["diff-index", "--quiet", "--cached", "HEAD", "--"]);
}

#[test]
fn ending_a_session_takes_it_out_of_the_live_list() {
    let temp = TempDir::new("stop-end");
    let store = store(&temp);
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");

    record_progress(&store, "mac-test", "one", "/a", &transcript, STAMP).expect("one");
    record_progress(&store, "mac-test", "two", "/b", &transcript, STAMP).expect("two");
    record_end(&store, "mac-test", "one").expect("end");

    let path = store.join("machines").join("mac-test").join("live.json");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
    assert!(
        !live.sessions.contains_key("one"),
        "the ended session is gone"
    );
    assert!(
        live.sessions.contains_key("two"),
        "a session that is still running must survive somebody else ending"
    );

    // Ending a session nobody recorded is not an error: the hook may fire for a session that
    // never wrote anything.
    record_end(&store, "mac-test", "never-seen").expect("unknown session");
}

#[test]
fn a_push_waits_for_its_debounce_and_reaches_the_remote() {
    let temp = TempDir::new("stop-push");
    let store = store(&temp);
    let engine = temp.dir("engine");
    let remote = temp.dir("remote.git");
    git(&remote, &["init", "--bare", "--quiet"]);
    git(
        &store,
        &["remote", "add", "origin", &remote.display().to_string()],
    );
    git(&store, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");
    commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("commit");
    git(
        &store,
        &["push", "--quiet", "--set-upstream", "origin", "main"],
    );

    // A first push is due, a second one within the window is not.
    assert!(push_if_due(&store, &engine, 1_000_000, Duration::from_secs(20)).expect("first"));
    assert!(!push_if_due(&store, &engine, 1_000_010, Duration::from_secs(20)).expect("second"));
    assert!(
        push_if_due(&store, &engine, 1_000_030, Duration::from_secs(20)).expect("third"),
        "once the window has passed, the next stop pushes again"
    );

    let output = std::process::Command::new("git")
        .args(["log", "--oneline"])
        .current_dir(&remote)
        .output()
        .expect("git log");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("vibememory"),
        "the commit must actually reach the remote"
    );
}

#[test]
fn a_store_without_a_remote_does_not_bother_the_session() {
    let temp = TempDir::new("stop-noremote");
    let store = store(&temp);
    let engine = temp.dir("engine");
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");
    commit_snapshot(&store, &transcript, RELATIVE, STAMP).expect("commit");

    push_if_due(&store, &engine, 1_000_000, Duration::from_secs(20))
        .expect("a missing remote is not an error the session should hear about");
}

#[test]
fn a_session_is_live_from_its_start_not_from_its_first_stop() {
    let temp = TempDir::new("stop-live-early");
    let store = store(&temp);
    // What SessionStart records: no transcript exists yet, and the session has written nothing.
    record_live(&store, "mac-test", "session-1", "{PROJECTS}/Project", STAMP).expect("record");

    let path = store.join("machines").join("mac-test").join("live.json");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(
        live.sessions["session-1"].cwd, "{PROJECTS}/Project",
        "until this exists, the tick treats the directory as idle and may move it"
    );

    // And the first Stop refreshes it rather than duplicating it.
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, live_transcript()).expect("write");
    record_progress(
        &store,
        "mac-test",
        "session-1",
        "{PROJECTS}/Project",
        &transcript,
        STAMP,
    )
    .expect("progress");
    let live: Live =
        serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(live.sessions.len(), 1);
}
