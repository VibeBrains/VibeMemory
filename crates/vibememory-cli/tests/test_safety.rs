//! What a test of the engine may never touch.
//!
//! Until now the whole project was pure logic: a test could not damage anything because it could
//! not reach the disk. This crate can. Its tests will create directories, write files and spawn
//! git — and the data they are about is the owner's real history: `~/.claude`, the store, the
//! Desktop descriptors. One test that forgets to redirect a path is not a failing test, it is
//! lost history.
//!
//! So the rule is checked mechanically rather than remembered: a test of this crate names no real
//! home, no real config directory and no real store. Everything it touches lives in a temporary
//! directory it made itself.

// The gate reads the repository on purpose; the purity rule is lifted for this file alone.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::fs;
use std::path::{Path, PathBuf};

/// Text that must not appear in a test of this crate, with the reason.
const FORBIDDEN: &[(&str, &str)] = &[
    (
        "/Users/",
        "an absolute macOS home: a test must build its paths under a temporary directory",
    ),
    ("/home/", "an absolute Linux home"),
    ("C:\\Users", "an absolute Windows home"),
    ("~/.claude", "the real config directory of Claude Code"),
    ("~/.vibememory", "the real engine directory"),
    (
        "home_dir",
        "the real home directory of whoever runs the tests",
    ),
    ("Application Support/Claude", "the real Desktop store"),
    (
        "OneDrive",
        "the cloud folder: reading it pulls files over the network",
    ),
];

/// Sources whose subject is exactly these strings, and which therefore may name them.
const EXEMPT: &[&str] = &["test_safety.rs"];

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).unwrap_or_else(|e| panic!("read {}: {e}", current.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn no_test_of_the_engine_names_real_user_data() {
    let mut failures: Vec<String> = Vec::new();
    let sources = rust_sources(&tests_dir());
    assert!(!sources.is_empty(), "no test sources were checked");

    for source in &sources {
        let name = source
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        let text = fs::read_to_string(source).expect("read test source");
        for (needle, why) in FORBIDDEN {
            if text.contains(needle) {
                failures.push(format!("{name}: names {needle:?} — {why}"));
            }
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
