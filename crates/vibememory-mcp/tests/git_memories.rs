//! The store on its host: memory read from a bare repository and written to it as commits.

// The test builds repositories on disk and runs git, so the purity gate is lifted here.
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
use vibememory_mcp::git_memories::{Appended, GitMemories};
use vibememory_mcp::memories::{Memories, StoreMemories};
use vibememory_mcp::tools::{self, Caller};

const CALLER: Caller<'static> = Caller::owner("host-test", None);

/// A directory of this test, removed at the end.
struct Temp(PathBuf);

impl Temp {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vibememory-git-memories-{label}-{}",
            std::process::id()
        ));
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

fn git(dir: &Path, args: &[&str], date: Option<&str>) -> String {
    let mut command = Command::new("git");
    command.args(args).current_dir(dir);
    command
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid");
    if let Some(date) = date {
        command
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date);
    }
    let output = command.output().expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// The root of a repository made read-only for as long as it is held, and writable again after,
/// so that the directory of the test can be removed. Unix only: the host is Linux, and its rights
/// are what this imitates.
#[cfg(unix)]
struct ReadOnlyRoot(PathBuf);

#[cfg(unix)]
impl ReadOnlyRoot {
    fn new(repo: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(repo, fs::Permissions::from_mode(0o555)).expect("read-only");
        Self(repo.to_path_buf())
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyRoot {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
    }
}

/// A bare store and a machine's working copy of it, with memory and two sessions of different ages.
fn host_and_machine(temp: &Temp) -> (PathBuf, PathBuf) {
    let bare = temp.0.join("store.git");
    let work = temp.0.join("machine");
    fs::create_dir_all(&bare).expect("bare");
    git(
        &bare,
        &["init", "--bare", "--quiet", "--initial-branch=main"],
        None,
    );
    git(
        &temp.0,
        &["clone", "--quiet", bare.to_str().unwrap(), "machine"],
        None,
    );
    git(&work, &["symbolic-ref", "HEAD", "refs/heads/main"], None);

    // A project exists before memory is written to it — a session made it — because a write does
    // not create one.
    fs::create_dir_all(work.join("projects/Project")).expect("project");
    let local = StoreMemories::new(work.clone(), "mac-test".to_owned());
    save(
        &local,
        "Project",
        "store-naming",
        "where the store name comes from",
    );
    fs::write(
        work.join("projects/Project/older.jsonl"),
        "{\"said\":\"old words\"}\n",
    )
    .expect("t");
    git(&work, &["add", "-A"], None);
    git(
        &work,
        &["commit", "--quiet", "-m", "older"],
        Some("2026-09-01T10:00:00Z"),
    );
    fs::write(
        work.join("projects/Project/newer.jsonl"),
        "{\"said\":\"new words\"}\n",
    )
    .expect("t");
    git(&work, &["add", "-A"], None);
    git(
        &work,
        &["commit", "--quiet", "-m", "newer"],
        Some("2026-09-02T10:00:00Z"),
    );
    // The older session is continued after the newer one was written: its last commit, not its
    // first, is when it was last worked on.
    fs::write(
        work.join("projects/Project/older.jsonl"),
        "{\"said\":\"old words\"}\n{\"said\":\"continued\"}\n",
    )
    .expect("t");
    git(&work, &["add", "-A"], None);
    git(
        &work,
        &["commit", "--quiet", "-m", "older continued"],
        Some("2026-09-03T10:00:00Z"),
    );
    git(&work, &["push", "--quiet", "origin", "main"], None);
    (bare, work)
}

fn save(memories: &dyn Memories, project: &str, id: &str, description: &str) -> Value {
    tools::call(
        "memory_save",
        &json!({ "project": project, "id": id, "kind": "project", "description": description, "body": "b" }),
        &CALLER,
        memories,
    )
    .expect("save")
}

#[test]
fn a_bare_store_answers_what_the_working_copy_answers() {
    let temp = Temp::new("read");
    let (bare, work) = host_and_machine(&temp);
    let host = GitMemories::new(bare, "host".to_owned());
    let local = StoreMemories::new(work, "mac-test".to_owned());

    assert_eq!(host.projects().unwrap(), vec!["Project".to_owned()]);
    let search = json!({ "query": "store" });
    assert_eq!(
        tools::call("memory_search", &search, &CALLER, &host).unwrap(),
        tools::call("memory_search", &search, &CALLER, &local).unwrap(),
        "one memory, whichever way it is read"
    );

    let sessions: Vec<String> = host
        .transcripts("Project")
        .unwrap()
        .into_iter()
        .map(|transcript| transcript.session)
        .collect();
    assert_eq!(
        sessions,
        vec!["older", "newer"],
        "by the last commit that touched each, the journal left out"
    );
    assert_eq!(
        host.read_transcript("Project", "older").unwrap(),
        b"{\"said\":\"old words\"}\n{\"said\":\"continued\"}\n"
    );
    assert!(host.read_transcript("Project", "absent").is_err());
    assert_eq!(
        host.load("Nowhere").unwrap().records.len(),
        0,
        "no journal is empty memory"
    );
}

#[test]
fn a_write_on_the_host_is_a_commit_a_machine_fetches() {
    let temp = Temp::new("write");
    let (bare, work) = host_and_machine(&temp);
    let host = GitMemories::new(bare.clone(), "host".to_owned());
    let before = git(&bare, &["rev-parse", "main"], None);

    // As on the host: the writer may add objects and move refs, and nothing else — the root of the
    // repository, with its `config` and `hooks/`, is the owner's. A write that puts anything there,
    // a temporary index say, is refused.
    #[cfg(unix)]
    let _root = ReadOnlyRoot::new(&bare);
    save(
        &host,
        "Project",
        "from-the-host",
        "written over MCP on the host",
    );

    assert_eq!(
        git(&bare, &["rev-parse", "main^"], None),
        before,
        "exactly one commit on top of what was there"
    );
    git(&work, &["pull", "--quiet", "origin", "main"], None);
    let local = StoreMemories::new(work, "mac-test".to_owned());
    let found = tools::call(
        "memory_get",
        &json!({ "project": "Project", "id": "from-the-host" }),
        &CALLER,
        &local,
    );
    assert!(found.is_ok(), "the machine has it after a fetch: {found:?}");
    assert!(
        tools::call(
            "memory_get",
            &json!({ "project": "Project", "id": "store-naming" }),
            &CALLER,
            &local
        )
        .is_ok(),
        "and nothing that was there is gone"
    );
}

#[test]
fn a_write_that_started_before_a_push_changes_nothing() {
    let temp = Temp::new("stale");
    let (bare, work) = host_and_machine(&temp);
    let host = GitMemories::new(bare.clone(), "host".to_owned());
    let stale = git(&bare, &["rev-parse", "main"], None);

    fs::write(work.join("projects/Project/pushed.jsonl"), "{}\n").expect("t");
    git(&work, &["add", "-A"], None);
    git(&work, &["commit", "--quiet", "-m", "a push lands"], None);
    git(&work, &["push", "--quiet", "origin", "main"], None);
    let pushed = git(&bare, &["rev-parse", "main"], None);

    let outcome = host
        .append_on(&stale, "Project", b"{\"line\":1}\n", "late write")
        .expect("append");
    assert_eq!(outcome, Appended::Moved);
    assert_eq!(
        git(&bare, &["rev-parse", "main"], None),
        pushed,
        "the push keeps its commit"
    );
    assert!(
        fs::read_dir(&bare)
            .unwrap()
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with("vibememory-mcp-index")),
        "no temporary index left behind"
    );
}
