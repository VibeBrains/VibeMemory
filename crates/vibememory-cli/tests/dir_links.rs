//! Directory links, made and removed the one way the engine does it.

// The test makes links and directories, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fs;

use support::TempDir;
use vibememory_cli::dir_link;

#[test]
fn a_link_reads_as_a_link_and_removing_it_leaves_the_target_whole() {
    let temp = TempDir::new("dir-link");
    let target = temp.dir("store/projects/Project");
    fs::write(target.join("session.jsonl"), "{}\n").expect("write");
    let link = temp
        .path()
        .join("claude")
        .join("projects")
        .join("-work-Project");
    fs::create_dir_all(link.parent().expect("parent")).expect("parent");

    dir_link::create(&target, &link).expect("create");
    // What every caller relies on: a link reads as a link and never as a real directory — the
    // import of a "real directory" ends in `remove_dir_all`. On Windows the standard library gives
    // a junction the same answer; that is read in its source, not run here.
    let metadata = fs::symlink_metadata(&link).expect("metadata");
    assert!(metadata.is_symlink() && !metadata.is_dir(), "{metadata:?}");
    assert_eq!(fs::read_link(&link).expect("read the link"), target);
    assert!(
        link.join("session.jsonl").is_file(),
        "the link leads into the store"
    );

    dir_link::remove(&link).expect("remove");
    assert!(fs::symlink_metadata(&link).is_err(), "the link is gone");
    assert_eq!(
        fs::read_to_string(target.join("session.jsonl")).expect("the target survives"),
        "{}\n"
    );
}
