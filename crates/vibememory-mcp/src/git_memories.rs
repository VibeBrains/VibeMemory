//! Memory read from and written to a bare repository — the store on its host, where there is no
//! working copy to read files from.
//!
//! The host keeps the store as a bare repository and nothing else: a checkout would be another
//! gigabyte on a disk that filled up on 2026-09-12. So everything goes through git itself. Reading
//! is blobs of `main`; writing an event is a commit on top of `main` that moves the branch only if
//! nobody moved it in the meantime. A machine then receives the write by an ordinary fetch, and the
//! journal merge driver unions it with whatever that machine wrote itself.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use vibememory_core::memory::journal::{self, Event, Memory, fold};

use crate::memories::{DirectoryProject, Memories, TranscriptRef, version_name};

/// The branch the store keeps its history on.
const BRANCH: &str = vibememory_cli::tick::BRANCH;

/// How many times a write is retried when a push moved `main` under it.
const APPEND_ATTEMPTS: usize = 5;

/// The store as a bare repository.
pub struct GitMemories {
    repo: PathBuf,
    machine_id: String,
    /// Distinguishes versions written inside the same second, as for the working-copy store.
    written: AtomicU64,
    /// Names the temporary index of each write.
    indexes: AtomicU64,
}

/// What one attempt to append found.
#[derive(Debug, PartialEq, Eq)]
pub enum Appended {
    /// The event is on `main` in this commit.
    Committed(String),
    /// `main` was no longer where the attempt started; nothing was changed.
    Moved,
}

impl GitMemories {
    /// A bare repository, written to on behalf of `machine_id`.
    #[must_use]
    pub fn new(repo: PathBuf, machine_id: String) -> Self {
        Self {
            repo,
            machine_id,
            written: AtomicU64::new(0),
            indexes: AtomicU64::new(0),
        }
    }

    /// Runs git on the repository and returns its stdout as bytes. Output is read while git runs,
    /// so a transcript of 19 MiB cannot fill the pipe and wedge it.
    fn git(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>, String> {
        use std::io::Write as _;

        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&self.repo)
            .args(args)
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
        let mut child = command
            .spawn()
            .map_err(|error| format!("git could not be started: {error}"))?;
        if let Some(bytes) = input {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| "git took no input".to_owned())?;
            stdin.write_all(bytes).map_err(|error| error.to_string())?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn text(&self, args: &[&str], env: &[(&str, &str)]) -> Result<String, String> {
        self.git(args, None, env)
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
    }

    /// The blob at `path` in `commit`, or `None` when there is no such file. Absence is asked with
    /// `ls-tree`, so a broken repository is an error and not an empty journal.
    fn blob(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        if self.text(&["ls-tree", commit, "--", path], &[])?.is_empty() {
            return Ok(None);
        }
        self.git(
            &["cat-file", "blob", &format!("{commit}:{path}")],
            None,
            &[],
        )
        .map(Some)
    }

    /// One attempt to append `line` to a project's journal on top of `main` as it is now.
    ///
    /// # Errors
    ///
    /// What git refused.
    pub fn append_once(
        &self,
        project: &str,
        line: &[u8],
        message: &str,
    ) -> Result<Appended, String> {
        let old = self.text(&["rev-parse", "--verify", BRANCH], &[])?;
        self.append_on(&old, project, line, message)
    }

    /// The same on top of a given commit. Public so a gate can hand it one that is already stale:
    /// a write that started before a push must change nothing, not overwrite the push.
    ///
    /// # Errors
    ///
    /// What git refused.
    pub fn append_on(
        &self,
        old: &str,
        project: &str,
        line: &[u8],
        message: &str,
    ) -> Result<Appended, String> {
        let path = journal_path(project);
        let mut content = self.blob(old, &path)?.unwrap_or_default();
        // Append, never rewrite: the journal is what machines merge by union.
        content.extend_from_slice(line);
        let blob = text_of(&self.git(&["hash-object", "-w", "--stdin"], Some(&content), &[])?);

        let index = self.repo.join(format!(
            "vibememory-mcp-index-{}-{}",
            std::process::id(),
            self.indexes.fetch_add(1, Ordering::Relaxed)
        ));
        let index_path = index.to_string_lossy().into_owned();
        let attempt = (|| -> Result<Appended, String> {
            let with_index = [("GIT_INDEX_FILE", index_path.as_str())];
            self.git(&["read-tree", old], None, &with_index)?;
            self.git(
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    &format!("100644,{blob},{path}"),
                ],
                None,
                &with_index,
            )?;
            let tree = self.text(&["write-tree"], &with_index)?;
            let email = format!("vibememory-mcp@{}.invalid", self.machine_id);
            let identity = [
                ("GIT_AUTHOR_NAME", self.machine_id.as_str()),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_COMMITTER_NAME", self.machine_id.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
            ];
            let commit = self.text(&["commit-tree", &tree, "-p", old, "-m", message], &identity)?;
            let branch = format!("refs/heads/{BRANCH}");
            // The old value makes this a compare-and-swap: a push that landed in between keeps
            // its commit, and this write is done again on top of it.
            if self
                .git(&["update-ref", &branch, &commit, old], None, &[])
                .is_ok()
            {
                return Ok(Appended::Committed(commit));
            }
            let now = self.text(&["rev-parse", "--verify", BRANCH], &[])?;
            if now == old {
                Err(format!("git refused to move {BRANCH} to {commit}"))
            } else {
                Ok(Appended::Moved)
            }
        })();
        let _ = std::fs::remove_file(&index);
        attempt
    }
}

fn journal_path(project: &str) -> String {
    format!(
        "projects/{project}/{}",
        vibememory_cli::memory::JOURNAL_FILE
    )
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_owned()
}

impl Memories for GitMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        let listed = self.text(&["ls-tree", "-d", "--name-only", BRANCH, "projects/"], &[])?;
        let mut names: Vec<String> = listed
            .lines()
            .filter_map(|line| line.strip_prefix("projects/"))
            .map(str::to_owned)
            .collect();
        names.sort();
        Ok(names)
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        let bytes = self
            .blob(BRANCH, &journal_path(project))?
            .unwrap_or_default();
        let (events, unreadable) = journal::parse(&bytes);
        Ok(fold(&events, unreadable))
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        let line = journal::encode(event).map_err(|error| error.to_string())?;
        let message = format!("vibememory-mcp: memory of {project} at {}", self.now());
        for _ in 0..APPEND_ATTEMPTS {
            if let Appended::Committed(_) = self.append_once(project, &line, &message)? {
                return Ok(());
            }
        }
        Err(format!(
            "{BRANCH} moved under every one of {APPEND_ATTEMPTS} attempts; nothing was written"
        ))
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        let dir = format!("projects/{project}/");
        let listed = self.text(&["ls-tree", "--name-only", BRANCH, &dir], &[])?;
        let mut sessions: BTreeMap<String, u64> = listed
            .lines()
            .filter_map(|line| line.strip_prefix(dir.as_str()))
            .filter_map(|name| name.strip_suffix(".jsonl"))
            // The memory journal lives beside the transcripts and is not one of them.
            .filter(|session| *session != "memory")
            .map(|session| (session.to_owned(), 0))
            .collect();
        // A bare repository keeps no modification times. The time of the last commit that touched
        // a file is the same answer the tick would have given by writing it.
        let log = self.text(
            &["log", "--format=@%ct", "--name-only", BRANCH, "--", &dir],
            &[],
        )?;
        let mut time = 0;
        for line in log.lines() {
            if let Some(stamp) = line.strip_prefix('@') {
                time = stamp.parse().unwrap_or(0);
            } else if let Some(session) = line
                .strip_prefix(dir.as_str())
                .and_then(|name| name.strip_suffix(".jsonl"))
                && let Some(modified) = sessions.get_mut(session)
                && *modified == 0
            {
                *modified = time;
            }
        }
        let mut found: Vec<TranscriptRef> = sessions
            .into_iter()
            .map(|(session, modified)| TranscriptRef { session, modified })
            .collect();
        found.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.session.cmp(&right.session))
        });
        Ok(found)
    }

    fn read_transcript(&self, project: &str, session: &str) -> Result<Vec<u8>, String> {
        self.blob(BRANCH, &format!("projects/{project}/{session}.jsonl"))?
            .ok_or_else(|| format!("no transcript {session} in {project}"))
    }

    fn new_version(&self, id: &str) -> String {
        let nth = self.written.fetch_add(1, Ordering::Relaxed);
        version_name(&self.machine_id, &self.now(), nth, id)
    }

    fn now(&self) -> String {
        vibememory_cli::clock::now()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        Err(format!(
            "the store's host cannot see the client's disk, so it cannot name the project of \
             {directory}; pass project explicitly"
        ))
    }
}
