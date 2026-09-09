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

    /// A uuid for a new event: unique on this machine, and the same shape the engine writes.
    fn new_version(&self, id: &str) -> String;

    /// The clock, so the tools do not have one.
    fn now(&self) -> String;
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
        format!("{}-{}-{nth}-{id}", self.machine_id, self.now())
    }

    fn now(&self) -> String {
        vibememory_cli::clock::now()
    }
}

/// A journal held in memory, for tests and for anyone embedding the server.
#[derive(Debug, Default)]
pub struct FakeMemories {
    /// Events by project, in the order they were appended.
    pub events: std::cell::RefCell<BTreeMap<String, Vec<Event>>>,
    /// What [`Memories::now`] answers.
    pub stamp: String,
    /// Same job as the real one's: a fixed clock makes collisions certain rather than likely.
    written: AtomicU64,
}

impl FakeMemories {
    /// An empty fake with a fixed clock.
    #[must_use]
    pub fn new(stamp: &str) -> Self {
        Self {
            events: std::cell::RefCell::new(BTreeMap::new()),
            stamp: stamp.to_owned(),
            written: AtomicU64::new(0),
        }
    }

    /// Seeds a project with events, as if they had been written before.
    pub fn seed(&self, project: &str, events: Vec<Event>) {
        self.events.borrow_mut().insert(project.to_owned(), events);
    }
}

impl Memories for FakeMemories {
    fn projects(&self) -> Result<Vec<String>, String> {
        Ok(self.events.borrow().keys().cloned().collect())
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
