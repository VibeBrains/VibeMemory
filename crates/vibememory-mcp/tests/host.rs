//! The server on the store's host, against a real snapshot file and real bare repositories: who
//! the snapshot lets in, in whose name they write, and what changes when the file does.

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
use std::process::Command;

use serde_json::{Value, json};
use vibememory_mcp::host::Host;
use vibememory_mcp::http::{HttpRequest, Note, answer};

/// The owner's token from before the cabinet: no prefix, compared as a whole.
const LEGACY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ALICE: &str = "vmt_7q2m9x4a_alice-writes-to-vibebrains";
const READER: &str = "vmt_b3c4d5e6_alice-only-reads";
const EXPIRED: &str = "vmt_c4d5e6f7_alice-was-let-in-once";

/// A directory of this test, removed at the end.
struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vibememory-host-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp");
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A bare repository whose `main` holds the given files, as a store or a team repository would.
fn bare_with(temp: &Temp, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let bare = temp.0.join(name);
    let work = temp.0.join(format!("{name}.work"));
    fs::create_dir_all(&work).expect("work");
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    for (path, text) in files {
        let path = work.join(path);
        fs::create_dir_all(path.parent().unwrap()).expect("dir");
        fs::write(path, text).expect("file");
    }
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "--quiet", "-m", "start"]);
    git(
        &temp.0,
        &[
            "clone",
            "--quiet",
            "--bare",
            work.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    bare
}

fn digest(token: &str) -> String {
    vibememory_cli::sha256::hex(token.as_bytes())
}

/// The snapshot of these tests: the owner's personal store and one memory team with Alice in it.
fn snapshot(personal: &Path) -> Value {
    json!({
        "version": 1,
        "teamCount": 2,
        "teams": {
            "personal": {
                "adopted": true,
                "repo": personal.to_str().unwrap(),
                "writable": true,
                "limits": { "maxRecords": 5000, "maxRecordBytes": 65536 },
                "members": { "borodatych": "owner" }
            },
            "vibebrains": {
                "mode": "memory",
                "writable": true,
                "projects": ["VibeIDE"],
                "limits": { "maxRecords": 2, "maxRecordBytes": 2048, "quotaBytes": 1_073_741_824, "seats": 5 },
                "members": { "borodatych": "owner", "alice": "member" }
            }
        },
        "tokens": [
            { "id": "tk_legacy", "legacy": true, "sha256": digest(LEGACY), "team": "personal",
              "member": "borodatych", "agent": "claude-code", "role": "writer", "history": true,
              "projects": null, "expiresAt": null },
            { "id": "tk_7q2m9x4a", "sha256": digest(ALICE), "team": "vibebrains", "member": "alice",
              "agent": "vibeide", "role": "writer", "history": false, "projects": null, "expiresAt": null },
            { "id": "tk_b3c4d5e6", "sha256": digest(READER), "team": "vibebrains", "member": "alice",
              "agent": "vibeide", "role": "reader", "history": false, "projects": null, "expiresAt": null },
            { "id": "tk_c4d5e6f7", "sha256": digest(EXPIRED), "team": "vibebrains", "member": "alice",
              "agent": "vibeide", "role": "writer", "history": false, "projects": null,
              "expiresAt": "2001-01-01T00:00:00Z" }
        ],
        "keys": []
    })
}

/// Writes a snapshot the way the cabinet does: a new file beside it, renamed over it.
fn publish(path: &Path, text: &str) {
    let fresh = path.with_extension("json.new");
    fs::write(&fresh, text).expect("snapshot");
    fs::rename(&fresh, path).expect("rename");
}

/// A host with the personal store and the team's repository, and the snapshot on disk.
struct Stand {
    temp: Temp,
    personal: PathBuf,
    access: PathBuf,
    host: Host,
}

fn stand(label: &str, cabinet: Option<&str>) -> Stand {
    let temp = Temp::new(label);
    let personal = bare_with(
        &temp,
        "store.git",
        &[(
            "projects/Probe/older.jsonl",
            "{\"type\":\"user\",\"timestamp\":\"2026-09-01T10:00:00Z\",\"message\":{\"content\":\"old words\"}}\n",
        )],
    );
    let teams = temp.0.join("teams");
    fs::create_dir_all(&teams).expect("teams");
    bare_with(
        &temp,
        "teams/vibebrains.git",
        &[(".gitattributes", "*.jsonl merge=vibememory-jsonl\n")],
    );
    let access = temp.0.join("access.json");
    publish(&access, &snapshot(&personal).to_string());
    let host = Host::open(access.clone(), teams, cabinet.map(str::to_owned)).expect("open");
    Stand {
        temp,
        personal,
        access,
        host,
    }
}

fn request(token: &str, method: &str, params: &Value) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/mcp".into(),
        headers: vec![("authorization".into(), format!("Bearer {token}"))],
        body: json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
            .to_string()
            .into_bytes(),
    }
}

/// Calls a tool; the structured answer, and whether it was a refusal.
fn call(stand: &Stand, token: &str, tool: &str, arguments: &Value) -> (Value, bool, Note) {
    let (response, note) = answer(
        &request(
            token,
            "tools/call",
            &json!({ "name": tool, "arguments": arguments }),
        ),
        &stand.host,
    );
    assert_eq!(
        response.status,
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    let body: Value = serde_json::from_slice(&response.body).expect("json");
    let result = &body["result"];
    (
        result["structuredContent"].clone(),
        result["isError"] == true,
        note,
    )
}

fn save(project: &str, id: &str, body: &str) -> Value {
    json!({ "project": project, "id": id, "kind": "project", "description": "d", "body": body })
}

fn status_of(stand: &Stand, token: &str) -> (u16, Note) {
    let (response, note) = answer(&request(token, "ping", &json!({})), &stand.host);
    (response.status, note)
}

#[test]
fn the_legacy_token_writes_to_the_personal_store_in_the_owners_name() {
    let stand = stand("legacy", None);
    let (answer, refused, note) = call(
        &stand,
        LEGACY,
        "memory_save",
        &save("Probe", "from-https", "b"),
    );
    assert!(!refused, "{answer}");
    assert!(matches!(note, Note::Call { ref token, ok: true, .. } if token == "tk_legacy"));

    let author = git(&stand.personal, &["log", "-1", "--format=%an", "main"]);
    assert_eq!(author, "host-tk_legacy", "the commit says who wrote it");
    let journal = git(
        &stand.personal,
        &["show", "main:projects/Probe/memory.jsonl"],
    );
    let line: Value = serde_json::from_str(&journal).expect("one line");
    assert!(
        line["uuid"]
            .as_str()
            .unwrap()
            .starts_with("host-tk_legacy-"),
        "{line}"
    );
    assert_eq!(line["record"]["member"], "borodatych", "{line}");
    assert_eq!(line["record"]["agent"], "claude-code", "{line}");
}

#[test]
fn a_revoked_token_is_refused_by_the_next_request_without_a_restart() {
    let stand = stand("revoke", None);
    assert_eq!(status_of(&stand, ALICE).0, 200);

    let mut revoked = snapshot(&stand.personal);
    revoked["tokens"]
        .as_array_mut()
        .unwrap()
        .retain(|token| token["id"] != "tk_7q2m9x4a");
    publish(&stand.access, &revoked.to_string());
    assert_eq!(status_of(&stand, ALICE), (401, Note::Refused));
    assert_eq!(status_of(&stand, LEGACY).0, 200, "the others still get in");
}

#[test]
fn a_snapshot_that_breaks_shuts_the_door_until_it_reads_cleanly() {
    let stand = stand("broken", None);
    // A second member of the personal store: its transcripts are the owner's alone.
    let mut broken = snapshot(&stand.personal);
    broken["teams"]["personal"]["members"]["alice"] = json!("member");
    publish(&stand.access, &broken.to_string());
    assert_eq!(status_of(&stand, LEGACY), (503, Note::Quiet));
    assert_eq!(
        status_of(&stand, ALICE).0,
        503,
        "nobody, not only the owner"
    );

    fs::remove_file(&stand.access).expect("remove");
    assert_eq!(
        status_of(&stand, LEGACY).0,
        503,
        "a vanished file is no better"
    );

    publish(&stand.access, &snapshot(&stand.personal).to_string());
    assert_eq!(status_of(&stand, LEGACY).0, 200);
}

#[test]
fn a_host_does_not_start_without_rights_it_can_read() {
    let temp = Temp::new("start");
    let missing = Host::open(temp.0.join("access.json"), temp.0.clone(), None);
    assert!(missing.is_err());
    fs::write(temp.0.join("access.json"), "{\"version\": 2}").expect("write");
    assert!(Host::open(temp.0.join("access.json"), temp.0.clone(), None).is_err());
}

#[test]
fn a_memory_team_writes_only_to_the_projects_the_cabinet_listed() {
    let stand = stand("projects", None);
    let (answer, refused, _) = call(&stand, ALICE, "memory_save", &save("VibeIDE", "one", "b"));
    assert!(!refused, "{answer}");
    let team = stand.temp.0.join("teams/vibebrains.git");
    let journal = git(&team, &["show", "main:projects/VibeIDE/memory.jsonl"]);
    assert!(journal.contains("\"member\":\"alice\""), "{journal}");
    assert!(
        journal.contains("\"uuid\":\"host-tk_7q2m9x4a-"),
        "{journal}"
    );

    let (answer, refused, note) = call(&stand, ALICE, "memory_save", &save("Other", "two", "b"));
    assert!(refused, "{answer}");
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("a write does not create one"),
        "{answer}"
    );
    assert!(matches!(note, Note::Call { ok: false, .. }));

    // The host cannot see the client's disk; it answers with what the caller may use instead.
    let (resolved, refused, _) = call(
        &stand,
        ALICE,
        "project_resolve",
        &json!({ "directory": "/Users/alice/VibeIDE" }),
    );
    assert!(!refused, "{resolved}");
    assert!(resolved["project"].is_null(), "{resolved}");
    assert_eq!(resolved["projects"], json!(["VibeIDE"]));
}

#[test]
fn a_reader_reads_and_does_not_write() {
    let stand = stand("reader", None);
    let (_, refused, _) = call(&stand, READER, "memory_search", &json!({ "query": "" }));
    assert!(!refused);
    let (answer, refused, _) = call(&stand, READER, "memory_save", &save("VibeIDE", "one", "b"));
    assert!(refused);
    assert!(
        answer["error"].as_str().unwrap().contains("only reads"),
        "{answer}"
    );
}

#[test]
fn the_team_limits_are_checked_before_anything_is_written() {
    let stand = stand("limits", None);
    for id in ["one", "two"] {
        let (answer, refused, _) = call(&stand, ALICE, "memory_save", &save("VibeIDE", id, "b"));
        assert!(!refused, "{answer}");
    }
    let (answer, refused, _) = call(&stand, ALICE, "memory_save", &save("VibeIDE", "three", "b"));
    assert!(refused);
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .starts_with("too_many_entries"),
        "{answer}"
    );
    // A change to a record that exists is not a new record, but it is held to the size limit.
    let big = "x".repeat(4096);
    let (answer, refused, _) = call(
        &stand,
        ALICE,
        "memory_update",
        &json!({ "project": "VibeIDE", "id": "one", "body": big }),
    );
    assert!(refused);
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .starts_with("entry_too_large"),
        "{answer}"
    );
    let team = stand.temp.0.join("teams/vibebrains.git");
    let journal = git(&team, &["show", "main:projects/VibeIDE/memory.jsonl"]);
    assert_eq!(journal.lines().count(), 2, "nothing refused was written");
}

#[test]
fn past_sessions_are_searched_only_with_a_token_that_may() {
    let stand = stand("history", None);
    let (answer, refused, _) = call(
        &stand,
        ALICE,
        "history_search",
        &json!({ "query": "words" }),
    );
    assert!(refused);
    assert!(
        answer["error"].as_str().unwrap().contains("past sessions"),
        "{answer}"
    );
    let (answer, refused, _) = call(
        &stand,
        LEGACY,
        "history_search",
        &json!({ "query": "old words" }),
    );
    assert!(!refused, "{answer}");
    assert_eq!(
        answer["results"].as_array().map(Vec::len),
        Some(1),
        "{answer}"
    );
}

#[test]
fn an_expired_token_is_answered_like_an_unknown_one() {
    let stand = stand("expired", None);
    let (unknown, _) = answer(
        &request("vmt_7q2m9x4a_not-it", "ping", &json!({})),
        &stand.host,
    );
    let (expired, note) = answer(&request(EXPIRED, "ping", &json!({})), &stand.host);
    assert_eq!(expired, unknown);
    assert_eq!(note, Note::Expired("tk_c4d5e6f7".to_owned()));
}

#[test]
fn a_read_only_team_says_where_its_grant_is_renewed() {
    let stand = stand("readonly", Some("https://app.example.invalid"));
    let mut expired = snapshot(&stand.personal);
    expired["teams"]["vibebrains"]["writable"] = json!(false);
    publish(&stand.access, &expired.to_string());
    let (answer, refused, _) = call(&stand, ALICE, "memory_save", &save("VibeIDE", "one", "b"));
    assert!(refused);
    let problem = answer["error"].as_str().unwrap();
    assert!(
        problem.contains("read-only") && problem.contains("https://app.example.invalid"),
        "{problem}"
    );
    let (_, refused, _) = call(&stand, ALICE, "memory_search", &json!({ "query": "" }));
    assert!(!refused, "reading goes on");
}
