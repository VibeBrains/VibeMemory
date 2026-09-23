//! The host's commands through the binary, on a copy of the host's layout in a temporary
//! directory: `access-apply` makes the team stores and `vmgit`'s keys and reports them, a machine
//! key clones and pushes through `shell` and a real `pre-receive`, and reaches its team's memory and
//! its status over the same forced command.
//!
//! Unix only: the host is Linux, and its hooks and forced commands are shell scripts.

#![cfg(unix)]
// The test builds repositories on disk and runs the binary, so the purity gate is lifted here.
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
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use vibememory_mcp::access;
use vibememory_mcp::apply;
use vibememory_mcp::git_memories::GitMemories;
use vibememory_mcp::layout;

const BINARY: &str = env!("CARGO_BIN_EXE_vibememory-mcp");
/// Alice's laptop in the fixtures: a member of the memory team `vibebrains`, an admin of the sync
/// team `syncteam`.
const ALICE_LAPTOP: &str = "mk_3f8k1p0z";

/// A copy of the host's layout, removed at the end.
struct Host {
    root: PathBuf,
    access: PathBuf,
    teams: PathBuf,
    keys: PathBuf,
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn bare(repo: &Path, args: &[&str]) -> String {
    let mut all = vec!["--git-dir", repo.to_str().expect("utf-8 path")];
    all.extend_from_slice(args);
    git(repo.parent().expect("parent"), &all)
}

/// A bare repository with one commit holding `files`.
fn bare_with(repo: &Path, files: &[(&str, &str)]) {
    let work = repo.with_extension("work");
    fs::create_dir_all(&work).expect("work");
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    for (path, content) in files {
        let file = work.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("dirs");
        fs::write(file, content).expect("file");
    }
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "seed"]);
    git(
        repo.parent().expect("parent"),
        &[
            "clone",
            "--quiet",
            "--bare",
            work.to_str().expect("utf-8"),
            repo.to_str().expect("utf-8"),
        ],
    );
    fs::remove_dir_all(&work).expect("remove work");
}

/// The host of the fixtures: the snapshot of `applyPlans.json` with `syncteam` writable and the
/// adopted store in the temporary directory, a deleted team's live directory and an orphan.
fn host(label: &str) -> Host {
    let root = std::env::temp_dir().join(format!("vibememory-host-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let host = Host {
        access: root.join("access/access.json"),
        teams: root.join("teams"),
        keys: root.join("vmgit/.ssh/authorized_keys"),
        root,
    };
    fs::create_dir_all(host.access.parent().expect("access dir")).expect("access dir");
    fs::create_dir_all(&host.teams).expect("teams");
    let personal = host.root.join("personal.git");
    bare_with(&personal, &[("projects/VibeMemory/memory.jsonl", "")]);
    bare_with(
        &host.teams.join("oldteam.git"),
        &[("projects/Old/memory.jsonl", "")],
    );
    bare_with(
        &host.teams.join("lost.git"),
        &[
            ("projects/Acme/memory.jsonl", ""),
            ("machines/someone-box/live.json", "{}"),
        ],
    );
    let plans = support::fixture("fixtures/host/applyPlans.json");
    let mut snapshot = support::fixture("fixtures/access/accessSnapshots.json")["base"].clone();
    support::merge_patch(&mut snapshot, &plans["snapshotPatch"]);
    support::merge_patch(
        &mut snapshot,
        &json!({"teams": {
            "syncteam": {"writable": true},
            "personal": {"repo": personal.to_str().expect("utf-8")},
        }}),
    );
    fs::write(
        &host.access,
        serde_json::to_vec_pretty(&snapshot).expect("encode"),
    )
    .expect("snapshot");
    host
}

fn run(args: &[&str], host: &Host, env: &[(&str, &str)], input: Option<&str>) -> Output {
    let mut command = Command::new(BINARY);
    command
        .args(args)
        .args([
            "--access",
            host.access.to_str().expect("utf-8"),
            "--teams",
            host.teams.to_str().expect("utf-8"),
        ])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("run the binary");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait")
}

fn apply(host: &Host) -> Output {
    let store_init = support::repo_root().join("infra/storeInit.sh");
    run(
        &[
            "access-apply",
            "--authorized-keys",
            host.keys.to_str().expect("utf-8"),
            "--store-init",
            store_init.to_str().expect("utf-8"),
        ],
        host,
        &[],
        None,
    )
}

fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read")).expect("JSON")
}

fn inode(path: &Path) -> u64 {
    fs::metadata(path).expect("metadata").ino()
}

#[test]
fn apply_makes_the_stores_and_the_keys_and_reports_them() {
    let host = host("apply");
    let lost = inode(&host.teams.join("lost.git"));

    let first = apply(&host);
    assert!(
        first.status.success(),
        "access-apply: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    for slug in ["otherteam", "syncteam", "vibebrains"] {
        let repo = host.teams.join(format!("{slug}.git"));
        assert_eq!(
            bare(&repo, &["ls-tree", "--name-only", "main"]),
            ".gitattributes",
            "{slug}: the first commit holds .gitattributes alone"
        );
        assert_eq!(
            vibememory_mcp::git_memories::run(
                &repo,
                &["cat-file", "blob", "main:.gitattributes"],
                None,
                &[]
            )
            .expect("blob"),
            vibememory_cli::install::GITATTRIBUTES.as_bytes(),
            "{slug}: the engine's text, byte for byte"
        );
        let hook = fs::read_to_string(repo.join("hooks/pre-receive")).expect("hook");
        assert!(
            hook.contains(&format!("exec {} pre-receive", layout::BINARY)),
            "{slug}: {hook}"
        );
        assert_eq!(bare(&repo, &["config", "receive.fsckObjects"]), "true");
    }
    assert!(!host.teams.join("oldteam.git").exists(), "renamed");
    assert!(host.teams.join("oldteam.deleted-2026-09-01.git").is_dir());
    assert_eq!(
        inode(&host.teams.join("lost.git")),
        lost,
        "the orphan is untouched"
    );

    let bytes = fs::read(&host.access).expect("snapshot");
    let snapshot = access::check(&bytes).expect("valid");
    assert_eq!(
        fs::read_to_string(&host.keys).expect("keys"),
        apply::authorized_keys(&snapshot, layout::BINARY)
    );
    assert_eq!(
        fs::metadata(&host.keys).expect("keys").mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(host.keys.parent().expect(".ssh"))
            .expect(".ssh")
            .mode()
            & 0o777,
        0o700
    );

    let applied = json_file(&layout::applied_file(&host.access));
    assert_eq!(applied["snapshotHash"], vibememory_cli::sha256::hex(&bytes));
    assert_eq!(applied["teamCount"], 5);
    assert_eq!(applied["problems"], json!([]));
    let report = json_file(&layout::report_file(&host.access));
    let teams: Vec<&str> = report["teams"]
        .as_object()
        .expect("teams")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(teams, ["otherteam", "personal", "syncteam", "vibebrains"]);
    assert_eq!(
        report["teams"]["personal"]["projects"],
        json!(["VibeMemory"])
    );
    assert_eq!(
        report["deleted"],
        json!([{"slug": "oldteam", "date": "2026-09-01", "sizeBytes": report["deleted"][0]["sizeBytes"]}])
    );
    assert_eq!(report["orphans"][0]["slug"], "lost");
    assert_eq!(report["orphans"][0]["projects"], json!(["Acme"]));
    assert_eq!(report["orphans"][0]["machines"], true);
    assert_eq!(report["applied"], applied);
}

#[test]
fn apply_again_changes_nothing_but_the_engines_text_and_refuses_a_broken_snapshot() {
    let host = host("again");
    assert!(apply(&host).status.success());
    let applied = json_file(&layout::applied_file(&host.access));

    // A second application changes nothing.
    let vibebrains = host.teams.join("vibebrains.git");
    let main = bare(&vibebrains, &["rev-parse", "main"]);
    assert!(apply(&host).status.success());
    assert_eq!(bare(&vibebrains, &["rev-parse", "main"]), main);

    // An older .gitattributes on main is brought to the engine's text, history kept.
    GitMemories::new(vibebrains.clone(), "test".to_owned())
        .settle_file(".gitattributes", b"* -text\n", "an older engine")
        .expect("older text");
    assert!(apply(&host).status.success());
    assert_eq!(bare(&vibebrains, &["rev-list", "--count", "main"]), "3");
    assert_eq!(
        bare(&vibebrains, &["cat-file", "blob", "main:.gitattributes"]),
        vibememory_cli::install::GITATTRIBUTES.trim_end()
    );

    // A snapshot that breaks a rule changes nothing and says so.
    let keys = fs::read(&host.keys).expect("keys");
    fs::write(&host.access, br#"{"version": 1}"#).expect("broken snapshot");
    assert!(!apply(&host).status.success());
    let rejected = json_file(&layout::applied_file(&host.access));
    assert_eq!(rejected["snapshotHash"], applied["snapshotHash"]);
    assert_eq!(rejected["problems"][0]["code"], "snapshotRejected");
    assert_eq!(fs::read(&host.keys).expect("keys"), keys, "keys untouched");
}

/// A stand-in for ssh: git runs it with the host and the command, and it runs the forced command
/// the way sshd would, with the command in `SSH_ORIGINAL_COMMAND`.
fn fake_ssh(host: &Host) -> PathBuf {
    let path = host.root.join("fake-ssh");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nfor last; do :; done\nSSH_ORIGINAL_COMMAND=\"$last\" exec {BINARY} shell \"$VM_KEY\" --access {} --teams {}\n",
            host.access.display(),
            host.teams.display()
        ),
    )
    .expect("fake ssh");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
    path
}

/// Git as a member's machine with `key`, over the forced command.
fn member_git(host: &Host, dir: &Path, key: &str, args: &[&str]) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_SSH_COMMAND", fake_ssh(host))
        .env("GIT_SSH_VARIANT", "simple")
        .env("VM_KEY", key)
        .env("GIT_AUTHOR_NAME", "alice")
        .env("GIT_AUTHOR_EMAIL", "alice@example.invalid")
        .env("GIT_COMMITTER_NAME", "alice")
        .env("GIT_COMMITTER_EMAIL", "alice@example.invalid")
        .output()
        .expect("run git")
}

#[test]
fn a_key_clones_and_pushes_only_what_the_rules_let_through() {
    let host = host("push");
    assert!(apply(&host).status.success());
    // The hook as installed calls the host's binary; here it calls this build, on this layout, and
    // keeps no disk reserve — the temporary directory's disk is not the host's.
    let sync = host.teams.join("syncteam.git");
    fs::write(
        sync.join("hooks/pre-receive"),
        format!(
            "#!/bin/sh\nexec {BINARY} pre-receive --access {} --reserve-bytes 0\n",
            host.access.display()
        ),
    )
    .expect("hook");

    let clone = member_git(
        &host,
        &host.root,
        ALICE_LAPTOP,
        &["clone", "--quiet", "vmhost:teams/syncteam.git", "work"],
    );
    assert!(
        clone.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    let work = host.root.join("work");
    assert!(work.join(".gitattributes").is_file());

    fs::create_dir_all(work.join("projects/Acme")).expect("project");
    fs::write(work.join("projects/Acme/s1.jsonl"), "{}\n").expect("transcript");
    fs::create_dir_all(work.join("machines/alice-laptop")).expect("machine");
    fs::write(work.join("machines/alice-laptop/live.json"), "{}\n").expect("live");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "a tick"]);
    let pushed = member_git(
        &host,
        &work,
        ALICE_LAPTOP,
        &["push", "--quiet", "origin", "main"],
    );
    assert!(
        pushed.status.success(),
        "push: {}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    assert_eq!(
        bare(&sync, &["rev-parse", "main"]),
        git(&work, &["rev-parse", "HEAD"])
    );

    fs::write(work.join(".gitattributes"), "* text\n").expect("edit");
    git(&work, &["commit", "--quiet", "-am", "edit the host's file"]);
    let refused = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", "main"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success());
    assert!(said.contains("remote: vibememory: pathDenied"), "{said}");
    assert!(said.contains("remote: .gitattributes"), "{said}");
    git(&work, &["reset", "--quiet", "--hard", "origin/main"]);

    for target in ["HEAD:refs/heads/side", "HEAD:refs/tags/v1"] {
        let refused = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", target]);
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(!refused.status.success(), "{target}");
        assert!(
            said.contains("remote: vibememory: refDenied"),
            "{target}: {said}"
        );
    }
    assert_eq!(
        bare(&sync, &["for-each-ref", "--format=%(refname)"]),
        "refs/heads/main"
    );

    let export = member_git(
        &host,
        &host.root,
        ALICE_LAPTOP,
        &["clone", "--quiet", "vmhost:teams/vibebrains.git", "export"],
    );
    let said = String::from_utf8_lossy(&export.stderr);
    assert!(!export.status.success());
    assert!(said.contains("vibememory: exportDenied"), "{said}");

    let memory_push = member_git(
        &host,
        &work,
        ALICE_LAPTOP,
        &["push", "vmhost:teams/vibebrains.git", "HEAD:main"],
    );
    let said = String::from_utf8_lossy(&memory_push.stderr);
    assert!(!memory_push.status.success());
    assert!(said.contains("vibememory: pushDenied"), "{said}");

    let shell = run(
        &["shell", ALICE_LAPTOP],
        &host,
        &[("SSH_ORIGINAL_COMMAND", "bash -c id")],
        None,
    );
    assert_eq!(shell.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&shell.stderr).starts_with("vibememory: commandDenied\n"),
        "{}",
        String::from_utf8_lossy(&shell.stderr)
    );
}

#[test]
fn a_key_reaches_its_teams_memory_and_its_status() {
    let host = host("mcp");
    assert!(apply(&host).status.success());

    let session = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_save","arguments":{"project":"VibeIDE","id":"over-ssh","kind":"project","description":"d","body":"b"}}}"#,
    ]
    .join("\n");
    let served = run(
        &["shell", ALICE_LAPTOP],
        &host,
        &[(
            "SSH_ORIGINAL_COMMAND",
            "mcp --team vibebrains --agent test-agent",
        )],
        Some(&session),
    );
    assert!(
        served.status.success(),
        "{}",
        String::from_utf8_lossy(&served.stderr)
    );
    let answers: Vec<Value> = String::from_utf8_lossy(&served.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[1]["result"]["isError"], false, "{}", answers[1]);
    let journal = bare(
        &host.teams.join("vibebrains.git"),
        &["cat-file", "blob", "main:projects/VibeIDE/memory.jsonl"],
    );
    assert!(journal.contains("\"alice-laptop-"), "{journal}");
    assert!(journal.contains("\"member\":\"alice\""), "{journal}");
    assert!(journal.contains("\"agent\":\"test-agent\""), "{journal}");

    let status = run(
        &["shell", ALICE_LAPTOP],
        &host,
        &[("SSH_ORIGINAL_COMMAND", "status")],
        None,
    );
    assert!(status.status.success());
    let answer: Value = serde_json::from_slice(&status.stdout).expect("JSON");
    assert_eq!(answer["key"]["id"], ALICE_LAPTOP);
    assert_eq!(answer["storeNames"], json!(["alice-laptop"]));
    let teams: Vec<&str> = answer["teams"]
        .as_object()
        .expect("teams")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(teams, ["syncteam", "vibebrains"], "only the key's teams");

    fs::remove_file(layout::report_file(&host.access)).expect("remove host.json");
    let unavailable = run(
        &["shell", ALICE_LAPTOP],
        &host,
        &[("SSH_ORIGINAL_COMMAND", "status")],
        None,
    );
    assert_eq!(unavailable.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&unavailable.stderr).starts_with("vibememory: statusUnavailable\n")
    );
}
