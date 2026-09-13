//! Where the memory lives, and the only part of this server that touches a disk.
//!
//! Everything above it works against the trait, so the protocol and the tools are tested against
//! a fake and never need a store, a clock or a machine id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use vibememory_core::memory::journal::{self, Event, Memory, fold};

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
    fn read_transcript(&self, project: &str, session: &str) -> Result<Vec<u8>, String>;

    /// A uuid for a new event: unique on this machine, and the same shape the engine writes.
    fn new_version(&self, id: &str) -> String;

    /// The clock, so the tools do not have one.
    fn now(&self) -> String;
}

/// One session of a project, without its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRef {
    /// The session id, which is also the file name.
    pub session: String,
    /// When the file was last written, as seconds since the epoch. Only for ordering: the search
    /// goes newest first, because that is the order a person asks about their own history in.
    pub modified: u64,
}

/// The real store under `<engine>/store/projects/<name>/memory.jsonl`.
pub struct StoreMemories {
    store: PathBuf,
    machine_id: String,
    /// Distinguishes versions written inside the same second. See [`Memories::new_version`].
    written: AtomicU64,
}

impl StoreMemories {
    /// Points at a store directory on behalf of a machine.
    #[must_use]
    pub fn new(store: PathBuf, machine_id: String) -> Self {
        Self {
            store,
            machine_id,
            written: AtomicU64::new(0),
        }
    }

    fn journal_of(&self, project: &str) -> PathBuf {
        self.store
            .join("projects")
            .join(project)
            .join(vibememory_cli::memory::JOURNAL_FILE)
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
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("{}: {error}", dir.display())),
        };
        let mut found: Vec<TranscriptRef> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                if path.extension()? != "jsonl" {
                    return None;
                }
                let session = path.file_stem()?.to_string_lossy().into_owned();
                // The memory journal lives beside the transcripts and is not one of them.
                if session == "memory" {
                    return None;
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|data| data.modified().ok())
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_secs());
                Some(TranscriptRef { session, modified })
            })
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
        let path = self
            .store
            .join("projects")
            .join(project)
            .join(format!("{session}.jsonl"));
        std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))
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
        let nth = self.written.fetch_add(1, Ordering::Relaxed);
        version_name(&self.machine_id, &self.now(), nth, id)
    }

    fn now(&self) -> String {
        vibememory_cli::clock::now()
    }
}

/// The uuid of a new event: machine, time, a counter for writes inside the same second, record.
/// One rule for every store the server writes to — see [`StoreMemories::new_version`] for why each
/// part is there.
#[must_use]
pub fn version_name(machine_id: &str, now: &str, nth: u64, id: &str) -> String {
    format!("{machine_id}-{now}-{nth}-{id}")
}

/// One transcript of the fake: session id, modified time, raw lines.
pub type FakeTranscript = (String, u64, String);

/// A journal held in memory, for tests and for anyone embedding the server.
#[derive(Debug, Default)]
pub struct FakeMemories {
    /// Events by project, in the order they were appended.
    pub events: std::cell::RefCell<BTreeMap<String, Vec<Event>>>,
    /// What [`Memories::now`] answers.
    pub stamp: String,
    /// Same job as the real one's: a fixed clock makes collisions certain rather than likely.
    written: AtomicU64,
    /// Transcripts by project.
    pub history: std::cell::RefCell<BTreeMap<String, Vec<FakeTranscript>>>,
    /// How many transcripts were actually read. The cap on results is cheap to check; the cap on
    /// *reading* is the one that matters on a 1.7 GiB corpus, and it is invisible without this.
    pub reads: AtomicU64,
}

impl FakeMemories {
    /// An empty fake with a fixed clock.
    #[must_use]
    pub fn new(stamp: &str) -> Self {
        Self {
            events: std::cell::RefCell::new(BTreeMap::new()),
            stamp: stamp.to_owned(),
            written: AtomicU64::new(0),
            history: std::cell::RefCell::new(BTreeMap::new()),
            reads: AtomicU64::new(0),
        }
    }

    /// Seeds a project with events, as if they had been written before.
    pub fn seed(&self, project: &str, events: Vec<Event>) {
        self.events.borrow_mut().insert(project.to_owned(), events);
    }
}

impl Memories for FakeMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        // Memory journals and transcripts both live under `projects/<name>`, and the real store
        // answers by listing that directory. A project with sessions but no memory yet is still a
        // project, so the fake has to say so too.
        let mut names: Vec<String> = self.events.borrow().keys().cloned().collect();
        names.extend(self.history.borrow().keys().cloned());
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn load(&self, project: &str) -> Result<Memory, String> {
        let borrowed = self.events.borrow();
        let events = borrowed.get(project).cloned().unwrap_or_default();
        Ok(fold(&events, Vec::new()))
    }

    fn append(&self, project: &str, event: &Event) -> Result<(), String> {
        self.events
            .borrow_mut()
            .entry(project.to_owned())
            .or_default()
            .push(event.clone());
        Ok(())
    }

    fn transcripts(&self, project: &str) -> Result<Vec<TranscriptRef>, String> {
        let borrowed = self.history.borrow();
        let mut found: Vec<TranscriptRef> = borrowed
            .get(project)
            .into_iter()
            .flatten()
            .map(|(session, modified, _)| TranscriptRef {
                session: session.clone(),
                modified: *modified,
            })
            .collect();
        found.sort_by_key(|transcript| std::cmp::Reverse(transcript.modified));
        Ok(found)
    }

    fn read_transcript(&self, project: &str, session: &str) -> Result<Vec<u8>, String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let borrowed = self.history.borrow();
        borrowed
            .get(project)
            .into_iter()
            .flatten()
            .find(|(id, _, _)| id == session)
            .map(|(_, _, text)| text.clone().into_bytes())
            .ok_or_else(|| format!("no transcript {session}"))
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
}

/// The store project of `cwd`, by the engine's own naming rules; `None` outside any project the
/// store already holds.
///
/// # Errors
///
/// An unreadable config, or what the naming rules refused.
pub fn project_here(engine_dir: &Path, cwd: &Path) -> Result<Option<String>, String> {
    let text = std::fs::read_to_string(engine_dir.join("config.json"))
        .map_err(|error| format!("config.json: {error}"))?;
    let config =
        vibememory_cli::config::Config::parse(&text, vibememory_core::naming::PathSyntax::Posix)
            .map_err(|error| format!("config.json: {error}"))?;
    let syntax = vibememory_cli::hook::session_start::host_syntax();
    let canonical = vibememory_core::naming::canonical_cwd(&cwd.to_string_lossy(), syntax);
    match vibememory_cli::project::resolve(&engine_dir.join("store"), &config.naming, &canonical) {
        Ok(vibememory_core::naming::Resolution::Named { name, .. }) => {
            // Only a project the store already holds. A client may start its servers in any
            // directory — measured: `/tmp` resolves to a project called `tmp` — and a default that
            // creates projects would scatter memory into places nobody syncs on purpose.
            let known = engine_dir
                .join("store")
                .join("projects")
                .join(name.as_str())
                .is_dir();
            Ok(known.then(|| name.as_str().to_owned()))
        }
        Ok(vibememory_core::naming::Resolution::Ignored { .. }) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// The store of this machine, from the engine's own configuration.
///
/// # Errors
///
/// What stopped the configuration from being read.
pub fn from_engine(engine_dir: &Path, config_dir: &Path) -> Result<StoreMemories, String> {
    let text = std::fs::read_to_string(engine_dir.join("config.json"))
        .map_err(|error| format!("config.json: {error}"))?;
    // The same syntax the engine reads its own config with; the roots inside are this machine's,
    // and deriving it per string would make one file mean two things.
    let config =
        vibememory_cli::config::Config::parse(&text, vibememory_core::naming::PathSyntax::Posix)
            .map_err(|error| format!("config.json: {error}"))?;
    let _ = config_dir;
    Ok(StoreMemories::new(
        engine_dir.join("store"),
        config.machine_id,
    ))
}
