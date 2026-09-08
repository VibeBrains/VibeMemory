//! Memory on disk: where it lives, how edits get back into the journal, and what the quarantine
//! does with a version that must not be lost.

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
use std::path::{Path, PathBuf};

use support::TempDir;
use vibememory_cli::memory::{
    JOURNAL_FILE, MemoryLocation, memory_dir, quarantine, quarantined, settings_memory_dir, sync,
};

const STAMP: &str = "2026-09-05T11:00:00Z";
const AGENT: &str = "claude-code";
const ENC: &str = "-tmp-project";

fn paths(temp: &TempDir) -> (PathBuf, PathBuf) {
    (temp.dir("memory"), temp.path().join(JOURNAL_FILE))
}

/// A document as the CLI writes one, with the frontmatter the engine reads back.
fn document(name: &str, body: &str, version: Option<&str>) -> String {
    let version_line = version.map_or_else(String::new, |v| format!("  version: {v}\n"));
    format!(
        "---\nname: {name}\ntitle: {name}\ndescription: a note\nmetadata:\n  type: project\n\
         {version_line}---\n\n{body}\n"
    )
}

fn read_index(memory: &Path) -> String {
    fs::read_to_string(memory.join("MEMORY.md")).expect("the index must be projected")
}

#[test]
fn the_default_memory_path_is_inside_the_project_directory() {
    let temp = TempDir::new("memory-path");
    let config = temp.dir("claude");
    let found = memory_dir(&MemoryLocation::default(), &config, ENC);
    assert_eq!(found, config.join("projects").join(ENC).join("memory"));
}

#[test]
fn every_override_moves_memory_and_cowork_outranks_the_rest() {
    let temp = TempDir::new("memory-override");
    let config = temp.dir("claude");

    let settings_only = MemoryLocation {
        settings_dir: Some("/elsewhere/settings"),
        ..MemoryLocation::default()
    };
    assert_eq!(
        memory_dir(&settings_only, &config, ENC),
        PathBuf::from("/elsewhere/settings"),
        "guessing the path from enc would make the store synchronise an empty directory"
    );

    let all_three = MemoryLocation {
        cowork_override: Some("/elsewhere/cowork"),
        remote_dir: Some("/elsewhere/remote"),
        settings_dir: Some("/elsewhere/settings"),
    };
    assert_eq!(
        memory_dir(&all_three, &config, ENC),
        PathBuf::from("/elsewhere/cowork"),
        "a Cowork session keeps its memory where Cowork says"
    );

    let blank = MemoryLocation {
        settings_dir: Some("   "),
        ..MemoryLocation::default()
    };
    assert_eq!(
        memory_dir(&blank, &config, ENC),
        config.join("projects").join(ENC).join("memory"),
        "an empty setting is not a path"
    );
}

#[test]
fn the_settings_key_is_read_and_a_missing_one_is_not_an_error() {
    let temp = TempDir::new("memory-settings");
    let settings = temp.path().join("settings.json");
    assert_eq!(settings_memory_dir(&settings), None, "no file, no override");

    fs::write(&settings, br#"{"autoMemoryDirectory":"/elsewhere"}"#).expect("write");
    assert_eq!(
        settings_memory_dir(&settings),
        Some("/elsewhere".to_owned())
    );

    fs::write(&settings, b"{ not json").expect("write");
    assert_eq!(
        settings_memory_dir(&settings),
        None,
        "a broken settings file must not decide where memory lives"
    );
}

#[test]
fn a_written_document_becomes_a_record_and_is_projected_back() {
    let temp = TempDir::new("memory-import");
    let (memory, journal) = paths(&temp);
    fs::write(
        memory.join("store-naming.md"),
        document("store-naming", "The name follows the git common dir.", None),
    )
    .expect("write");

    let synced = sync(&memory, &journal, STAMP, AGENT).expect("sync");
    assert_eq!(synced.imported, 1, "the edit became a version");
    assert!(synced.rejected.is_empty(), "{:?}", synced.rejected);
    assert!(
        journal.exists(),
        "the journal is the truth; the markdown is its projection"
    );
    assert!(
        read_index(&memory).contains("store-naming"),
        "the index must list the record"
    );
}

#[test]
fn reading_an_untouched_projection_writes_nothing() {
    let temp = TempDir::new("memory-quiet");
    let (memory, journal) = paths(&temp);
    fs::write(
        memory.join("store-naming.md"),
        document("store-naming", "First.", None),
    )
    .expect("write");
    sync(&memory, &journal, STAMP, AGENT).expect("first");
    let journal_after_first = fs::read(&journal).expect("read journal");

    // The hook runs on every stop; a projection nobody touched may not produce a version each
    // time, or the journal would grow without anybody writing anything.
    let again = sync(&memory, &journal, "2026-09-05T12:00:00Z", AGENT).expect("second");
    assert_eq!(again.imported, 0, "nothing changed, so nothing was written");
    assert!(again.written.is_empty(), "{:?}", again.written);
    assert_eq!(
        fs::read(&journal).expect("read journal"),
        journal_after_first,
        "the journal must be byte-for-byte what it was"
    );
}

#[test]
fn an_edit_made_after_the_projection_becomes_a_new_version() {
    let temp = TempDir::new("memory-edit");
    let (memory, journal) = paths(&temp);
    fs::write(
        memory.join("store-naming.md"),
        document("store-naming", "First.", None),
    )
    .expect("write");
    sync(&memory, &journal, STAMP, AGENT).expect("first");

    let projected = fs::read_to_string(memory.join("store-naming.md")).expect("read");
    let edited = projected.replace("First.", "Second, edited by a person.");
    fs::write(memory.join("store-naming.md"), edited).expect("write edit");

    let synced = sync(&memory, &journal, "2026-09-05T12:00:00Z", AGENT).expect("second");
    assert_eq!(synced.imported, 1, "the edit is a version, not a loss");
    assert!(
        fs::read_to_string(memory.join("store-naming.md"))
            .expect("read")
            .contains("Second, edited by a person."),
        "the person's words survive the projection being rewritten"
    );
}

#[test]
fn a_document_that_cannot_be_read_is_reported_and_left_alone() {
    let temp = TempDir::new("memory-broken");
    let (memory, journal) = paths(&temp);
    let broken = memory.join("broken.md");
    fs::write(&broken, b"no frontmatter at all\n").expect("write");

    let synced = sync(&memory, &journal, STAMP, AGENT).expect("sync");
    assert_eq!(
        synced.rejected.len(),
        1,
        "the reader must say what it could not read"
    );
    assert_eq!(
        fs::read_to_string(&broken).expect("read"),
        "no frontmatter at all\n",
        "a file the engine could not understand is never rewritten"
    );
}

#[test]
fn a_quarantined_version_is_never_overwritten_by_the_next_one() {
    let temp = TempDir::new("memory-quarantine");
    let engine = temp.dir("engine");

    let first = quarantine(&engine, "memory-note.md", b"the first version").expect("first");
    // Two different store paths sanitise to one name (`a/b.md` and `a-b.md`), and the version
    // already set aside has no right to be replaced.
    let second = quarantine(&engine, "memory-note.md", b"the second version").expect("second");

    assert_ne!(first, second, "the second version needs its own name");
    assert_eq!(fs::read(&first).expect("read"), b"the first version");
    assert_eq!(fs::read(&second).expect("read"), b"the second version");
    assert_eq!(
        quarantined(&engine).len(),
        2,
        "both are waiting to be reconciled"
    );
}

#[test]
fn an_empty_quarantine_is_an_empty_list_not_a_failure() {
    let temp = TempDir::new("memory-noquarantine");
    assert!(quarantined(&temp.dir("engine")).is_empty());
}

#[test]
fn the_quarantine_notice_names_what_is_actually_waiting() {
    use vibememory_cli::memory::{Quarantined, quarantined_kind};

    // A folded Desktop card. Calling this "a version of a memory file" sends the reader looking
    // for a conflict between two machines' notes that does not exist.
    assert_eq!(
        quarantined_kind("local_02ea2773-529f-GPD-WIN-MAX2.json-2026-09-08T12:06:15Z"),
        Quarantined::Card
    );
    assert_eq!(
        quarantined_kind("deleted_bac59ceb-GPD-2026-09-08T12:06:15Z"),
        Quarantined::Card
    );
    // The kinds the quarantine held before cards arrived.
    assert_eq!(
        quarantined_kind("memory-MEMORY.md-2026-09-08T08:46:50Z"),
        Quarantined::Memory
    );
    assert_eq!(
        quarantined_kind("85bdb79b-d69d.jsonl-2026-09-08T08:46:50Z"),
        Quarantined::Transcript
    );
    // Named, never guessed about.
    assert_eq!(quarantined_kind("something-else"), Quarantined::Other);

    // Each kind says what it is and what to do, and no two say the same thing.
    let kinds = [
        Quarantined::Memory,
        Quarantined::Transcript,
        Quarantined::Card,
        Quarantined::Other,
    ];
    for kind in kinds {
        assert!(!kind.plural().is_empty() && !kind.what_to_do().is_empty());
    }
    let mut said: Vec<&str> = kinds.iter().map(|kind| kind.plural()).collect();
    said.sort_unstable();
    said.dedup();
    assert_eq!(said.len(), kinds.len(), "each kind must name itself");
    // A card in the quarantine is not pending work: it has already given up what it knew.
    assert!(
        Quarantined::Card.what_to_do().contains("delete"),
        "the notice must not ask for a reconciliation that cannot happen"
    );
}
