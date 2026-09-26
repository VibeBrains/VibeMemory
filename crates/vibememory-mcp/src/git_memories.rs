//! Memory read from and written to a bare repository — the store on its host, where there is no
//! working copy to read files from.
//!
//! The host keeps the store as a bare repository and nothing else: a checkout would be another
//! gigabyte on a disk that filled up on 2026-09-12. So everything goes through git itself. Reading
//! is blobs of `main`; writing an event is a commit on top of `main` that moves the branch only if
//! nobody moved it in the meantime. A machine then receives the write by an ordinary fetch, and the
//! journal merge driver unions it with whatever that machine wrote itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use vibememory_core::memory::journal::{self, Event, Memory, fold};

use crate::memories::{DirectoryProject, Memories, TranscriptRef, next_write, version_name};

/// The branch the store keeps its history on.
const BRANCH: &str = vibememory_cli::tick::BRANCH;

/// How many times a write is retried when a push moved `main` under it.
const APPEND_ATTEMPTS: usize = 5;

/// The store as a bare repository, written to on behalf of one writer.
///
/// Cheap to make: the server makes one per request, because the writer is whoever the request's
/// token or key says it is.
pub struct GitMemories {
    repo: PathBuf,
    /// What versions and commits are signed with: a machine for the owner's own ssh session,
    /// `host-<token id>` for a write over HTTPS.
    writer: String,
}

/// Names the temporary index of each write. One per process: two requests of one server write at
/// once, each with its own index file.
fn next_index() -> u64 {
    static INDEXES: AtomicU64 = AtomicU64::new(0);
    INDEXES.fetch_add(1, Ordering::Relaxed)
}

/// Runs git on the bare repository `repo` and returns its stdout as bytes.
///
/// Always with `--git-dir`: on the host the repositories belong to other accounts, and git answers
/// `-C` or a working directory there with "dubious ownership", while an explicit git dir is not
/// checked. Output is read while git runs and the input is written beside it, so neither a
/// transcript of 19 MiB nor a long list of questions can fill a pipe and wedge it.
///
/// # Errors
///
/// Git that could not start, or its stderr when it failed.
pub fn run(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    env: &[(&str, &str)],
) -> Result<Vec<u8>, String> {
    use std::io::Write as _;

    let mut command = Command::new("git");
    command
        .arg("--git-dir")
        .arg(repo)
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
    // The input is written from a thread of its own while the output is read here: git answers as
    // it reads, and an answer that fills the pipe before the input is all written would wedge both
    let output = std::thread::scope(|scope| {
        let writer = input
            .map(|bytes| {
                child
                    .stdin
                    .take()
                    .ok_or_else(|| "git took no input".to_owned())
                    .map(|mut stdin| scope.spawn(move || stdin.write_all(bytes)))
            })
            .transpose()?;
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        // a git that stopped reading and failed says why on stderr, below; its broken pipe does not
        let written = writer.map_or(Ok(()), |writer| {
            writer
                .join()
                .map_err(|_| "the input of git could not be written".to_owned())?
                .map_err(|error| error.to_string())
        });
        Ok::<_, String>((output, written))
    })?;
    let (output, written) = output;
    if output.status.success() {
        written?;
        Ok(output.stdout)
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
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
    /// A bare repository, written to on behalf of `writer`.
    #[must_use]
    pub fn new(repo: PathBuf, writer: String) -> Self {
        Self { repo, writer }
    }

    fn git(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        env: &[(&str, &str)],
    ) -> Result<Vec<u8>, String> {
        run(&self.repo, args, input, env)
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

    /// A project's journal on `main` as the store keeps it, byte for byte; empty when it has none.
    ///
    /// # Errors
    ///
    /// What git refused.
    pub fn journal_bytes(&self, project: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .blob(BRANCH, &journal_path(project))?
            .unwrap_or_default())
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
        self.put_on(Some(old), &path, &content, message)
    }

    /// The commit `main` points at, or `None` in a repository that has no `main` yet. Asked with
    /// `for-each-ref`, so a missing branch is an answer and a broken repository an error.
    ///
    /// # Errors
    ///
    /// What git refused.
    pub fn main_commit(&self) -> Result<Option<String>, String> {
        let found = self.text(
            &[
                "for-each-ref",
                "--format=%(objectname)",
                &format!("refs/heads/{BRANCH}"),
            ],
            &[],
        )?;
        Ok((!found.is_empty()).then_some(found))
    }

    /// Makes `path` on `main` hold exactly `content`, committing only when it does not; in a
    /// repository without `main` the commit is its first. The host keeps a team store's
    /// `.gitattributes` this way. Returns whether a commit was made.
    ///
    /// # Errors
    ///
    /// What git refused, or `main` moving under every attempt.
    pub fn settle_file(&self, path: &str, content: &[u8], message: &str) -> Result<bool, String> {
        for _ in 0..APPEND_ATTEMPTS {
            let old = self.main_commit()?;
            if let Some(old) = &old
                && self.blob(old, path)?.as_deref() == Some(content)
            {
                return Ok(false);
            }
            if let Appended::Committed(_) = self.put_on(old.as_deref(), path, content, message)? {
                return Ok(true);
            }
        }
        Err(format!(
            "{BRANCH} moved under every one of {APPEND_ATTEMPTS} attempts; {path} was not written"
        ))
    }

    /// One commit that makes `path` hold `content`: on top of `old`, or the repository's first
    /// commit when `old` is `None`. `main` moves only if it is still at `old` — still missing, for
    /// a first commit.
    fn put_on(
        &self,
        old: Option<&str>,
        path: &str,
        content: &[u8],
        message: &str,
    ) -> Result<Appended, String> {
        let blob = text_of(&self.git(&["hash-object", "-w", "--stdin"], Some(content), &[])?);

        // Outside the repository: on the host the server writes as a user who may add objects and
        // move refs there and nothing else — its `config` and `hooks/` are the owner's.
        let index = std::env::temp_dir().join(format!(
            "vibememory-mcp-index-{}-{}",
            std::process::id(),
            next_index()
        ));
        let index_path = index.to_string_lossy().into_owned();
        let attempt = (|| -> Result<Appended, String> {
            let with_index = [("GIT_INDEX_FILE", index_path.as_str())];
            match old {
                Some(old) => self.git(&["read-tree", old], None, &with_index)?,
                None => self.git(&["read-tree", "--empty"], None, &with_index)?,
            };
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
            let email = format!("vibememory-mcp@{}.invalid", self.writer);
            let identity = [
                ("GIT_AUTHOR_NAME", self.writer.as_str()),
                ("GIT_AUTHOR_EMAIL", email.as_str()),
                ("GIT_COMMITTER_NAME", self.writer.as_str()),
                ("GIT_COMMITTER_EMAIL", email.as_str()),
            ];
            let mut arguments = vec!["commit-tree", tree.as_str()];
            if let Some(old) = old {
                arguments.extend(["-p", old]);
            }
            arguments.extend(["-m", message]);
            let commit = self.text(&arguments, &identity)?;
            // The old value makes this a compare-and-swap: a push that landed in between keeps
            // its commit, and this write is done again on top of it. An empty old value is git's
            // "the branch must not exist yet".
            let Err(why) = self.move_branch(&commit, old.unwrap_or_default()) else {
                return Ok(Appended::Committed(commit));
            };
            if self.main_commit()?.as_deref() == old {
                Err(format!("git refused to move {BRANCH} to {commit}: {why}"))
            } else {
                Ok(Appended::Moved)
            }
        })();
        let _ = std::fs::remove_file(&index);
        attempt
    }

    /// Moves the branch from `old` to `new`, and only if it is still at `old`.
    ///
    /// Through a `HEAD` of its own. Git locks `HEAD` whenever it moves the branch `HEAD` names, and
    /// the lock is created beside `HEAD`, at the root of the repository — which on the host is the
    /// owner's and closed to the server, because `config` and `hooks/` live there. A linked
    /// worktree's directory, holding its own `HEAD` and a `commondir` naming the repository, takes
    /// that lock instead; the branch itself is locked and moved in the repository's `refs/`, the
    /// same way every push moves it, so a push and a write still exclude each other.
    fn move_branch(&self, new: &str, old: &str) -> Result<(), String> {
        let repo = std::fs::canonicalize(&self.repo)
            .map_err(|error| format!("{}: {error}", self.repo.display()))?;
        let own = std::env::temp_dir().join(format!(
            "vibememory-mcp-head-{}-{}",
            std::process::id(),
            next_index()
        ));
        let moved = (|| -> Result<(), String> {
            std::fs::create_dir(&own).map_err(|error| format!("{}: {error}", own.display()))?;
            std::fs::write(own.join("HEAD"), format!("ref: refs/heads/{BRANCH}\n"))
                .map_err(|error| error.to_string())?;
            std::fs::write(own.join("commondir"), format!("{}\n", repo.display()))
                .map_err(|error| error.to_string())?;
            // Seen through a worktree, the repository is not bare to git, and git would keep a
            // reflog in its `logs/` — one more thing written at the root.
            let output = Command::new("git")
                .args(["-c", "core.logAllRefUpdates=false", "update-ref"])
                .arg(format!("refs/heads/{BRANCH}"))
                .args([new, old])
                .env("GIT_DIR", &own)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()
                .map_err(|error| format!("git could not be started: {error}"))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
            }
        })();
        let _ = std::fs::remove_dir_all(&own);
        moved
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
        version_name(&self.writer, &self.now(), next_write(), id)
    }

    fn now(&self) -> String {
        vibememory_cli::clock::now()
    }

    fn project_of_directory(&self, _directory: &str) -> Result<DirectoryProject, String> {
        Ok(DirectoryProject::NotVisible)
    }

    fn store_bytes(&self) -> Result<u64, String> {
        Ok(crate::disk::dir_size(&self.repo, None))
    }
}
