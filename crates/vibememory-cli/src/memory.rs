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
    /// Projection files removed because their record was forgotten and nobody had edited them.
    pub removed: Vec<String>,
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
/// who is asking. An edit that holds the value of a token this machine keeps (`kept`) is not
/// imported: the core refuses a token of the cabinet by its shape, but the owner's token from
/// before the cabinet has none, and the journal is committed as it is.
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
    kept: &crate::held::Kept,
) -> Result<Synced, String> {
    let mut memory = load(journal_path)?;
    let mut synced = Synced {
        unreadable: memory.unreadable.len(),
        ..Synced::default()
    };

    // 1. Edits first: what the projection holds now becomes versions before anything is written.
    let documents = read_documents(memory_dir)?;
    // The engine writes as the store's owner, and a version without a member is the owner's: a
    // personal store has one author.
    let mut import = markdown::import(&documents, &memory, stamp, agent, None);
    // The file never says which project it belongs to — it does not have to, it lives inside one.
    // A record does have to say: over MCP it arrives without a path, and "which project is this
    // memory about" then has no other answer.
    let project = journal_path
        .parent()
        .and_then(std::path::Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    for event in &mut import.events {
        if let journal::Action::Upsert { record } = &mut event.action
            && record.project.is_empty()
        {
            record.project.clone_from(&project);
        }
    }
    // Untouched files the projection no longer holds: a forgotten record's document, which left
    // in place the next run would read as a new record, and rival files of settled rivals.
    for name in import.stale {
        let path = memory_dir.join(&name);
        match std::fs::remove_file(&path) {
            Ok(()) => synced.removed.push(name),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    for (name, error) in import.rejected {
        synced.rejected.push((name, error.to_string()));
    }
    let mut holding = Vec::new();
    import.events.retain(|event| {
        let found = journal::encode(event)
            .map(|line| kept.found_in(&line))
            .unwrap_or_default();
        if found.is_empty() {
            return true;
        }
        let name = event.id().as_str().to_owned();
        let named: Vec<&str> = found.iter().map(String::as_str).collect();
        holding.push((
            name,
            format!(
                "holds agent token {}, kept on this machine; save it without the token",
                named.join(", ")
            ),
        ));
        false
    });
    synced.rejected.extend(holding);
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
    // A version already waiting here is not set aside a second time: the migration is re-run
    // while the other machine still writes, and every run would otherwise add one more copy of
    // the same file for the person to sort through.
    if let Some(existing) = already_quarantined(&dir, bytes) {
        return Ok(existing);
    }
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

/// The quarantined file holding exactly these bytes, if one does.
fn already_quarantined(dir: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| std::fs::read(path).is_ok_and(|held| held == bytes))
}

/// What a file set aside in the quarantine is, judged by its name.
///
/// The quarantine started as the memory's, and for a while everything in it was a memory record.
/// It is not any more — a folded Desktop card lands here too — and a notice that calls a card a
/// "memory file" sends the reader looking for a conflict that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Quarantined {
    /// A version of a memory record: two machines wrote the same file and neither wins.
    Memory,
    /// A transcript kept back rather than merged.
    Transcript,
    /// A Desktop session card: a cloud client's conflict copy, folded into the card it duplicated.
    Card,
    /// Something this build does not recognise; named, never guessed about.
    Other,
}

impl Quarantined {
    /// What the notice calls one of these, in the plural.
    #[must_use]
    pub const fn plural(self) -> &'static str {
        match self {
            Self::Memory => "version(s) of memory files",
            Self::Transcript => "transcript(s)",
            Self::Card => "Desktop session card(s)",
            Self::Other => "file(s)",
        }
    }

    /// What the reader is supposed to do about it.
    #[must_use]
    pub const fn what_to_do(self) -> &'static str {
        match self {
            Self::Memory => "reconcile them with the versions in the store",
            Self::Transcript => "compare them with the store's copies",
            // A folded copy has already given the card it duplicated everything it knew, so
            // nothing is pending here: the file is kept only because it is another machine's
            // version and deleting it is a person's call.
            Self::Card => "delete them once you no longer want another machine's copy",
            Self::Other => "look at them",
        }
    }
}

/// What a quarantined file is, by name.
#[must_use]
pub fn quarantined_kind(name: &str) -> Quarantined {
    // The quarantine appends a stamp, so the type has to be read from the name's body rather than
    // from its ending.
    if name.starts_with("local_") || name.starts_with("deleted_") {
        Quarantined::Card
    } else if name.contains(".jsonl") {
        Quarantined::Transcript
    } else if name.contains(".md") {
        Quarantined::Memory
    } else {
        Quarantined::Other
    }
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

/// Open hand-offs of a project: files under `memory/sessions/` whose frontmatter says
/// `status: open`.
///
/// The index in `MEMORY.md` cannot be relied on for this. Observed twice on 2026-09-08: the file
/// is rewritten back to the CLI's default template (`# Memory` / `Nothing remembered yet.`)
/// minutes after a session writes an index into it. Whatever does that, the engine is not going
/// to fight it — it reads the hand-off files themselves, which nothing else touches.
#[must_use]
pub fn open_handoffs(memory_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(memory_dir.join("sessions")) else {
        return Vec::new();
    };
    let mut open = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Frontmatter only: a `status: open` deep in the prose is prose.
        let front = text.split("---").nth(1).unwrap_or_default();
        if front.lines().any(|line| line.trim() == "status: open")
            && let Some(name) = path.file_stem()
        {
            open.push(name.to_string_lossy().into_owned());
        }
    }
    open.sort();
    open
}
