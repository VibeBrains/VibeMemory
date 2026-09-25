//! What `pre-receive` reads of a push for tokens: every object the push brings, merges included,
//! and no more than the host lets a push make it read. The objects are made with git's plumbing
//! and left unreferenced, as a push's objects are while the hook decides.

// The test builds repositories on disk and runs git, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vibememory_mcp::git_memories;
use vibememory_mcp::push_scan::{Scanned, scan};

/// A bare repository of this test, removed at the end.
struct Repo(PathBuf);

impl Repo {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vibememory-push-scan-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp");
        git(&path, &["init", "--quiet", "--bare"], None);
        Self(path)
    }

    fn blob(&self, content: &str) -> String {
        git(&self.0, &["hash-object", "-w", "--stdin"], Some(content))
    }

    fn tree(&self, entries: &[(&str, &str)]) -> String {
        let mut listing = String::new();
        for (name, blob) in entries {
            listing.push_str("100644 blob ");
            listing.push_str(blob);
            listing.push('\t');
            listing.push_str(name);
            listing.push('\n');
        }
        git(&self.0, &["mktree"], Some(&listing))
    }

    fn commit(&self, tree: &str, parents: &[&str], message: &str) -> String {
        let mut args = vec!["commit-tree", tree];
        for parent in parents {
            args.extend(["-p", parent]);
        }
        args.extend(["-m", message]);
        git(&self.0, &args, None)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str], input: Option<&str>) -> String {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write");
    }
    let output = child.wait_with_output().expect("wait");
    assert!(output.status.success(), "git {args:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A line with an agent token in it, from the fixture of such lines.
fn token_line() -> String {
    support::fixture("fixtures/export/settingsWithToken.json")["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "transcriptToolResult")
        .and_then(|case| case["text"].as_str())
        .expect("the case")
        .to_owned()
}

#[test]
fn a_token_a_merge_writes_while_resolving_is_found() {
    let repo = Repo::new("merge");
    let base = repo.commit(&repo.tree(&[("a.txt", &repo.blob("a\n"))]), &[], "base");
    let left = repo.commit(
        &repo.tree(&[("a.txt", &repo.blob("left\n"))]),
        &[&base],
        "left",
    );
    let right = repo.commit(
        &repo.tree(&[("a.txt", &repo.blob("right\n"))]),
        &[&base],
        "right",
    );
    // the resolution is in the merge alone: no parent holds these bytes
    let resolved = repo.tree(&[("a.txt", &repo.blob(&token_line()))]);
    let merge = repo.commit(&resolved, &[&left, &right], "merge");
    assert_eq!(
        scan(&repo.0, &merge, u64::MAX).expect("scan"),
        Scanned::Found(vec!["a.txt".to_owned()])
    );
}

#[test]
fn a_push_holding_more_than_the_host_reads_is_too_large() {
    let repo = Repo::new("large");
    let commit = repo.commit(
        &repo.tree(&[("big.txt", &repo.blob(&"x".repeat(4096)))]),
        &[],
        "big",
    );
    assert!(matches!(
        scan(&repo.0, &commit, 1024).expect("scan"),
        Scanned::TooLarge { bytes } if bytes > 4096
    ));
    assert_eq!(
        scan(&repo.0, &commit, u64::MAX).expect("scan"),
        Scanned::Found(Vec::new())
    );
}

#[test]
fn many_questions_and_long_answers_do_not_wedge_git() {
    let repo = Repo::new("pipes");
    // Every answer is longer than its question, and all of them together far more than a pipe holds
    let mut asked = String::new();
    for index in 0..8000_u32 {
        writeln!(asked, "{index:040x}").expect("write");
    }
    let answers = git_memories::run(
        &repo.0,
        &["cat-file", "--batch-check"],
        Some(asked.as_bytes()),
        &[],
    )
    .expect("run");
    assert_eq!(String::from_utf8_lossy(&answers).lines().count(), 8000);
}
