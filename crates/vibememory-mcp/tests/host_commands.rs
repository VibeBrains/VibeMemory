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
    bare_with(
        &personal,
        &[
            ("projects/VibeMemory/memory.jsonl", "{}\n"),
            ("projects/VibeMemory/memory/MEMORY.md", "m\n"),
            (
                "projects/VibeMemory/11111111-1111-4111-8111-111111111111.jsonl",
                "{}\n{}\n",
            ),
        ],
    );
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
    apply_with(host, &[])
}

fn apply_with(host: &Host, extra: &[&str]) -> Output {
    let store_init = support::repo_root().join("infra/storeInit.sh");
    let mut args = vec![
        "access-apply",
        "--authorized-keys",
        host.keys.to_str().expect("utf-8"),
        "--store-init",
        store_init.to_str().expect("utf-8"),
    ];
    args.extend_from_slice(extra);
    run(&args, host, &[], None)
}

fn json_file(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read")).expect("JSON")
}

fn inode(path: &Path) -> u64 {
    fs::metadata(path).expect("metadata").ino()
}

/// What the owner reads a project by: its bytes in `main` — memory apart from sessions — when it
/// was last written and by whom
fn project_facts_split(project: &serde_json::Value) {
    assert_eq!(
        project["sizeBytes"], 11,
        "the bytes of the journal, the memory file and the transcript: {project}"
    );
    assert_eq!(
        project["memoryBytes"], 5,
        "the journal and the memory file, not the transcript: {project}"
    );
    assert!(
        project["lastCommitAt"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z')),
        "{project}"
    );
    assert!(
        project["lastAuthor"]
            .as_str()
            .is_some_and(|author| !author.is_empty()),
        "{project}"
    );
    assert!(
        project.get("lastAgent").is_none(),
        "a commit not signed by an engine names no agent: {project}"
    );
}

#[test]
fn an_engine_commit_names_its_agent_and_nothing_else_does() {
    use vibememory_mcp::status::engine_agent;
    assert_eq!(
        engine_agent("claude-code@vibememory.invalid").as_deref(),
        Some("claude-code")
    );
    assert_eq!(
        engine_agent("dsh-desktop@vibememory.invalid").as_deref(),
        Some("dsh-desktop")
    );
    for other in [
        "test@example.invalid",
        "vibememory-mcp@host-tk_7a9b2c4d.invalid",
        "Claude@vibememory.invalid",
        "@vibememory.invalid",
        "claude-code@vibememory.invalid.evil",
    ] {
        assert_eq!(engine_agent(other), None, "{other}");
    }
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
    project_facts_split(&report["teams"]["personal"]["projectFacts"]["VibeMemory"]);
    // the quota counts the files of `main`: the journal, the memory file and the transcript
    assert_eq!(report["teams"]["personal"]["treeBytes"], 11);
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

/// The snapshot on disk replaced by `snapshot`, pretty, as the cabinet writes it.
fn put_snapshot(host: &Host, snapshot: &Value) {
    fs::write(
        &host.access,
        serde_json::to_vec_pretty(snapshot).expect("encode"),
    )
    .expect("snapshot");
}

/// What the forced command answers Alice's laptop for `status`: the exit code and the first line
/// of stderr.
fn laptop_status(host: &Host) -> (Option<i32>, String) {
    let status = run(
        &["shell", ALICE_LAPTOP],
        host,
        &[("SSH_ORIGINAL_COMMAND", "status")],
        None,
    );
    let stderr = String::from_utf8_lossy(&status.stderr);
    (
        status.status.code(),
        stderr.lines().next().unwrap_or_default().to_owned(),
    )
}

/// The memory teams the nightly backup would bundle.
fn backup_teams(host: &Host) -> String {
    let listed = Command::new(BINARY)
        .args(["access", "teams"])
        .arg(&host.access)
        .args(["--mode", "memory"])
        .output()
        .expect("run the binary");
    assert!(listed.status.success());
    String::from_utf8_lossy(&listed.stdout).trim().to_owned()
}

#[test]
fn an_older_snapshot_is_refused_and_the_one_applied_stays_in_force() {
    let host = host("serial");
    assert!(apply(&host).status.success());
    let bytes = fs::read(&host.access).expect("snapshot");
    let copy = layout::applied_snapshot_file(&host.access);
    assert_eq!(
        fs::read(&copy).expect("copy"),
        bytes,
        "the copy is what was applied"
    );
    assert_eq!(fs::metadata(&copy).expect("copy").mode() & 0o777, 0o640);
    let applied = json_file(&layout::applied_file(&host.access));
    assert_eq!(applied["serial"], 12);
    assert_eq!(backup_teams(&host), "vibebrains");

    // A publication that lost its turn lands late: serial 11, without the memory team and without
    // Alice's laptop.
    let keys = fs::read(&host.keys).expect("keys");
    let mut stale: Value = serde_json::from_slice(&bytes).expect("JSON");
    stale["serial"] = json!(11);
    stale["teams"].as_object_mut().unwrap().remove("vibebrains");
    stale["tokens"]
        .as_array_mut()
        .unwrap()
        .retain(|token| token["team"] != "vibebrains");
    stale["keys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["id"] != ALICE_LAPTOP);
    put_snapshot(&host, &stale);
    assert!(!apply(&host).status.success());
    let refused = json_file(&layout::applied_file(&host.access));
    assert_eq!(refused["problems"][0]["code"], "serialBehind");
    assert_eq!(refused["serial"], 12);
    assert_eq!(refused["snapshotHash"], applied["snapshotHash"]);
    assert_eq!(fs::read(&host.keys).expect("keys"), keys, "keys untouched");
    assert_eq!(fs::read(&copy).expect("copy"), bytes, "the copy stays");
    assert!(
        host.teams.join("vibebrains.git").is_dir(),
        "the store stays"
    );

    // Everything on the host goes by the snapshot applied, not by the stale file.
    assert_eq!(laptop_status(&host).0, Some(0), "the key still gets in");
    assert_eq!(
        backup_teams(&host),
        "vibebrains",
        "and its team is backed up"
    );
    let report = json_file(&layout::report_file(&host.access));
    assert!(report["teams"].get("vibebrains").is_some(), "{report}");

    // Two decisions under one serial: the host keeps the one it took, and only the latest refusal
    // is listed.
    stale["serial"] = json!(12);
    put_snapshot(&host, &stale);
    assert!(!apply(&host).status.success());
    let problems = json_file(&layout::applied_file(&host.access))["problems"].clone();
    assert_eq!(problems[0]["code"], "serialBehind");
    assert_eq!(
        problems
            .as_array()
            .expect("problems")
            .iter()
            .filter(|problem| problem["code"] == "serialBehind")
            .count(),
        1
    );

    // A newer one is applied, and the refusal is gone.
    stale["serial"] = json!(13);
    put_snapshot(&host, &stale);
    assert!(apply(&host).status.success());
    let applied = json_file(&layout::applied_file(&host.access));
    assert_eq!(applied["serial"], 13);
    assert_eq!(applied["problems"], json!([]));
    assert_eq!(
        fs::read(&copy).expect("copy"),
        fs::read(&host.access).expect("snapshot")
    );
    assert_eq!(
        laptop_status(&host),
        (Some(1), "vibememory: unknownKey".to_owned())
    );
    assert_eq!(backup_teams(&host), "");

    // A banned member's key is refused even by a snapshot that lists it.
    let mut banned: Value = serde_json::from_slice(&bytes).expect("JSON");
    banned["serial"] = json!(14);
    banned["bans"] = json!([{"member": "alice", "until": null}]);
    put_snapshot(&host, &banned);
    assert!(apply(&host).status.success());
    assert_eq!(
        laptop_status(&host),
        (Some(1), "vibememory: unknownKey".to_owned())
    );
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

/// The applied host's `syncteam` store with its hook calling this build, and Alice's laptop's clone
/// of it; the store and the clone.
fn alice_clone(host: &Host) -> (PathBuf, PathBuf) {
    assert!(apply(host).status.success());
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
        host,
        &host.root,
        ALICE_LAPTOP,
        &["clone", "--quiet", "vmhost:teams/syncteam.git", "work"],
    );
    assert!(
        clone.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    (sync, host.root.join("work"))
}

/// Alice's laptop writes `files` in its clone, commits them and pushes `main`.
fn tick(host: &Host, work: &Path, files: &[&str]) -> Output {
    for path in files {
        let path = work.join(path);
        fs::create_dir_all(path.parent().expect("a directory")).expect("directory");
        fs::write(path, "{}\n").expect("file");
    }
    git(work, &["add", "-A"]);
    git(work, &["commit", "--quiet", "-m", "a tick"]);
    member_git(
        host,
        work,
        ALICE_LAPTOP,
        &["push", "--quiet", "origin", "main"],
    )
}

#[test]
fn a_key_clones_and_pushes_only_what_the_rules_let_through() {
    let host = host("push");
    let (sync, work) = alice_clone(&host);
    assert!(work.join(".gitattributes").is_file());

    let pushed = tick(
        &host,
        &work,
        &["projects/Acme/s1.jsonl", "machines/alice-laptop/live.json"],
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
fn a_push_over_a_stale_file_goes_by_the_snapshot_applied() {
    let host = host("stale-push");
    let (_, work) = alice_clone(&host);

    // A file without the laptop's key lands after the host applied a newer one: the forced command
    // and pre-receive both go by the snapshot applied, and the push lands.
    let mut stale: Value =
        serde_json::from_slice(&fs::read(&host.access).expect("snapshot")).expect("JSON");
    stale["serial"] = json!(11);
    stale["keys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["id"] != ALICE_LAPTOP);
    put_snapshot(&host, &stale);
    let pushed = tick(&host, &work, &["projects/Acme/s2.jsonl"]);
    assert!(
        pushed.status.success(),
        "push over a stale file: {}",
        String::from_utf8_lossy(&pushed.stderr)
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

#[test]
fn an_open_session_ends_with_the_first_request_after_the_key_is_revoked() {
    use std::io::{BufRead as _, BufReader};

    let host = host("session");
    assert!(apply(&host).status.success());
    let mut child = Command::new(BINARY)
        .args(["shell", ALICE_LAPTOP])
        .args([
            "--access",
            host.access.to_str().expect("utf-8"),
            "--teams",
            host.teams.to_str().expect("utf-8"),
        ])
        .env(
            "SSH_ORIGINAL_COMMAND",
            "mcp --team vibebrains --agent test-agent",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run the binary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut answers = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let mut ask = |line: &str| -> Value {
        writeln!(stdin, "{line}").expect("write");
        stdin.flush().expect("flush");
        serde_json::from_str(&answers.next().expect("an answer").expect("read")).expect("JSON")
    };
    let search = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x"}}}"#;
    assert!(
        ask(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)["result"].is_object()
    );
    assert_eq!(ask(search)["result"]["isError"], false);

    // The cabinet revokes the laptop's key while the session is open.
    let mut revoked: Value =
        serde_json::from_slice(&fs::read(&host.access).expect("snapshot")).expect("JSON");
    revoked["serial"] = json!(13);
    revoked["keys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["id"] != ALICE_LAPTOP);
    put_snapshot(&host, &revoked);
    let ended = ask(search);
    assert_eq!(ended["error"]["code"], -32001, "{ended}");
    assert!(
        ended["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not in the snapshot any more"),
        "{ended}"
    );
    let status = child.wait().expect("wait");
    assert!(status.success(), "the session ends on its own");
}

#[test]
fn a_run_that_cannot_list_the_stores_applies_nothing_and_the_timer_catches_up() {
    let host = host("catch-up");
    assert!(apply(&host).status.success());
    let copy = layout::applied_snapshot_file(&host.access);
    let before = fs::read(&copy).expect("copy");

    // A newer snapshot while the team stores cannot be listed: nothing applied, and it says so.
    let mut newer: Value =
        serde_json::from_slice(&fs::read(&host.access).expect("snapshot")).expect("JSON");
    newer["serial"] = json!(13);
    newer["keys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["id"] != ALICE_LAPTOP);
    put_snapshot(&host, &newer);
    fs::set_permissions(&host.teams, fs::Permissions::from_mode(0o000)).expect("chmod");
    let failed = apply_with(&host, &["--catch-up"]);
    fs::set_permissions(&host.teams, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(!failed.status.success());
    let applied = json_file(&layout::applied_file(&host.access));
    assert_eq!(applied["serial"], 12);
    assert_eq!(applied["problems"][0]["code"], "applyFailed");
    assert_eq!(
        fs::read(&copy).expect("copy"),
        before,
        "no copy of what was not applied"
    );

    // The timer's next run applies it; the one after finds nothing to do.
    assert!(apply_with(&host, &["--catch-up"]).status.success());
    assert_eq!(json_file(&layout::applied_file(&host.access))["serial"], 13);
    let stamp = fs::metadata(layout::applied_file(&host.access))
        .expect("applied")
        .modified()
        .expect("mtime");
    assert!(apply_with(&host, &["--catch-up"]).status.success());
    assert_eq!(
        fs::metadata(layout::applied_file(&host.access))
            .expect("applied")
            .modified()
            .expect("mtime"),
        stamp,
        "caught up: nothing rewritten"
    );

    // Without a copy, applied.json still keeps the host from going back.
    fs::remove_file(&copy).expect("remove the copy");
    newer["serial"] = json!(12);
    put_snapshot(&host, &newer);
    assert!(!apply(&host).status.success());
    let refused = json_file(&layout::applied_file(&host.access));
    assert_eq!(refused["problems"][0]["code"], "serialBehind");
    assert_eq!(refused["serial"], 13);
    // Nor does the door open by that older file: the key is refused, not let in by serial 12.
    let why = vibememory_mcp::hostops::read_in_force(&host.access, |_| {})
        .expect_err("serial 13 was applied and has no copy");
    assert!(why.contains("serial 13 was applied"), "{why}");
    // The timer does not refuse the same bytes again every quarter of an hour.
    assert_eq!(refused["lastRefused"]["code"], "serialBehind");
    let stamp = fs::metadata(layout::applied_file(&host.access))
        .expect("applied")
        .modified()
        .expect("mtime");
    assert!(apply_with(&host, &["--catch-up"]).status.success());
    assert_eq!(
        fs::metadata(layout::applied_file(&host.access))
            .expect("applied")
            .modified()
            .expect("mtime"),
        stamp,
        "refused already: nothing rewritten"
    );
}

#[test]
fn a_run_that_cannot_list_the_stores_keeps_what_failed_for_each_team() {
    let host = host("team-failures");
    // syncteam's directory is taken by a file: its store cannot be set up
    fs::write(host.teams.join("syncteam.git"), "not a repository").expect("write");
    assert!(!apply(&host).status.success());
    let first = json_file(&layout::applied_file(&host.access));
    let team_failed = |applied: &Value| {
        applied["problems"]
            .as_array()
            .expect("problems")
            .iter()
            .any(|problem| problem["code"] == "applyFailed" && problem["team"] == "syncteam")
    };
    assert!(team_failed(&first), "{first}");

    let mut newer: Value =
        serde_json::from_slice(&fs::read(&host.access).expect("snapshot")).expect("JSON");
    newer["serial"] = json!(13);
    put_snapshot(&host, &newer);
    fs::set_permissions(&host.teams, fs::Permissions::from_mode(0o000)).expect("chmod");
    let failed = apply(&host);
    fs::set_permissions(&host.teams, fs::Permissions::from_mode(0o755)).expect("chmod");
    assert!(!failed.status.success());
    let after = json_file(&layout::applied_file(&host.access));
    assert_eq!(after["problems"][0]["code"], "applyFailed");
    assert!(after["problems"][0].get("team").is_none());
    assert!(
        team_failed(&after),
        "what failed for syncteam stays: {after}"
    );
}

/// A transcript line with an agent token in it, from the fixture of such lines.
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
fn a_push_of_a_file_holding_an_agent_token_is_refused_whole() {
    let host = host("token-push");
    let (sync, work) = alice_clone(&host);
    let before = bare(&sync, &["rev-parse", "main"]);
    let line = token_line();
    let transcript = "projects/Acme/11111111-1111-4111-8111-111111111111.jsonl";
    fs::create_dir_all(work.join("projects/Acme")).expect("dirs");
    fs::write(work.join(transcript), line).expect("write");
    fs::write(work.join("projects/Acme/notes.txt"), "no token here\n").expect("write");
    git(&work, &["add", "-A"]);
    git(
        &work,
        &["commit", "--quiet", "-m", "a tick of an old engine"],
    );
    let refused = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", "main"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains("remote: vibememory: tokenInPush"), "{said}");
    assert!(said.contains(&format!("remote: {transcript}")), "{said}");
    assert!(
        !said.contains("notes.txt"),
        "only the files that hold one: {said}"
    );
    assert_eq!(
        bare(&sync, &["rev-parse", "main"]),
        before,
        "nothing landed"
    );
}

#[test]
fn a_token_the_push_adds_and_removes_again_is_still_refused() {
    let host = host("token-history");
    let (sync, work) = alice_clone(&host);
    let before = bare(&sync, &["rev-parse", "main"]);
    let transcript = "projects/Acme/11111111-1111-4111-8111-111111111111.jsonl";
    fs::create_dir_all(work.join("projects/Acme")).expect("dirs");
    fs::write(work.join(transcript), token_line()).expect("write");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "the token goes in"]);
    fs::write(work.join(transcript), "{}\n").expect("write");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "and out again"]);
    // The tree the push leaves is clean; its history is not.
    let refused = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", "main"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains("remote: vibememory: tokenInPush"), "{said}");
    assert!(said.contains(&format!("remote: {transcript}")), "{said}");
    assert_eq!(
        bare(&sync, &["rev-parse", "main"]),
        before,
        "nothing landed"
    );
}

#[test]
fn a_token_in_a_commit_message_is_refused() {
    let host = host("token-message");
    let (sync, work) = alice_clone(&host);
    let before = bare(&sync, &["rev-parse", "main"]);
    fs::create_dir_all(work.join("projects/Acme")).expect("dirs");
    fs::write(work.join("projects/Acme/notes.txt"), "a note\n").expect("write");
    git(&work, &["add", "-A"]);
    let token_text = token_line();
    git(&work, &["commit", "--quiet", "-m", &token_text]);
    let refused = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", "main"]);
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{said}");
    assert!(said.contains("remote: vibememory: tokenInPush"), "{said}");
    assert!(said.contains("remote: commit "), "{said}");
    assert_eq!(
        bare(&sync, &["rev-parse", "main"]),
        before,
        "nothing landed"
    );
}

#[test]
fn a_push_of_many_files_is_read_to_the_end() {
    let host = host("many-files");
    let (sync, work) = alice_clone(&host);
    // More answers than a pipe holds: a hook that wrote every question before reading an answer
    // would wait for git forever, and git for it.
    let dir = work.join("projects/Acme/many");
    fs::create_dir_all(&dir).expect("dirs");
    for index in 0..3000 {
        fs::write(
            dir.join(format!("{index:04}.txt")),
            format!("file {index}\n"),
        )
        .expect("write");
    }
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "many files"]);
    let pushed = member_git(&host, &work, ALICE_LAPTOP, &["push", "origin", "main"]);
    assert!(
        pushed.status.success(),
        "{}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    assert_eq!(
        bare(&sync, &["rev-parse", "main"]),
        git(&work, &["rev-parse", "HEAD"]).trim()
    );
}

#[test]
fn an_application_waits_for_the_one_under_way() {
    let host = host("apply-lock");
    assert!(apply(&host).status.success());
    // Another application holds the lock: the timer's run starts and waits, even with nothing to do.
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open({
            let file = layout::apply_lock_file(&host.teams);
            fs::create_dir_all(file.parent().expect("locks dir")).expect("locks dir");
            file
        })
        .expect("lock file");
    lock.lock().expect("lock");
    let store_init = support::repo_root().join("infra/storeInit.sh");
    let mut child = Command::new(BINARY)
        .args([
            "access-apply",
            "--catch-up",
            "--authorized-keys",
            host.keys.to_str().expect("utf-8"),
            "--store-init",
            store_init.to_str().expect("utf-8"),
            "--access",
            host.access.to_str().expect("utf-8"),
            "--teams",
            host.teams.to_str().expect("utf-8"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run the binary");
    // caught up, a run that did not wait would be over within milliseconds
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert!(
        child.try_wait().expect("try wait").is_none(),
        "it waits while the other one runs"
    );
    lock.unlock().expect("unlock");
    assert!(child.wait().expect("wait").success());
}

/// The journal of the memory fixtures' `oneRecordProjected`: one record, as the store keeps it.
fn fixture_journal() -> String {
    let cases = support::fixture("fixtures/memory/memoryScenarios.json");
    let case = cases["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|case| case["id"] == "oneRecordProjected")
        .expect("oneRecordProjected");
    case["journal"]
        .as_array()
        .expect("journal")
        .iter()
        .fold(String::new(), |mut journal, line| {
            journal.push_str(line.as_str().expect("line"));
            journal.push('\n');
            journal
        })
}

#[test]
fn the_report_says_when_a_team_was_last_reached() {
    let host = host("activity");
    let activity = layout::activity_dir(&host.teams);
    fs::create_dir_all(&activity).expect("activity");
    fs::write(activity.join("vibebrains"), b"").expect("touch");
    assert!(apply(&host).status.success());
    let report = json_file(&layout::report_file(&host.access));
    assert!(
        report["teams"]["vibebrains"]["lastAccessAt"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z')),
        "{report}"
    );
    // never reached: the key is left out rather than guessed
    assert!(
        report["teams"]["syncteam"].get("lastAccessAt").is_none(),
        "{report}"
    );
}

#[test]
fn an_archive_holds_the_journal_and_the_memory_as_files_and_waits_for_the_cabinet() {
    let host = host("export");
    let journal = fixture_journal();
    bare_with(
        &host.teams.join("acme.git"),
        &[("projects/VibeMemory/memory.jsonl", &journal)],
    );
    let exports = layout::exports_dir(&host.teams);
    fs::create_dir_all(&exports).expect("exports");
    let id = "0f8c2a9e-4b1d-4c7e-9a3f-5d6e7f8a9b0c";
    fs::write(exports.join(format!("{id}.request")), br#"{"slug":"acme"}"#).expect("request");
    // a name that is no request id is not the cabinet's, and is left alone
    fs::write(exports.join("not-an-id.request"), br#"{"slug":"acme"}"#).expect("stray");

    let output = run(&["export"], &host, &[], None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = exports.join(format!("{id}.zip"));
    assert_eq!(
        fs::metadata(&path).expect("archive").permissions().mode() & 0o777,
        0o640,
        "the cabinet reads it through the shared group"
    );
    let mut zip = zip::ZipArchive::new(fs::File::open(&path).expect("open")).expect("zip");
    let mut read = |name: &str| {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut zip.by_name(name).expect(name), &mut text).expect(name);
        text
    };
    assert_eq!(
        read("VibeMemory/memory.jsonl"),
        journal,
        "the journal goes back into a store unchanged"
    );
    assert!(read("VibeMemory/memory/MEMORY.md").contains("store-naming.md"));
    assert!(read("VibeMemory/memory/store-naming.md").contains("never the worktree"));
    assert!(read("README.txt").contains("acme"));
    assert!(!exports.join("not-an-id.zip").exists());
    // the request is the cabinet's file: the host leaves it for the cabinet to take away
    assert!(exports.join(format!("{id}.request")).exists());

    // a team with no live store gets no archive, and the run says it failed
    let ghost = "1a2b3c4d-0000-4000-8000-000000000001";
    fs::write(
        exports.join(format!("{ghost}.request")),
        br#"{"slug":"ghost"}"#,
    )
    .expect("ghost");
    assert!(!run(&["export"], &host, &[], None).status.success());
    assert!(!exports.join(format!("{ghost}.zip")).exists());
}

/// A team whose sessions were switched off keeps its memory alone: the transcripts and the
/// machines' directories leave `main` and its history, the generation rises once, and applying the
/// same snapshot again changes nothing.
#[test]
fn switching_sessions_off_leaves_the_memory_and_frees_the_rest() {
    let host = host("purge");
    let repo = host.teams.join("vibebrains.git");
    bare_with(
        &repo,
        &[
            (".gitattributes", "* merge=union\n"),
            ("projects/App/memory.jsonl", "{\"id\":\"m1\"}\n"),
            ("projects/App/memory/MEMORY.md", "index\n"),
            (
                "projects/App/11111111-1111-4111-8111-111111111111.jsonl",
                "a session nobody needs any more\n",
            ),
            ("machines/alice-laptop/live.json", "{}"),
        ],
    );
    let session = bare(
        &repo,
        &[
            "rev-parse",
            "main:projects/App/11111111-1111-4111-8111-111111111111.jsonl",
        ],
    );

    let applied = apply(&host);
    assert!(
        applied.status.success(),
        "access-apply: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let tree = bare(&repo, &["ls-tree", "-r", "--name-only", "main"]);
    assert_eq!(
        tree.lines().collect::<Vec<_>>(),
        [
            ".gitattributes",
            ".vibememory-generation",
            "projects/App/memory.jsonl",
            "projects/App/memory/MEMORY.md"
        ]
    );
    assert_eq!(
        bare(&repo, &["cat-file", "blob", "main:.vibememory-generation"]),
        "1"
    );
    assert_eq!(
        bare(&repo, &["rev-list", "--count", "main"]),
        "1",
        "no history behind it"
    );
    let gone = Command::new("git")
        .args([
            "--git-dir",
            repo.to_str().expect("utf-8"),
            "cat-file",
            "-e",
            &session,
        ])
        .status()
        .expect("git");
    assert!(
        !gone.success(),
        "the session's object is gone from the store"
    );

    let head = bare(&repo, &["rev-parse", "main"]);
    assert!(apply(&host).status.success());
    assert_eq!(
        bare(&repo, &["rev-parse", "main"]),
        head,
        "a second application changes nothing"
    );
}

/// The owner's new machine: a machine key that names the personal store.
const OWNER_GPD: &str = "mk_9tjsmx2j";

#[test]
fn the_owners_machine_key_clones_and_pushes_the_personal_store_where_it_lies() {
    let host = host("personal-key");
    let mut snapshot: Value =
        serde_json::from_slice(&fs::read(&host.access).expect("snapshot")).expect("json");
    let mut keys = snapshot["keys"].as_array().cloned().unwrap_or_default();
    keys.push(json!({
        "id": OWNER_GPD, "member": "borodatych", "machine": "gpd", "storeName": "borodatych-gpd",
        "teams": ["personal"],
        "publicKey": "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIERERERERERERERERERERERERERERERERERERERERERE"
    }));
    snapshot["keys"] = Value::Array(keys);
    put_snapshot(&host, &snapshot);
    assert!(apply(&host).status.success());
    let personal = host.root.join("personal.git");

    let clone = member_git(
        &host,
        &host.root,
        OWNER_GPD,
        &["clone", "--quiet", "vmhost:teams/personal.git", "gpd"],
    );
    assert!(
        clone.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&clone.stderr)
    );
    let work = host.root.join("gpd");
    assert!(
        work.join("projects/VibeMemory/memory/MEMORY.md").is_file(),
        "the personal store, not a team's"
    );

    fs::create_dir_all(work.join("machines/GPD-WIN-MAX2")).expect("dir");
    fs::write(work.join("machines/GPD-WIN-MAX2/live.json"), "{}\n").expect("file");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "gpd"]);
    let pushed = member_git(
        &host,
        &work,
        OWNER_GPD,
        &["push", "--quiet", "origin", "HEAD:main"],
    );
    assert!(
        pushed.status.success(),
        "push: {}",
        String::from_utf8_lossy(&pushed.stderr)
    );
    assert_eq!(
        bare(&personal, &["rev-parse", "main"]),
        git(&work, &["rev-parse", "HEAD"])
    );

    // Alice's laptop key does not name personal: the same path is closed to it
    let refused = member_git(
        &host,
        &host.root,
        ALICE_LAPTOP,
        &["clone", "vmhost:teams/personal.git", "alice"],
    );
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("unknownTeam"));
}

#[test]
fn a_local_server_refuses_an_agent_name_at_start_and_notes_a_good_one() {
    let engine = std::env::temp_dir().join(format!("vibememory-mcp-agent-{}", std::process::id()));
    let _ = fs::remove_dir_all(&engine);
    fs::create_dir_all(&engine).unwrap();
    let run = |agent: &str| {
        Command::new(BINARY)
            .args(["--agent", agent])
            .env("VIBEMEMORY_DIR", &engine)
            .current_dir(&engine)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };

    let refused = run("DSH-Desktop");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("is not a name"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !engine.join("clients").exists(),
        "a refused name is not noted"
    );

    let _ = run("dsh-desktop");
    let noted = fs::read_to_string(engine.join("clients/dsh-desktop")).unwrap();
    // When it started and which build it was: by the build, doctor tells a server older than the engine
    let (stamp, version) = noted.trim_end().split_once(' ').unwrap();
    assert!(stamp.ends_with('Z'), "{noted}");
    assert_eq!(version, env!("CARGO_PKG_VERSION"), "{noted}");
    fs::remove_dir_all(&engine).unwrap();
}
