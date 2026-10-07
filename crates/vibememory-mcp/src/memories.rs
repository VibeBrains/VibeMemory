//! Where the memory lives, and the only part of this server that touches a disk.
//!
//! Everything above it works against the trait, so the protocol and the tools are tested against
//! a fake and never need a store, a clock or a machine id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use vibememory_core::memory::journal::{self, Event, Memory, fold};

use crate::handoffs::{self, Handoff};

/// The journal of one project, and a way to add to it.
///
/// One method per thing the server actually needs. Deliberately not "give me the store": a server
/// that could reach the whole store would sooner or later start doing the engine's work.
pub trait Memories {
    /// Every project that has a journal, in a stable order.
    ///
    /// # Errors
    ///
    /// What went wrong reading the store.
    fn projects(&self) -> Result<Vec<String>, String>;

    /// The folded memory of one project. A project without a journal is empty memory, not an
    /// error: it simply has not been written to yet.
    ///
    /// # Errors
    ///
    /// What went wrong reading the journal.
    fn load(&self, project: &str) -> Result<Memory, String>;

    /// Appends one event to a project's journal.
    ///
    /// # Errors
    ///
    /// What went wrong writing it.
    fn append(&self, project: &str, event: &Event) -> Result<(), String>;

    /// The sessions of a project, newest first. Names and times only — a corpus of 1.7 GiB
    /// cannot be read to answer "which sessions mention this".
    ///
    /// # Errors
    ///
    /// What went wrong reading the store.
    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String>;

    /// The bytes of one transcript, read only when the search actually reaches it.
    ///
    /// # Errors
    ///
    /// What went wrong reading the file.
    fn read_transcript(&self, project: &str, transcript: &TranscriptRef)
    -> Result<Vec<u8>, String>;

    /// The open hand-offs of one project: the notes an agent left for whoever continues a flow.
    ///
    /// # Errors
    ///
    /// What went wrong reading them. A directory that is not there is no hand-offs, not an error.
    fn handoffs(&self, project: &str) -> Result<Handoffs, String>;

    /// A uuid for a new event: unique on this machine, and the same shape the engine writes.
    fn new_version(&self, id: &str) -> String;

    /// The clock, so the tools do not have one.
    fn now(&self) -> String;

    /// The store project a directory of the client belongs to, by the engine's own naming rules.
    /// A server shared by several windows has no single directory of its own, so the agent asks
    /// about the folder it works in instead of guessing a name from it.
    ///
    /// # Errors
    ///
    /// What stopped the rules from answering, or a store that cannot see the client's disk.
    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String>;

    /// The bytes the store takes on disk: what a team's quota counts, history included.
    ///
    /// # Errors
    ///
    /// A store that cannot be measured.
    fn store_bytes(&self) -> Result<u64, String>;
}

/// What the naming rules say about one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryProject {
    /// The store holds this project: writes may name it.
    Held {
        /// The store project.
        name: String,
        /// Which rule chose the name, as the engine's stable code.
        rule: String,
    },
    /// The rules give a name, but the store has no such project, and a write must not create one.
    Unheld {
        /// The name the rules would give.
        name: String,
    },
    /// The owner told the engine to leave this directory alone.
    Ignored {
        /// The rule that said so, in its own words.
        reason: String,
    },
    /// The store is on another machine and cannot see the client's disk: the client picks one of
    /// the projects it may use instead.
    NotVisible,
}

/// What a store can say about the hand-offs of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoffs {
    /// The open ones the working copy holds, by name.
    Read(Vec<Handoff>),
    /// This store keeps no working copy to read them from: the host reads a bare repository, and a
    /// hand-off is a file in a clone. The tool says so instead of answering "none", as
    /// `project_resolve` says why it cannot name the client's folder —
    /// [`DirectoryProject::NotVisible`].
    NotVisible,
}

/// A reference to memory is memory: a server shares one store between the requests it answers.
impl<M: Memories + ?Sized> Memories for &M {
    fn projects(&self) -> Result<Vec<String>, String> {
        (**self).projects()
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        (**self).load(project)
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        (**self).append(project, event)
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        (**self).transcripts(project)
    }

    fn read_transcript(
        &self,
        project: &str,
        transcript: &TranscriptRef,
    ) -> Result<Vec<u8>, String> {
        (**self).read_transcript(project, transcript)
    }

    fn handoffs(&self, project: &str) -> Result<Handoffs, String> {
        (**self).handoffs(project)
    }

    fn new_version(&self, id: &str) -> String {
        (**self).new_version(id)
    }

    fn now(&self) -> String {
        (**self).now()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        (**self).project_of_directory(directory)
    }

    fn store_bytes(&self) -> Result<u64, String> {
        (**self).store_bytes()
    }
}

/// The counter that tells apart versions written inside the same second.
///
/// One per process, starting at the process id shifted past any count a process reaches: two
/// servers on one machine — two ssh sessions in the same second — and two stores written by one
/// process must never produce the same uuid, because the union merge silently collapses events
/// that share one.
pub fn next_write() -> u64 {
    static WRITTEN: OnceLock<AtomicU64> = OnceLock::new();
    WRITTEN
        .get_or_init(|| AtomicU64::new(u64::from(std::process::id()) << 32))
        .fetch_add(1, Ordering::Relaxed)
}

/// One session of a project, without its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRef {
    /// The agent of a session another agent handed over with `session put`; `None` for Claude
    /// Code's own, which sit at the top of the project.
    pub agent: Option<String>,
    /// The session id, which is also the file name.
    pub session: String,
    /// When the file was last written, as seconds since the epoch. Only for ordering: the search
    /// goes newest first, because that is the order a person asks about their own history in.
    pub modified: u64,
}

/// The session a path inside a project directory is, if it is one: `<session>.jsonl` at the top for
/// Claude Code, `agents/<agent>/<session>.jsonl` for another agent. Anything deeper — a session's
/// own directory of subagents and tool results — and the memory journal are not sessions.
#[must_use]
pub fn transcript_of(name: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<&str> = name.split('/').collect();
    match parts.as_slice() {
        [file] => {
            let session = file.strip_suffix(".jsonl")?;
            (session != "memory" && !session.is_empty()).then(|| (None, session.to_owned()))
        }
        [dir, agent, file] if *dir == vibememory_core::foreign::AGENTS_DIR => {
            let session = file.strip_suffix(".jsonl")?;
            (!session.is_empty() && !agent.is_empty())
                .then(|| (Some((*agent).to_owned()), session.to_owned()))
        }
        _ => None,
    }
}

/// The directory a project's memory lives in, under the project: the journal and the projection
/// beside it. The engine's own layout, repeated here because the engine's name for it is private.
const MEMORY_DIR: &str = "memory";

/// The directory the hand-offs of a project live in, under its memory directory.
const HANDOFFS_DIR: &str = "sessions";

/// The real store under `<engine>/store/projects/<name>/memory.jsonl`.
pub struct StoreMemories {
    store: PathBuf,
    machine_id: String,
    /// Where the engine's configuration is: the personal store's parent, but not a team store's —
    /// that one's parent is the team's state directory.
    engine_dir: PathBuf,
}

impl StoreMemories {
    /// Points at the personal store directory on behalf of a machine.
    #[must_use]
    pub fn new(store: PathBuf, machine_id: String) -> Self {
        let engine_dir = store.parent().map(Path::to_path_buf).unwrap_or_default();
        Self {
            store,
            machine_id,
            engine_dir,
        }
    }

    /// Points at any store of this machine — the personal one or a team's clone — with the
    /// engine directory named, since a team clone does not lie directly in it.
    #[must_use]
    pub fn in_engine(engine_dir: PathBuf, store: PathBuf, machine_id: String) -> Self {
        Self {
            store,
            machine_id,
            engine_dir,
        }
    }

    /// The store directory this points at.
    #[must_use]
    pub fn store(&self) -> &Path {
        &self.store
    }

    fn journal_of(&self, project: &str) -> PathBuf {
        self.store
            .join("projects")
            .join(project)
            .join(vibememory_cli::memory::JOURNAL_FILE)
    }

    /// Where the hand-offs of a project lie: `memory/sessions/` beside the journal.
    fn handoffs_of(&self, project: &str) -> PathBuf {
        self.store
            .join("projects")
            .join(project)
            .join(MEMORY_DIR)
            .join(HANDOFFS_DIR)
    }
}

impl Memories for StoreMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        let root = self.store.join("projects");
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("{}: {error}", root.display())),
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        let path = self.journal_of(project);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let (events, unreadable) = journal::parse(&bytes);
        Ok(fold(&events, unreadable))
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        use std::io::Write as _;

        let path = self.journal_of(project);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let line = journal::encode(event).map_err(|error| error.to_string())?;
        // A token of a cabinet is refused by its shape before this (`Record::validate`); the owner's
        // token from before the cabinet has none, and only this machine knows its value
        if let Some(engine_dir) = self.store.parent() {
            let found = vibememory_cli::held::Kept::read(engine_dir).found_in(&line);
            if !found.is_empty() {
                let named: Vec<&str> = found.iter().map(String::as_str).collect();
                return Err(format!(
                    "holdsToken: the memory holds agent token {}, kept on this machine; it is not written",
                    named.join(", ")
                ));
            }
        }
        // Append, never rewrite: the journal is what two machines merge by union, and a server
        // that rewrote it would turn every concurrent write into a conflict.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.write_all(&line).map_err(|error| error.to_string())
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        let dir = self.store.join("projects").join(project);
        let mut found = sessions_in(&dir, None)?;
        let agents = dir.join(vibememory_core::foreign::AGENTS_DIR);
        if let Ok(entries) = std::fs::read_dir(&agents) {
            for agent in entries.filter_map(Result::ok) {
                if agent.path().is_dir() {
                    let name = agent.file_name().to_string_lossy().into_owned();
                    found.extend(sessions_in(&agent.path(), Some(&name))?);
                }
            }
        }
        found.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| left.session.cmp(&right.session))
        });
        Ok(found)
    }

    fn read_transcript(
        &self,
        project: &str,
        transcript: &TranscriptRef,
    ) -> Result<Vec<u8>, String> {
        let path = self.store.join(vibememory_core::foreign::session_path(
            project,
            transcript.agent.as_deref(),
            &transcript.session,
        ));
        std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn handoffs(&self, project: &str) -> Result<Handoffs, String> {
        Ok(Handoffs::Read(handoffs::open_in(
            &self.handoffs_of(project),
        )))
    }

    fn new_version(&self, id: &str) -> String {
        // Machine first, exactly as the engine's own events are named: two agents writing in the
        // same second on two machines must not produce the same uuid, or the union merge would
        // treat two different versions as one.
        //
        // The counter is not decoration. The engine writes at most one version of a record per
        // tick, so `{machine}-{stamp}-{id}` was unique for it; an agent over MCP can update the
        // same record twice inside one second, and two events sharing a uuid are silently
        // collapsed by the merge — the worst failure this system has, because nothing reports it.
        // Caught by the gate on the first run, on the test double before the real one.
        version_name(&self.machine_id, &self.now(), next_write(), id)
    }

    fn now(&self) -> String {
        vibememory_cli::clock::now()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        let path = Path::new(directory);
        // A relative path would be read against the server's own directory, which is exactly the
        // one that says nothing about the client's folder.
        if !path.is_absolute() {
            return Err(format!("directory must be an absolute path: {directory}"));
        }
        // Symlinks are followed here, and only here. Git answers about the real path, our own walk
        // for `.git` follows the path as written, and when the two differ the naming rules — quite
        // rightly — refuse to name anything: «git resolved /Volumes/… but the nearest `.git` is
        // /Users/…». That is not an exotic case. An IDE passes the folder as the person opened it,
        // and a person whose projects live on another disk opens them through a link in the home
        // directory; every call from VibeIDEA on such a machine failed, and the agent then asked
        // the owner to type the path by hand (18.09.2026).
        //
        // The cwd the hook path uses comes from a shell, which resolved the link already — which is
        // why this never showed up there.
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        directory_project(&self.engine_dir, &self.store, &real)
    }

    fn store_bytes(&self) -> Result<u64, String> {
        Ok(crate::disk::dir_size(&self.store, None))
    }
}

/// The uuid of a new event: machine, time, a counter for writes inside the same second, record.
/// One rule for every store the server writes to — see [`StoreMemories::new_version`] for why each
/// part is there.
#[must_use]
pub fn version_name(machine_id: &str, now: &str, nth: u64, id: &str) -> String {
    format!("{machine_id}-{now}-{nth}-{id}")
}

/// The sessions of one directory: its `*.jsonl` files, the memory journal aside.
fn sessions_in(dir: &Path, agent: Option<&str>) -> Result<Vec<TranscriptRef>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", dir.display())),
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (_, session) = transcript_of(&name)?;
            let modified = entry
                .metadata()
                .ok()
                .and_then(|data| data.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            Some(TranscriptRef {
                agent: agent.map(str::to_owned),
                session,
                modified,
            })
        })
        .collect())
}

/// One transcript of the fake: session id, modified time, raw lines.
pub type FakeTranscript = (String, u64, String);

/// A transcript of the fake with the agent that handed it over, `None` for Claude Code.
pub type FakeSession = (Option<String>, FakeTranscript);

/// A journal held in memory, for tests and for anyone embedding the server. Behind locks, because
/// the HTTP server answers each connection on its own thread.
#[derive(Debug, Default)]
pub struct FakeMemories {
    /// Events by project, in the order they were appended.
    pub events: Mutex<BTreeMap<String, Vec<Event>>>,
    /// What [`Memories::now`] answers.
    pub stamp: String,
    /// Same job as the real one's: a fixed clock makes collisions certain rather than likely.
    written: AtomicU64,
    /// Transcripts by project, each with the agent that handed it over (`None` for Claude Code).
    pub history: Mutex<BTreeMap<String, Vec<FakeSession>>>,
    /// How many transcripts were actually read. The cap on results is cheap to check; the cap on
    /// *reading* is the one that matters on a 1.7 GiB corpus, and it is invisible without this.
    pub reads: AtomicU64,
    /// What [`Memories::project_of_directory`] answers, by directory.
    pub directories: Mutex<BTreeMap<String, DirectoryProject>>,
    /// The hand-offs of each project; a project that was not seeded has none.
    pub handoffs: Mutex<BTreeMap<String, Vec<Handoff>>>,
    /// What [`Memories::store_bytes`] answers: the size a test gives the store.
    pub store_bytes: AtomicU64,
}

/// A fake's lock is never held across a panic that matters: whatever it holds is still the
/// state the test wants to look at.
fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeMemories {
    /// An empty fake with a fixed clock.
    #[must_use]
    pub fn new(stamp: &str) -> Self {
        Self {
            events: Mutex::new(BTreeMap::new()),
            stamp: stamp.to_owned(),
            written: AtomicU64::new(0),
            history: Mutex::new(BTreeMap::new()),
            reads: AtomicU64::new(0),
            directories: Mutex::new(BTreeMap::new()),
            handoffs: Mutex::new(BTreeMap::new()),
            store_bytes: AtomicU64::new(0),
        }
    }

    /// Seeds a project with events, as if they had been written before.
    pub fn seed(&self, project: &str, events: Vec<Event>) {
        held(&self.events).insert(project.to_owned(), events);
    }

    /// The events written so far, by project.
    #[must_use]
    pub fn written(&self) -> BTreeMap<String, Vec<Event>> {
        held(&self.events).clone()
    }

    /// Adds a transcript to a project.
    pub fn add_transcript(&self, project: &str, transcript: FakeTranscript) {
        held(&self.history)
            .entry(project.to_owned())
            .or_default()
            .push((None, transcript));
    }

    /// Seeds what [`Memories::project_of_directory`] answers for one directory.
    pub fn answer_directory(&self, directory: &str, answer: DirectoryProject) {
        held(&self.directories).insert(directory.to_owned(), answer);
    }

    /// Seeds the hand-offs of a project, as the working copy would hold them.
    pub fn seed_handoffs(&self, project: &str, handoffs: Vec<Handoff>) {
        held(&self.handoffs).insert(project.to_owned(), handoffs);
    }
}

impl Memories for FakeMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        // Memory journals and transcripts both live under `projects/<name>`, and the real store
        // answers by listing that directory. A project with sessions but no memory yet is still a
        // project, so the fake has to say so too.
        let mut names: Vec<String> = held(&self.events).keys().cloned().collect();
        names.extend(held(&self.history).keys().cloned());
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        let events = held(&self.events).get(project).cloned().unwrap_or_default();
        Ok(fold(&events, Vec::new()))
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        held(&self.events)
            .entry(project.to_owned())
            .or_default()
            .push(event.clone());
        Ok(())
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        let history = held(&self.history);
        let mut found: Vec<TranscriptRef> = history
            .get(project)
            .into_iter()
            .flatten()
            .map(|(agent, (session, modified, _))| TranscriptRef {
                agent: agent.clone(),
                session: session.clone(),
                modified: *modified,
            })
            .collect();
        found.sort_by_key(|transcript| std::cmp::Reverse(transcript.modified));
        Ok(found)
    }

    fn read_transcript(
        &self,
        project: &str,
        transcript: &TranscriptRef,
    ) -> Result<Vec<u8>, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        held(&self.history)
            .get(project)
            .into_iter()
            .flatten()
            .find(|(agent, (id, _, _))| *agent == transcript.agent && *id == transcript.session)
            .map(|(_, (_, _, text))| text.clone().into_bytes())
            .ok_or_else(|| format!("no transcript {}", transcript.session))
    }

    fn handoffs(&self, project: &str) -> Result<Handoffs, String> {
        Ok(Handoffs::Read(
            held(&self.handoffs)
                .get(project)
                .cloned()
                .unwrap_or_default(),
        ))
    }

    fn new_version(&self, id: &str) -> String {
        format!(
            "fake-{}-{}-{id}",
            self.stamp,
            self.written.fetch_add(1, Ordering::Relaxed)
        )
    }
    fn now(&self) -> String {
        self.stamp.clone()
    }

    fn project_of_directory(&self, directory: &str) -> Result<DirectoryProject, String> {
        held(&self.directories)
            .get(directory)
            .cloned()
            .ok_or_else(|| format!("no answer seeded for {directory}"))
    }

    fn store_bytes(&self) -> Result<u64, String> {
        Ok(self.store_bytes.load(Ordering::Relaxed))
    }
}

/// The store project of `cwd`, by the engine's own naming rules; `None` outside any project the
/// store already holds.
///
/// # Errors
///
/// An unreadable config, or what the naming rules refused.
pub fn project_here(engine_dir: &Path, store: &Path, cwd: &Path) -> Result<Option<String>, String> {
    Ok(match directory_project(engine_dir, store, cwd)? {
        DirectoryProject::Held { name, .. } => Some(name),
        // Only a project the store already holds. A client may start its servers in any
        // directory — measured: `/tmp` resolves to a project called `tmp` — and a default that
        // creates projects would scatter memory into places nobody syncs on purpose.
        DirectoryProject::Unheld { .. }
        | DirectoryProject::Ignored { .. }
        | DirectoryProject::NotVisible => None,
    })
}

/// Everything the naming rules say about `cwd` in `store`, including why there is no project.
///
/// # Errors
///
/// An unreadable config, or what the naming rules refused.
pub fn directory_project(
    engine_dir: &Path,
    store: &Path,
    cwd: &Path,
) -> Result<DirectoryProject, String> {
    let text = std::fs::read_to_string(engine_dir.join("config.json"))
        .map_err(|error| format!("config.json: {error}"))?;
    let config =
        vibememory_cli::config::Config::parse(&text, vibememory_core::naming::PathSyntax::Posix)
            .map_err(|error| format!("config.json: {error}"))?;
    let syntax = vibememory_cli::hook::session_start::host_syntax();
    let canonical = vibememory_core::naming::canonical_cwd(&cwd.to_string_lossy(), syntax);
    match vibememory_cli::project::resolve(store, &config.naming, &canonical) {
        Ok(vibememory_core::naming::Resolution::Named { name, source }) => {
            let held = store.join("projects").join(name.as_str()).is_dir();
            let name = name.as_str().to_owned();
            Ok(if held {
                DirectoryProject::Held {
                    name,
                    rule: source.code().to_owned(),
                }
            } else {
                DirectoryProject::Unheld { name }
            })
        }
        Ok(vibememory_core::naming::Resolution::Ignored { reason }) => {
            Ok(DirectoryProject::Ignored {
                reason: match reason {
                    vibememory_core::naming::IgnoreReason::Pattern(pattern) => {
                        format!("ignoreCwd: {pattern}")
                    }
                    vibememory_core::naming::IgnoreReason::ProjectDirName(value) => {
                        format!("CLAUDE_CODE_PROJECT_DIR_NAME: {value}")
                    }
                },
            })
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The store a local server started in `cwd` works in — the personal one, or the clone of the team
/// the directory is routed to — with the member its versions are signed by in a team.
///
/// # Errors
///
/// An unreadable configuration, or a directory of a team that is not connected here: the server
/// then does not start rather than write a team's memory into the personal store.
pub fn for_directory(
    engine_dir: &Path,
    cwd: &Path,
) -> Result<(StoreMemories, Option<String>), String> {
    let text = std::fs::read_to_string(engine_dir.join("config.json"))
        .map_err(|error| format!("config.json: {error}"))?;
    // The same syntax the engine reads its own config with; the roots inside are this machine's,
    // and deriving it per string would make one file mean two things.
    let config =
        vibememory_cli::config::Config::parse(&text, vibememory_core::naming::PathSyntax::Posix)
            .map_err(|error| format!("config.json: {error}"))?;
    let layout = vibememory_cli::install::Layout {
        config_dir: PathBuf::new(),
        engine_dir: engine_dir.to_path_buf(),
        home: None,
    };
    let syntax = vibememory_cli::hook::session_start::host_syntax();
    let canonical = vibememory_core::naming::canonical_cwd(&cwd.to_string_lossy(), syntax);
    let store = vibememory_cli::stores::for_cwd(&layout, &config, &canonical, syntax)?;
    let member = match &store.team {
        Some(team) => Some(vibememory_cli::team_connect::read_record(&layout, team)?.member),
        None => None,
    };
    Ok((
        StoreMemories::in_engine(engine_dir.to_path_buf(), store.clone, store.machine_id),
        member,
    ))
}
