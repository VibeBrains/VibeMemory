//! The block of `.git/info/exclude` that keeps a team clone's old sessions on this machine: it
//! grows without repeating itself, and leaves every line the person wrote where it was.

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
use vibememory_cli::local_only::{is_local, keep};

#[test]
fn the_block_grows_and_the_persons_lines_stay() {
    let temp = TempDir::new("local-only");
    let clone = temp.dir("clone");
    fs::create_dir_all(clone.join(".git/info")).unwrap();
    fs::write(clone.join(".git/info/exclude"), "*.swp\n").unwrap();

    keep(&clone, &["projects/app/a.jsonl".to_owned()]).unwrap();
    keep(
        &clone,
        &[
            "projects/app/a.jsonl".to_owned(),
            "projects/app/b.jsonl".to_owned(),
        ],
    )
    .unwrap();

    let text = fs::read_to_string(clone.join(".git/info/exclude")).unwrap();
    assert!(text.starts_with("*.swp\n"), "{text}");
    assert_eq!(text.matches("/projects/app/a.jsonl").count(), 1, "{text}");
    assert!(is_local(&clone, "projects/app/a.jsonl"));
    assert!(is_local(&clone, "projects/app/b.jsonl"));
    assert!(!is_local(&clone, "projects/app/c.jsonl"));
    assert!(!is_local(&clone, "*.swp"));
}

#[test]
fn a_clone_without_the_file_gets_one() {
    let temp = TempDir::new("local-only-new");
    let clone = temp.dir("clone");
    assert!(!is_local(&clone, "projects/app/a.jsonl"));
    keep(&clone, &["projects/app/a.jsonl".to_owned()]).unwrap();
    assert!(is_local(&clone, "projects/app/a.jsonl"));
}

#[test]
fn a_shared_session_leaves_the_block_with_its_directory() {
    let temp = TempDir::new("local-only-share");
    let clone = temp.dir("clone");
    let sid = "11111111-1111-4111-8111-111111111111";
    keep(
        &clone,
        &[
            format!("projects/app/{sid}.jsonl"),
            format!("projects/app/{sid}/subagents/a.jsonl"),
            "projects/app/22222222-2222-4222-8222-222222222222.jsonl".to_owned(),
        ],
    )
    .unwrap();
    let released = vibememory_cli::local_only::release(&clone, sid).unwrap();
    assert_eq!(released.len(), 2, "{released:?}");
    assert!(!is_local(&clone, &format!("projects/app/{sid}.jsonl")));
    assert!(is_local(
        &clone,
        "projects/app/22222222-2222-4222-8222-222222222222.jsonl"
    ));
    // an unknown session releases nothing
    assert!(
        vibememory_cli::local_only::release(&clone, "nope")
            .unwrap()
            .is_empty()
    );
}
