//! Rules about the platform the engine runs on that no compiler enforces.

// The gate reads the repository on purpose; the purity rule is lifted for this file alone.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::fs;
use std::path::{Path, PathBuf};

/// Reads of the home directory that go around the one function in `install.rs`.
///
/// Five places once read `HOME` themselves, and one of them turned its absence into an empty
/// string — which put the engine's directory under whatever directory it was started in. On
/// Windows `HOME` exists only inside Git Bash, so that was not a corner case: it was the
/// scheduled tick.
const DIRECT_HOME_READS: &[&str] = &["var(\"HOME\")", "var_os(\"HOME\")"];

#[test]
fn the_home_is_read_in_one_place() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    for name in ["vibememory-core", "vibememory-cli", "vibememory-mcp"] {
        for file in rust_files(&crates.join(name).join("src")) {
            let text = fs::read_to_string(&file).expect("read a source file");
            for (number, line) in text.lines().enumerate() {
                if DIRECT_HOME_READS.iter().any(|read| line.contains(read)) {
                    found.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "the home is read through the one function in install.rs:\n{}",
        found.join("\n")
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files
}
