//! Shared scaffolding for the engine's tests.
//!
//! Everything a test touches lives under the system temporary directory and is removed when the
//! test ends. Nothing here may name a real home, a real config directory or a real store —
//! `tests/test_safety.rs` checks that mechanically, because one forgetful path in a test of this
//! crate is not a failing test but lost history.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros,
    dead_code
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Generous enough that a healthy git never hits it, short enough that a wedged one does not hang
/// the suite.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A directory of this test run, removed when the guard is dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates a fresh directory named after the test and this process.
    pub fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "vibememory-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A subdirectory, created.
    pub fn dir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(&path).expect("create subdirectory");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs git in `dir` and fails the test when git does.
pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|e| panic!("run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// A repository with one commit, so that `HEAD` exists.
pub fn git_repo_with_commit(dir: &Path) {
    git(dir, &["init", "--quiet"]);
    git(dir, &["config", "user.email", "test@example.invalid"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Runs git the way the tick does, and hands back what it said.
///
/// Under launchd the tick has a fixed `PATH` that holds nothing of the engine
/// (`vibememory_cli::install::LAUNCHD_PATH`) and a home of its own: a merge driver git cannot find
/// from here is one the tick cannot run, whatever a developer's shell would say. The home is the
/// test's, never a real one. Elsewhere the environment is kept — the scheduled task on Windows runs
/// with the user's.
pub fn git_as_the_tick(dir: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    if cfg!(target_os = "macos") {
        command
            .env_clear()
            .env("PATH", vibememory_cli::install::LAUNCHD_PATH)
            .env("HOME", home);
    }
    command
        .output()
        .unwrap_or_else(|e| panic!("run git {args:?} as the tick: {e}"))
}

/// A directory link made the way the engine makes one: a symlink on unix, a junction on Windows.
pub fn link_dir(target: &Path, link: &Path) {
    vibememory_cli::dir_link::create(target, link)
        .unwrap_or_else(|e| panic!("link {} -> {}: {e}", link.display(), target.display()));
}
