//! The tick, driven against two real clones of one bare repository — the closest thing to two
//! machines that fits inside a test.

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
use std::path::{Path, PathBuf};

use support::{TempDir, git, git_repo_with_commit};
use vibememory_cli::forget::forget;
use vibememory_cli::hook::stop::{Live, LiveSession};
use vibememory_cli::tick::{TickLock, Ticked, run};
use vibememory_core::desktop::roots::Roots;
use vibememory_core::naming::PathSyntax;

const CUTOFF: &str = "2026-09-05T00:00:00Z";
const STAMP: &str = "2026-09-05T10:00:00Z";
const SESSION: &str = "11111111-1111-4111-8111-111111111111";

fn relative() -> String {
    format!("projects/Project/{SESSION}.jsonl")
}

/// A bare repository and two clones of it, as two machines share one store.
struct Pair {
    mac: PathBuf,
    other: PathBuf,
}

fn two_machines(temp: &TempDir) -> Pair {
    let bare = temp.dir("remote.git");
    git(
        &bare,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
    );

    let mac = temp.dir("mac");
    git_repo_with_commit(&mac);
    git(&mac, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let path = relative();
    write_commit(&mac, &path, "{\"uuid\":\"one\"}\n", "first record");
    git(
        &mac,
        &["remote", "add", "origin", &bare.display().to_string()],
    );
    git(&mac, &["push", "--quiet", "-u", "origin", "main"]);

    let other = temp.path().join("other");
    git(
        temp.path(),
        &[
            "clone",
            "--quiet",
            &bare.display().to_string(),
            &other.display().to_string(),
        ],
    );
    git(&other, &["config", "user.email", "test@example.invalid"]);
    git(&other, &["config", "user.name", "test"]);
    Pair { mac, other }
}

/// One tick with the layout a real machine has: a config directory beside the store and no roots
/// declared, which is what a machine looks like before anybody sets them up.
fn tick(store: &Path, temp: &TempDir) -> Ticked {
    let config_dir = temp.dir("claude");
    let roots = Roots::new(std::collections::BTreeMap::new(), PathSyntax::Posix);
    run(store, &config_dir, "mac-test", &roots, None, STAMP, CUTOFF)
}

fn write_commit(store: &Path, relative: &str, contents: &str, message: &str) {
    let path = store.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(&path, contents).expect("write");
    git(store, &["add", relative]);
    git(store, &["commit", "--quiet", "-m", message]);
}

fn mark_live_at(store: &Path, machine: &str, session: &str, at: &str) {
    let dir = store.join("machines").join(machine);
    fs::create_dir_all(&dir).expect("dirs");
    let mut live = Live::default();
    live.sessions.insert(
        session.to_owned(),
        LiveSession {
            at: at.to_owned(),
            cwd: "/x".to_owned(),
        },
    );
    fs::write(
        dir.join("live.json"),
        serde_json::to_string(&live).expect("encode"),
    )
    .expect("write live");
}

/// A heartbeat as fresh as this tick: the session is working right now.
fn mark_live(store: &Path, machine: &str, session: &str) {
    mark_live_at(store, machine, session, STAMP);
}

#[test]
fn what_the_other_machine_wrote_arrives() {
    let temp = TempDir::new("tick-arrives");
    let pair = two_machines(&temp);
    write_commit(
        &pair.other,
        &relative(),
        "{\"uuid\":\"one\"}\n{\"uuid\":\"two\"}\n",
        "their record",
    );
    git(&pair.other, &["push", "--quiet", "origin", "main"]);

    let ticked = tick(&pair.mac, &temp);
    assert!(ticked.problems.is_empty(), "{:?}", ticked.problems);
    assert!(ticked.merged, "the tick must merge what it fetched");
    let here = fs::read_to_string(pair.mac.join(relative())).expect("read");
    assert!(here.contains("two"), "their record is here now: {here}");
}

#[test]
fn a_session_live_on_this_machine_holds_the_merge_back() {
    let temp = TempDir::new("tick-live");
    let pair = two_machines(&temp);
    write_commit(
        &pair.other,
        &relative(),
        "{\"uuid\":\"one\"}\n{\"uuid\":\"two\"}\n",
        "their record",
    );
    git(&pair.other, &["push", "--quiet", "origin", "main"]);
    mark_live(&pair.mac, "mac-test", SESSION);

    let ticked = tick(&pair.mac, &temp);
    assert!(!ticked.merged, "a live session may not be merged into");
    assert_eq!(ticked.held_back, vec![SESSION.to_owned()]);
    let here = fs::read_to_string(pair.mac.join(relative())).expect("read");
    assert!(
        !here.contains("two"),
        "the file under active append is untouched: {here}"
    );

    // When the session ends, the same tick does the work it held back.
    fs::write(
        pair.mac.join("machines").join("mac-test").join("live.json"),
        "{\"sessions\":{}}",
    )
    .expect("write");
    let ticked = tick(&pair.mac, &temp);
    assert!(ticked.merged, "nothing is live any more, so it merges");
}

#[test]
fn a_deleted_transcript_is_put_back_instead_of_pushed() {
    let temp = TempDir::new("tick-restore");
    let pair = two_machines(&temp);
    // Something removed the file: a sweeper, a sync client, somebody tidying up. Pushing that
    // deletion would take the records off every machine.
    fs::remove_file(pair.mac.join(relative())).expect("remove");

    let ticked = tick(&pair.mac, &temp);
    assert_eq!(ticked.restored, vec![relative()]);
    assert!(
        pair.mac.join(relative()).exists(),
        "the transcript must be back"
    );
}

#[test]
fn a_forget_removes_the_transcript_on_this_machine_too() {
    let temp = TempDir::new("tick-forget");
    let pair = two_machines(&temp);
    forget(&pair.mac, "other-machine", SESSION, &relative(), STAMP).expect("forget");

    let ticked = tick(&pair.mac, &temp);
    assert_eq!(ticked.forgotten, vec![SESSION.to_owned()]);
    assert!(
        !pair.mac.join(relative()).exists(),
        "a forgotten session leaves no file behind"
    );
    // And the removal must not come back as a restore on the next tick.
    let again = tick(&pair.mac, &temp);
    assert!(
        again.restored.is_empty(),
        "the push-guard must not resurrect what was forgotten: {:?}",
        again.restored
    );
}

#[test]
fn a_store_without_a_remote_is_not_a_problem() {
    let temp = TempDir::new("tick-local");
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    write_commit(&store, &relative(), "{\"uuid\":\"one\"}\n", "first");

    let ticked = tick(&store, &temp);
    assert!(ticked.problems.is_empty(), "{:?}", ticked.problems);
    assert!(!ticked.fetched && !ticked.pushed);
}

#[test]
fn a_merge_that_cannot_be_done_is_aborted_and_leaves_a_clean_tree() {
    let temp = TempDir::new("tick-abort");
    let pair = two_machines(&temp);
    // No merge driver is configured in this clone — the state a machine is in before `install`
    // has run. Both sides then change the same lines, and git cannot resolve it alone.
    write_commit(
        &pair.other,
        &relative(),
        "{\"uuid\":\"theirs\"}\n",
        "their rewrite",
    );
    git(&pair.other, &["push", "--quiet", "origin", "main"]);
    write_commit(
        &pair.mac,
        &relative(),
        "{\"uuid\":\"ours\"}\n",
        "our rewrite",
    );

    let ticked = tick(&pair.mac, &temp);
    assert!(!ticked.merged, "an unresolvable merge is not a merge");
    assert!(
        !ticked.problems.is_empty(),
        "the tick must say why nothing happened"
    );

    let here = fs::read_to_string(pair.mac.join(relative())).expect("read");
    assert!(
        !here.contains("<<<<"),
        "the tree must be left without conflict markers: {here}"
    );
    assert_eq!(here, "{\"uuid\":\"ours\"}\n", "our version is untouched");
    // A repository left mid-merge would make every later commit record somebody's guess.
    assert!(
        !pair.mac.join(".git").join("MERGE_HEAD").exists(),
        "the repository must not be left in the middle of a merge"
    );
}

#[test]
fn two_ticks_do_not_run_over_each_other() {
    let temp = TempDir::new("tick-lock");
    let engine = temp.dir("engine");

    let held = TickLock::take(&engine).expect("first");
    TickLock::take(&engine).expect_err("a second tick must stand aside");
    drop(held);
    TickLock::take(&engine).expect("once the first has finished, the next one runs");
}

#[test]
fn a_lock_left_by_a_dead_process_is_taken_over() {
    let temp = TempDir::new("tick-stale");
    let engine = temp.dir("engine");
    // A machine that lost power leaves this behind. Nobody should have to remove it by hand.
    fs::write(engine.join("tick.lock"), b"999999").expect("write stale lock");

    TickLock::take(&engine).expect("a lock whose holder is gone is not a claim");
}

#[test]
fn nothing_incoming_is_not_reported_as_a_merge() {
    let temp = TempDir::new("tick-quiet");
    let pair = two_machines(&temp);
    // This machine is ahead, the other has written nothing. A two-dot diff would show our own
    // commit as a difference and make the tick claim a merge that never happened — and, worse,
    // hold back the merge of a session that is live here because of our own records.
    write_commit(
        &pair.mac,
        &relative(),
        "{\"uuid\":\"one\"}\n{\"uuid\":\"ours\"}\n",
        "our record",
    );
    mark_live(&pair.mac, "mac-test", SESSION);

    let ticked = tick(&pair.mac, &temp);
    assert!(!ticked.merged, "there was nothing to merge");
    assert!(
        ticked.held_back.is_empty(),
        "our own records may not hold back our own merges: {:?}",
        ticked.held_back
    );
    assert!(ticked.pushed, "but what we wrote does go out");

    // And a tick with nothing of its own to send says so by staying quiet.
    let again = tick(&pair.mac, &temp);
    assert!(
        !again.pushed,
        "a log that says `pushed` every two minutes cannot show a stuck machine"
    );
}

#[test]
fn memory_that_arrived_from_elsewhere_becomes_readable_files() {
    let temp = TempDir::new("tick-memory");
    let pair = two_machines(&temp);
    // The other machine wrote a memory record. Its journal arrives with the merge; without a
    // projection nobody could read it until a session happened to start in that project.
    let journal = "projects/Project/memory.jsonl";
    let event = "{\"uuid\":\"v1\",\"action\":\"upsert\",\"record\":{\"id\":\"store-naming\",\
\"kind\":\"project\",\"project\":\"Project\",\"title\":\"Store naming\",\
\"description\":\"where the name comes from\",\
\"body\":\"The name follows the git common dir.\",\"links\":[],\"agent\":\"gpd\",\
\"createdAt\":\"2026-09-05T09:00:00Z\",\"updatedAt\":\"2026-09-05T09:00:00Z\"}}\n";
    write_commit(&pair.other, journal, event, "their memory");
    git(&pair.other, &["push", "--quiet", "origin", "main"]);

    let ticked = tick(&pair.mac, &temp);
    assert!(ticked.merged, "the journal must arrive first");
    assert_eq!(
        ticked.projected_memory,
        vec!["Project".to_owned()],
        "the tick must say which project it made readable"
    );

    let index = fs::read_to_string(pair.mac.join("projects/Project/memory/MEMORY.md"))
        .expect("the index must exist");
    assert!(
        index.contains("Store naming"),
        "the record must be readable as markdown: {index}"
    );

    // And a second tick, with nothing new, writes nothing.
    let again = tick(&pair.mac, &temp);
    assert!(
        again.projected_memory.is_empty(),
        "an unchanged projection may not be rewritten every two minutes: {:?}",
        again.projected_memory
    );
}

#[test]
fn a_session_that_ended_without_a_hook_stops_blocking_everything() {
    let temp = TempDir::new("tick-stale");
    let pair = two_machines(&temp);
    write_commit(
        &pair.other,
        &relative(),
        "{\"uuid\":\"one\"}\n{\"uuid\":\"two\"}\n",
        "their record",
    );
    git(&pair.other, &["push", "--quiet", "origin", "main"]);
    // A crash, a kill, a lid closed on a dying battery: the heartbeat was never cleared, and this
    // claim would hold back every merge of that file for ever.
    mark_live_at(&pair.mac, "mac-test", SESSION, "2026-09-04T09:00:00Z");

    let ticked = tick(&pair.mac, &temp);
    assert_eq!(
        ticked.stale_sessions,
        vec![SESSION.to_owned()],
        "an hour-old heartbeat is not a live session"
    );
    let live: Live = serde_json::from_str(
        &fs::read_to_string(pair.mac.join("machines").join("mac-test").join("live.json"))
            .expect("read"),
    )
    .expect("parse");
    assert!(live.sessions.is_empty(), "the claim must be gone");

    // And the merge it was holding back happens on the next tick.
    let again = tick(&pair.mac, &temp);
    assert!(again.merged, "nothing blocks it any more");
}

#[test]
fn a_fresh_heartbeat_is_left_alone() {
    let temp = TempDir::new("tick-fresh");
    let pair = two_machines(&temp);
    mark_live(&pair.mac, "mac-test", SESSION);

    // The cutoff is older than the heartbeat: the session is working right now.
    let config_dir = temp.dir("claude");
    let roots = Roots::new(std::collections::BTreeMap::new(), PathSyntax::Posix);
    let ticked = run(
        &pair.mac,
        &config_dir,
        "mac-test",
        &roots,
        None,
        STAMP,
        "2020-01-01T00:00:00Z",
    );
    assert!(
        ticked.stale_sessions.is_empty(),
        "a live session must not be declared dead: {:?}",
        ticked.stale_sessions
    );
}

#[test]
fn what_the_hooks_wrote_into_the_outbox_is_committed_by_the_tick() {
    let temp = TempDir::new("tick-outbox-commit");
    let pair = two_machines(&temp);
    // A hook recorded progress: the files exist, nothing committed them.
    let transcript = temp.path().join("live.jsonl");
    fs::write(&transcript, "{\"uuid\":\"one\"}\n").expect("write");
    vibememory_cli::hook::stop::record_progress(
        &pair.mac,
        "mac-test",
        SESSION,
        "/x",
        &transcript,
        STAMP,
    )
    .expect("record");

    let ticked = tick(&pair.mac, &temp);
    assert!(ticked.outbox_committed > 0, "{ticked:?}");
    let output = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .current_dir(&pair.mac)
        .output()
        .expect("ls-tree");
    let tree = String::from_utf8_lossy(&output.stdout);
    assert!(
        tree.contains("machines/mac-test/tails.json"),
        "until the tick commits it, nothing this machine says reaches the others: {tree}"
    );
    assert_eq!(
        tick(&pair.mac, &temp).outbox_committed,
        0,
        "an unchanged outbox makes no commit"
    );
}
