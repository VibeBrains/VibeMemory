//! Open hand-offs are read from the hand-off files, not from the memory index.
//!
//! The index in `MEMORY.md` was observed being rewritten back to the CLI's default template
//! minutes after a session wrote an index into it, so a session that trusted it would start
//! believing nothing is in progress.

// The test writes files, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use support::TempDir;
use vibememory_cli::memory::open_handoffs;

fn write(dir: &std::path::Path, name: &str, body: &str) {
    std::fs::write(dir.join(name), body).expect("write hand-off");
}

#[test]
fn only_files_whose_frontmatter_says_open_are_reported() {
    let temp = TempDir::new("handoffs");
    let memory = temp.path().join("memory");
    let sessions = memory.join("sessions");
    std::fs::create_dir_all(&sessions).expect("create sessions");

    // The index says nothing is in progress; the files say otherwise. The files win.
    std::fs::write(
        memory.join("MEMORY.md"),
        "# Memory\n\nNothing remembered yet.\n",
    )
    .expect("write index");

    // Created in reverse alphabetical order, so the sorted answer is not the order they arrived.
    write(&sessions, "windows.md", "---\nstatus: open\n---\n\nwork\n");
    write(&sessions, "engine.md", "---\nstatus: open\n---\n\nwork\n");
    write(&sessions, "shipped.md", "---\nstatus: done\n---\n\ndone\n");
    // A body line identical to the marker must not count: only the frontmatter decides.
    write(
        &sessions,
        "prose.md",
        "---\nstatus: done\n---\n\nthe frontmatter I have to write next time:\n\nstatus: open\n",
    );
    // Not a hand-off at all.
    write(&sessions, "notes.txt", "---\nstatus: open\n---\n");

    // Order: APFS already hands back names alphabetically, so this assertion cannot prove the
    // sort. It is kept for filesystems that return hash order (ext4), where the message would
    // otherwise differ between machines.
    assert_eq!(open_handoffs(&memory), vec!["engine", "windows"]);
}

#[test]
fn a_project_without_hand_offs_reports_none() {
    let temp = TempDir::new("handoffs-empty");
    assert!(open_handoffs(temp.path()).is_empty());
}
