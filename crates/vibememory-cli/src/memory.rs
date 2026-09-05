//! Memory on this machine: where the CLI keeps it, and how the store's records get in and out.
//!
//! The records are the truth (`projects/<name>/memory.jsonl`, an append-only journal); the
//! markdown the CLI reads and writes is a projection of them. Both hooks run the same `sync`:
//! read the projection back into events first, so an edit made since the last run becomes a
//! version before anything is regenerated, then write the projection out. Done in that order,
//! nothing a person or a model wrote is ever overwritten by the engine's own output.

use std::path::{Path, PathBuf};

use vibememory_core::memory::journal::{self, Memory, fold};
use vibememory_core::memory::markdown;

/// Cowork's override of the memory path. It outranks everything: a Cowork session keeps its
/// memory where Cowork says, not where the project says.
pub const COWORK_MEMORY_VAR: &str = "CLAUDE_COWORK_MEMORY_PATH_OVERRIDE";
/// The remote-session override.
pub const REMOTE_MEMORY_VAR: &str = "CLAUDE_CODE_REMOTE_MEMORY_DIR";
/// The settings key that moves memory out of `projects/<enc>/memory`.
pub const SETTINGS_MEMORY_KEY: &str = "autoMemoryDirectory";
/// The journal's file name inside the project's store directory.
pub const JOURNAL_FILE: &str = "memory.jsonl";
/// The directory name the CLI uses under a project.
const MEMORY_DIR_NAME: &str = "memory";
/// Where quarantined versions go, under the engine directory.
const QUARANTINE_DIR: &str = "quarantine";

/// What this machine knows about where memory is, before any path is chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryLocation<'a> {
    /// `CLAUDE_COWORK_MEMORY_PATH_OVERRIDE`, if set.
    pub cowork_override: Option<&'a str>,
    /// `CLAUDE_CODE_REMOTE_MEMORY_DIR`, if set.
    pub remote_dir: Option<&'a str>,
    /// `autoMemoryDirectory` from the user's `settings.json`, if set.
    pub settings_dir: Option<&'a str>,
}

/// Where the CLI keeps memory for this session.
///
/// The CLI's own chain is policy → flag → local → project → user settings, and two environment
/// variables above all of that. The engine reads what it can reach from a hook — the
/// environment and the user settings — and falls back to the default, which through the link
/// is inside the store. Guessing the path from `enc` alone would make the store synchronise an
/// empty directory whenever any of these is set.
#[must_use]
pub fn memory_dir(location: &MemoryLocation<'_>, config_dir: &Path, enc: &str) -> PathBuf {
    let chosen = [
        location.cowork_override,
        location.remote_dir,
        location.settings_dir,
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|value| !value.is_empty());
    match chosen {
        Some(path) => PathBuf::from(path),
        None => config_dir.join("projects").join(enc).join(MEMORY_DIR_NAME),
    }
}

/// Reads `autoMemoryDirectory` from a settings file, if the file and the key exist.
#[must_use]
pub fn settings_memory_dir(settings: &Path) -> Option<String> {
    let text = std::fs::read_to_string(settings).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get(SETTINGS_MEMORY_KEY)?.as_str().map(str::to_owned)
}

/// What one `sync` did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Synced {
    /// Events appended to the journal: edits found in the projection.
    pub imported: usize,
    /// Projection files written because their content changed.
    pub written: Vec<String>,
    /// Documents that could not be read, with the reason; their files were left alone.
    pub rejected: Vec<(String, String)>,
    /// Records two machines wrote without seeing each other. The projection shows both; the
    /// person is asked to reconcile them, and this is the list to ask about.
    pub divergent: Vec<String>,
    /// Lines of the journal that could not be read.
    pub unreadable: usize,
}

/// Brings the projection and the journal into agreement, edits first.
///
/// `stamp` and `agent` go into every event this run creates: the core has no clock and no idea
/// who is asking.
///
/// # Errors
///
/// The text of what went wrong with the file system. A journal that cannot be read or written is
/// reported and nothing is projected: writing a projection from a half-read journal would be the
/// one way to lose records.
pub fn sync(
    memory_dir: &Path,
    journal_path: &Path,
    stamp: &str,
    agent: &str,
) -> Result<Synced, String> {
    let mut memory = load(journal_path)?;
    let mut synced = Synced {
        unreadable: memory.unreadable.len(),
        ..Synced::default()
    };

    // 1. Edits first: what the projection holds now becomes versions before anything is written.
    let documents = read_documents(memory_dir)?;
    let import = markdown::import(&documents, &memory, stamp, agent);
    for (name, error) in import.rejected {
        synced.rejected.push((name, error.to_string()));
    }
    if !import.events.is_empty() {
        append(journal_path, &import.events)?;
        synced.imported = import.events.len();
        memory = load(journal_path)?;
    }

    // 2. Then the projection, written only where it differs: an untouched file keeps its mtime,
    // and the CLI is not told the memory changed when it did not.
    let projection = markdown::project(&memory);
    std::fs::create_dir_all(memory_dir).map_err(|error| error.to_string())?;
    for (name, bytes) in &projection.files {
        let path = memory_dir.join(name);
        if std::fs::read(&path).is_ok_and(|current| current == *bytes) {
            continue;
        }
        std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        synced.written.push(name.clone());
    }
    synced.divergent = memory
        .records
        .values()
        .filter(|entry| entry.is_divergent())
        .map(|entry| entry.record.id.as_str().to_owned())
        .collect();
    Ok(synced)
}

/// Reads the journal into its current state. No journal yet is empty memory, not an error.
fn load(journal_path: &Path) -> Result<Memory, String> {
    let bytes = match std::fs::read(journal_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("{}: {error}", journal_path.display())),
    };
    let (events, unreadable) = journal::parse(&bytes);
    Ok(fold(&events, unreadable))
}

/// Appends events to the journal, one line each, creating it if needed.
fn append(journal_path: &Path, events: &[journal::Event]) -> Result<(), String> {
    use std::io::Write;

    if let Some(parent) = journal_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(journal_path)
        .map_err(|error| format!("{}: {error}", journal_path.display()))?;
    for event in events {
        let line = journal::encode(event).map_err(|error| error.to_string())?;
        file.write_all(&line).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The markdown files of the memory directory, by name. A missing directory is an empty one.
fn read_documents(memory_dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let entries = match std::fs::read_dir(memory_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", memory_dir.display())),
    };
    let mut documents = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        // The extension is compared as the CLI writes it; a `.MD` file is somebody else's.
        if entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let bytes = std::fs::read(entry.path()).map_err(|error| error.to_string())?;
        documents.push((name, bytes));
    }
    documents.sort();
    Ok(documents)
}

/// Sets a version aside under the engine directory, never over an existing file.
///
/// Two different store paths can produce the same quarantine name after sanitising (`a/b.md`
/// and `a-b.md`), and a version that was set aside has no right to be overwritten by the next
/// one — so a taken name gets a numbered suffix instead.
///
/// # Errors
///
/// The text of what went wrong.
pub fn quarantine(engine_dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;

    let dir = engine_dir.join(QUARANTINE_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    for attempt in 0u32.. {
        let candidate = if attempt == 0 {
            dir.join(name)
        } else {
            dir.join(format!("{name}.{attempt}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(bytes).map_err(|error| error.to_string())?;
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("no free name in the quarantine directory".to_owned())
}

/// Files waiting in quarantine, for `additionalContext` and `doctor`.
#[must_use]
pub fn quarantined(engine_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(engine_dir.join(QUARANTINE_DIR)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
