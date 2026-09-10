//! Migrating the old synced folder: a synthetic copy of its layout, with every kind of thing the
//! real one holds — conflict copies, an archive-only store, a card that lost its transcript, a
//! file the gate refuses, and a cloud placeholder.

// The test builds a directory tree and a git repository, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use support::{TempDir, git_repo_with_commit};
use vibememory_cli::migrate::{Kind, apply, plan};

const STAMP: &str = "2026-09-05T14:00:00Z";
const MACHINE: &str = "mac-main";

fn record(uuid: &str) -> String {
    format!("{{\"type\":\"user\",\"uuid\":\"{uuid}\",\"timestamp\":\"2026-09-05T08:00:00Z\"}}\n")
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(path, text).expect("write");
}

/// The old folder, in miniature.
fn old_folder(temp: &TempDir) -> PathBuf {
    let source = temp.dir("old-claude");
    let all = source.join("projects").join("-ALL-");
    // A repo with one transcript and the cloud client's conflict copy of it.
    write(&all.join("VibeIDE").join("s1.jsonl"), &record("a"));
    write(
        &all.join("VibeIDE").join("s1-MacMini.jsonl"),
        &format!("{}{}", record("a"), record("b-only-in-copy")),
    );
    // Memory edited on two machines.
    write(
        &all.join("VibeIDE").join("memory").join("MEMORY.md"),
        "mac version\n",
    );
    write(
        &all.join("VibeIDE")
            .join("memory")
            .join("MEMORY-GPD-WIN-MAX2.md"),
        "windows version\n",
    );
    // A Finder leftover the gate refuses.
    write(&all.join("VibeIDE").join(".DS_Store"), "junk");
    // The archive-only store: runner sessions with cwd `/`.
    write(&all.join("-").join("runner.jsonl"), &record("r"));
    // Desktop's cards: the main one lost its transcript here, the other machine's copy has it.
    let cards = source.join("claude-code-sessions").join("acct").join("org");
    write(
        &cards.join("local_11.json"),
        r#"{"sessionId":"local_11","cwd":"/x","transcriptUnavailable":true,"title":"t"}"#,
    );
    write(
        &cards.join("local_11-GPD-WIN-MAX2.json"),
        r#"{"sessionId":"local_11","cliSessionId":"sid-11","cwd":"D:\\x","title":"t"}"#,
    );
    // The config files and a skill.
    write(&source.join("CLAUDE.md"), "rules\n");
    write(&source.join("settings.json"), "{}\n");
    write(
        &source.join("skills").join("s").join("SKILL.md"),
        "a skill\n",
    );
    source
}

fn store(temp: &TempDir) -> PathBuf {
    let store = temp.dir("store");
    git_repo_with_commit(&store);
    store
}

#[test]
fn the_plan_reads_only_metadata_and_says_where_everything_goes() {
    let temp = TempDir::new("migrate-plan");
    let source = old_folder(&temp);
    let plan = plan(&source, MACHINE).expect("plan");

    let dests: Vec<&str> = plan.files.iter().map(|f| f.dest.as_str()).collect();
    assert!(dests.contains(&"projects/VibeIDE/s1.jsonl"), "{dests:?}");
    assert!(dests.contains(&"config/CLAUDE.md"), "{dests:?}");
    assert!(dests.contains(&"config/skills/s/SKILL.md"), "{dests:?}");
    assert!(
        dests.contains(&"machines/mac-main/desktop/local_11.json"),
        "the main card goes to this machine's outbox: {dests:?}"
    );
    assert!(
        dests.contains(&"machines/GPD-WIN-MAX2/desktop/local_11.json"),
        "the other machine's copy is that machine's version: {dests:?}"
    );
    assert!(
        !dests.iter().any(|d| d.contains("runner")),
        "the archive-only store stays behind: {dests:?}"
    );
    assert_eq!(plan.archived, 1);
    assert_eq!(plan.refused.len(), 1, "{:?}", plan.refused);
    assert!(plan.refused[0].0.ends_with(".DS_Store"));

    let copy = plan
        .files
        .iter()
        .find(|f| f.source.ends_with("s1-MacMini.jsonl"))
        .expect("the conflict copy is planned");
    assert_eq!(
        copy.kind,
        Kind::TranscriptCopy {
            into: "projects/VibeIDE/s1.jsonl".to_owned()
        }
    );
}

#[test]
fn applying_merges_copies_repairs_cards_and_commits() {
    let temp = TempDir::new("migrate-apply");
    let source = old_folder(&temp);
    let store = store(&temp);
    let engine = temp.dir("engine");
    let plan = plan(&source, MACHINE).expect("plan");

    let applied = apply(&source, &store, &engine, &plan, STAMP).expect("apply");
    assert!(applied.mismatched.is_empty(), "{:?}", applied.mismatched);
    assert!(applied.committed);

    let merged = fs::read_to_string(store.join("projects/VibeIDE/s1.jsonl")).expect("read");
    assert!(
        merged.contains("b-only-in-copy"),
        "the record only the conflict copy had must survive: {merged}"
    );
    assert_eq!(merged.lines().count(), 2, "and the shared one only once");

    assert_eq!(
        fs::read_to_string(store.join("projects/VibeIDE/memory/MEMORY.md")).expect("read"),
        "mac version\n",
        "the main file wins"
    );
    assert_eq!(
        applied.quarantined.len(),
        1,
        "and the other machine's memory is set aside, not lost: {:?}",
        applied.quarantined
    );

    let card = fs::read_to_string(store.join("machines/mac-main/desktop/local_11.json"))
        .expect("read card");
    assert!(
        card.contains("\"cliSessionId\": \"sid-11\""),
        "the transcript id comes back from the copy that kept it: {card}"
    );
    assert!(
        !card.contains("transcriptUnavailable"),
        "the mark about one machine's disk does not travel: {card}"
    );
    assert_eq!(applied.repaired_cards, vec!["local_11.json".to_owned()]);

    assert!(engine.join("migrate-manifest.json").exists());
    assert!(
        !store.join("projects/VibeIDE/.DS_Store").exists(),
        "what the gate refused never entered"
    );
}

#[test]
fn the_source_is_never_written_to() {
    let temp = TempDir::new("migrate-readonly");
    let source = old_folder(&temp);
    // Bytes and modification times both: rewriting a file with the same bytes still makes the
    // cloud client upload it again and hand the other machine a conflict copy.
    let snapshot = |root: &Path| -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
        walk(root)
            .into_iter()
            .map(|p| {
                let modified = fs::metadata(&p).expect("stat").modified().expect("mtime");
                (p.clone(), fs::read(&p).expect("read"), modified)
            })
            .collect()
    };
    let before = snapshot(&source);

    let store = store(&temp);
    apply(
        &source,
        &store,
        &temp.dir("engine"),
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("apply");

    assert_eq!(
        before,
        snapshot(&source),
        "not a byte and not a timestamp of the old folder may change"
    );
}

#[test]
fn running_again_is_a_no_op_and_brings_in_what_the_other_machine_added() {
    let temp = TempDir::new("migrate-again");
    let source = old_folder(&temp);
    let store = store(&temp);
    let engine = temp.dir("engine");
    apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("first");

    let again = apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("second");
    assert_eq!(again.copied, 0, "nothing new to copy: {again:?}");
    assert!(!again.committed, "an unchanged store makes no commit");
    assert!(again.mismatched.is_empty(), "{:?}", again.mismatched);

    // The other machine, still on the old folder, wrote another record. This is the mechanism
    // by which it keeps reaching the store until it migrates itself.
    let transcript = source.join("projects/-ALL-/VibeIDE/s1.jsonl");
    let mut text = fs::read_to_string(&transcript).expect("read");
    text.push_str(&record("c-written-later"));
    fs::write(&transcript, text).expect("write");

    let third = apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("third");
    assert!(third.committed, "the new record is a change worth a commit");
    let merged = fs::read_to_string(store.join("projects/VibeIDE/s1.jsonl")).expect("read");
    assert!(merged.contains("c-written-later"), "{merged}");
    assert!(
        merged.contains("b-only-in-copy"),
        "and nothing the store already had is lost: {merged}"
    );
}

#[test]
fn a_store_checked_out_with_windows_line_endings_comes_back_whole() {
    // A store cloned on Windows before its `.gitattributes` landed carries CRLF on disk. The next
    // run merges that copy with the source, and a line with no `uuid` is identified by its bytes:
    // before the format reader learned that the carriage return is git's and not the record's,
    // every such line came out twice — no conflict, no report, nothing to notice.
    let temp = TempDir::new("migrate-crlf");
    let source = old_folder(&temp);
    let state = "{\"type\":\"bridge-session\",\"sessionId\":\"s\"}\n";
    let transcript = source.join("projects/-ALL-/VibeIDE/s1.jsonl");
    let original = fs::read_to_string(&transcript).expect("read");
    fs::write(&transcript, format!("{original}{state}")).expect("write");

    let store = store(&temp);
    let engine = temp.dir("engine");
    apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("first");

    let dest = store.join("projects/VibeIDE/s1.jsonl");
    let lf = fs::read(&dest).expect("read");
    let mut crlf = Vec::with_capacity(lf.len());
    for byte in &lf {
        if *byte == b'\n' {
            crlf.push(b'\r');
        }
        crlf.push(*byte);
    }
    assert_ne!(
        crlf, lf,
        "the store copy must really change, or this proves nothing"
    );
    fs::write(&dest, &crlf).expect("write");

    let again = apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("second");
    assert!(
        again.mismatched.is_empty(),
        "a line ending is not a divergence: {:?}",
        again.mismatched
    );

    let merged = fs::read_to_string(&dest).expect("read");
    assert_eq!(
        merged.matches("bridge-session").count(),
        1,
        "the state line was kept once, not once per line ending: {merged}"
    );
    assert!(
        merged.contains('\r'),
        "and the store copy was not rewritten to another line ending behind git's back: {merged:?}"
    );
}

#[test]
fn a_cloud_placeholder_stops_the_apply_before_anything_is_read() {
    let temp = TempDir::new("migrate-placeholder");
    let source = old_folder(&temp);
    // A sparse file: a size with no blocks on disk, which is exactly what a cloud client's
    // placeholder looks like to `stat`.
    let placeholder = source.join("projects/-ALL-/VibeIDE/s2.jsonl");
    let file = fs::File::create(&placeholder).expect("create");
    file.set_len(4096).expect("set_len");
    drop(file);

    let plan = plan(&source, MACHINE).expect("plan");
    if plan.dehydrated.is_empty() {
        // This file system allocated blocks for the sparse file; the check cannot be exercised
        // here, and pretending it was would be worse than saying so.
        eprintln!(
            "sparse files are allocated on this file system; placeholder check not exercised"
        );
        return;
    }
    assert!(
        plan.dehydrated[0].ends_with("s2.jsonl"),
        "{:?}",
        plan.dehydrated
    );

    let store = store(&temp);
    let refused = apply(&source, &store, &temp.dir("engine"), &plan, STAMP)
        .expect_err("a placeholder must stop the apply");
    assert!(refused.contains("pin"), "{refused}");
    assert!(
        !store.join("projects").exists(),
        "and nothing at all may have been written first"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)
            .expect("read dir")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn two_memory_records_whose_names_nest_are_not_a_conflict_copy() {
    let temp = TempDir::new("migrate-kebab");
    let source = temp.dir("old-claude");
    let memory = source.join("projects/-ALL-/VibeIDE/memory");
    // Record ids are kebab-case, so one name often extends another. Reading the second as a
    // machine-suffixed copy of the first would quarantine a real note.
    write(&memory.join("store.md"), "about the store\n");
    write(&memory.join("store-naming.md"), "about naming\n");
    // Whereas a real conflict copy carries a machine name, capitals and all.
    write(
        &memory.join("store-MacMini.md"),
        "the other machine's store note\n",
    );

    let plan = plan(&source, MACHINE).expect("plan");
    let kind_of = |name: &str| {
        plan.files
            .iter()
            .find(|f| f.source.ends_with(name))
            .unwrap_or_else(|| panic!("{name} is planned"))
            .kind
            .clone()
    };
    assert_eq!(
        kind_of("store-naming.md"),
        Kind::Copy,
        "a record, not a copy"
    );
    assert_eq!(
        kind_of("store-MacMini.md"),
        Kind::OtherCopy {
            of: "projects/VibeIDE/memory/store.md".to_owned()
        }
    );
}

#[test]
fn a_memory_file_the_store_rewrote_since_does_not_stop_the_next_run() {
    let temp = TempDir::new("migrate-rewritten");
    let source = old_folder(&temp);
    let store = store(&temp);
    let engine = temp.dir("engine");
    apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("first");

    // Between two runs the engine projected memory into the store: MEMORY.md is now the store's
    // own rendering, and the old folder still holds the CLI's version. Meanwhile the other machine
    // appended a record to a transcript. The next run must keep the store's MEMORY.md, set the
    // old one aside, and still commit the record — a hash mismatch on a file that was decided by
    // keep-both is not a mismatch at all.
    write(
        &store.join("projects/VibeIDE/memory/MEMORY.md"),
        "# Memory\n\nprojected by the engine\n",
    );
    let transcript = source.join("projects/-ALL-/VibeIDE/s1.jsonl");
    let mut text = fs::read_to_string(&transcript).expect("read");
    text.push_str(&record("appended-elsewhere"));
    fs::write(&transcript, text).expect("write");

    let again = apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("second");
    assert!(again.mismatched.is_empty(), "{:?}", again.mismatched);
    assert!(
        again.committed,
        "the appended record must reach a commit: {again:?}"
    );
    assert_eq!(
        fs::read_to_string(store.join("projects/VibeIDE/memory/MEMORY.md")).expect("read"),
        "# Memory\n\nprojected by the engine\n",
        "the store's version is the one that knows more"
    );
    assert!(
        fs::read_to_string(store.join("projects/VibeIDE/s1.jsonl"))
            .expect("read")
            .contains("appended-elsewhere")
    );
}

#[test]
fn a_file_an_earlier_run_left_uncommitted_is_committed_by_the_next() {
    let temp = TempDir::new("migrate-leftover");
    let source = old_folder(&temp);
    let store = store(&temp);
    let engine = temp.dir("engine");
    apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("first");
    // An earlier run merged this and stopped before committing: the bytes are on disk, in the
    // plan's own destination, and nothing else will ever commit them.
    write(
        &store.join("projects/VibeIDE/s1.jsonl"),
        &format!("{}{}", record("a"), record("left-over")),
    );

    let again = apply(
        &source,
        &store,
        &engine,
        &plan(&source, MACHINE).expect("plan"),
        STAMP,
    )
    .expect("second");
    assert!(again.committed, "{again:?}");
    let status = std::process::Command::new("git")
        .args(["status", "--short", "--", "projects"])
        .current_dir(&store)
        .output()
        .expect("git status");
    assert!(
        status.stdout.is_empty(),
        "nothing of the plan may stay uncommitted: {}",
        String::from_utf8_lossy(&status.stdout)
    );
}
