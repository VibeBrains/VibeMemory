//! Files kept on this machine because they hold an agent token.
//!
//! A transcript is the agent's raw output: a settings file a tool read comes back into it whole,
//! and so does a token a person pasted — into the prompt history, a memory file, a hand-off, a
//! skill. Committed, it would sit in the store's history, on the host and in its mirror for good —
//! in a team's store, in front of every member. So no such file is committed: it stays in the
//! store's working tree on this machine, a task file and a line of the prompt history stay out of
//! the outbox, the next session hears of it once, and `doctor` names it with the token's public id
//! for as long as it is held. Only the token's public id is ever written down — never the secret.
//!
//! One file is not screened here: a project's memory journal. It only grows, so one held line would
//! stop the project's memory for good; its records are refused with a token when they are written
//! (`holdsToken`). A text holds a token when it holds one shaped `vmt_…` (`vibememory_core::token`)
//! or the very value of a token this machine keeps ([`Kept`]) — the owner's token from before the
//! cabinet has no shape to recognise.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use vibememory_core::token::token_ids;

use crate::connect::{FRAGMENT_SUFFIX, SIDECAR_EXTENSION, TOKENS_DIR};

/// What is held, in the engine's own directory: the list names files of this machine's store.
const HELD_FILE: &str = "held.json";
/// A kept value shorter than this is no token of ours, and matching it would hold ordinary text.
const MIN_KEPT_TOKEN_BYTES: usize = 32;

/// The tokens this machine keeps, by value: `connect`'s files under `tokens/<team>/<agent>` of the
/// engine's directory, and the owner's token from before the cabinet once it is kept there too.
/// Each is named by the public id its sidecar gives, or by where it is kept — never by its value.
#[derive(Default)]
pub struct Kept(Vec<(String, Vec<u8>)>);

impl Kept {
    /// The tokens kept under `engine_dir`; a machine without any keeps none.
    #[must_use]
    pub fn read(engine_dir: &Path) -> Self {
        let mut kept = Vec::new();
        let Ok(teams) = std::fs::read_dir(engine_dir.join(TOKENS_DIR)) else {
            return Self(kept);
        };
        for team in teams
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
        {
            let Ok(files) = std::fs::read_dir(team.path()) else {
                continue;
            };
            for file in files.filter_map(Result::ok) {
                let path = file.path();
                let name = file.file_name().to_string_lossy().into_owned();
                if name.ends_with(FRAGMENT_SUFFIX)
                    || path
                        .extension()
                        .is_some_and(|extension| extension == SIDECAR_EXTENSION)
                    || !path.is_file()
                {
                    continue;
                }
                let Ok(value) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let value = value.trim();
                if value.len() < MIN_KEPT_TOKEN_BYTES {
                    continue;
                }
                let public = std::fs::read_to_string(crate::connect::sidecar_file(&path))
                    .ok()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                    .and_then(|sidecar| {
                        sidecar
                            .get("tokenId")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    });
                let label = public
                    .unwrap_or_else(|| format!("{}/{name}", team.file_name().to_string_lossy()));
                kept.push((label, value.as_bytes().to_vec()));
            }
        }
        Self(kept)
    }

    /// The names of the tokens `bytes` holds: shaped `vmt_…`, or the value of one kept here.
    #[must_use]
    pub fn found_in(&self, bytes: &[u8]) -> BTreeSet<String> {
        let mut found = token_ids(bytes);
        for (label, value) in &self.0 {
            if bytes
                .windows(value.len())
                .any(|window| window == value.as_slice())
            {
                found.insert(label.clone());
            }
        }
        found
    }
}

/// The session files this machine keeps back.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Held {
    /// Store path → why it is kept back.
    pub files: BTreeMap<String, HeldFile>,
}

/// One file kept back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HeldFile {
    /// When it was first kept back.
    pub since: String,
    /// The public ids (`tk_<id>`) of the tokens it holds.
    pub tokens: BTreeSet<String>,
    /// The file as it was when last read: an unchanged file is not read again.
    pub seen: Option<Seen>,
}

/// Size and modification time of a file, enough to tell that it has not changed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Seen {
    /// Bytes.
    pub size: u64,
    /// Nanoseconds since the epoch.
    pub modified: u128,
}

impl Seen {
    /// The file at `path` as it is now; `None` when it cannot be told.
    fn of(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            size: metadata.len(),
            modified,
        })
    }
}

impl Held {
    /// What is held; a machine that never held anything has an empty list.
    #[must_use]
    pub fn read(engine_dir: &Path) -> Self {
        std::fs::read_to_string(engine_dir.join(HELD_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write(&self, engine_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(engine_dir.join(HELD_FILE), text).map_err(|error| error.to_string())
    }
}

/// The sentence a session hears once, when a file is first kept back.
fn note(path: &str, tokens: &BTreeSet<String>) -> String {
    let named: Vec<&str> = tokens.iter().map(String::as_str).collect();
    format!(
        "VibeMemory: {path} holds agent token {}, and what holds it stays on this machine, not \
         synced. Revoke it in the cabinet and connect the agent again; `vibememory doctor` lists \
         what is held.",
        named.join(", ")
    )
}

/// Records that `path` is kept back for `tokens`, seen as `seen`. A file newly held is announced
/// to the next session.
fn hold(
    engine_dir: &Path,
    held: &mut Held,
    path: &str,
    tokens: BTreeSet<String>,
    seen: Option<Seen>,
    stamp: &str,
) -> Result<(), String> {
    let since = held
        .files
        .get(path)
        .map_or_else(|| stamp.to_owned(), |file| file.since.clone());
    let fresh = held
        .files
        .get(path)
        .is_none_or(|file| file.tokens != tokens);
    if fresh {
        crate::merge_report::add_pending(engine_dir, &note(path, &tokens))?;
    }
    held.files.insert(
        path.to_owned(),
        HeldFile {
            since,
            tokens,
            seen,
        },
    );
    Ok(())
}

/// Whether `path` is a project's memory journal, `projects/<name>/memory.jsonl`.
fn is_memory_journal(path: &str) -> bool {
    let mut segments = path.split('/');
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (
            Some("projects"),
            Some(_),
            Some(crate::memory::JOURNAL_FILE),
            None
        )
    )
}

/// A file of the store as git records it: its bytes and its mode.
pub struct Content {
    /// What git stores: the file's bytes, or a link's target.
    pub bytes: Vec<u8>,
    /// `100644`, `100755` or `120000`.
    pub mode: &'static str,
}

impl Content {
    /// The file at `path` as git would record it, without following a link; `None` when it is gone.
    fn read(path: &Path) -> Result<Option<Self>, String> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        if metadata.file_type().is_symlink() {
            let target =
                std::fs::read_link(path).map_err(|error| format!("{}: {error}", path.display()))?;
            return Ok(Some(Self {
                bytes: target.to_string_lossy().replace('\\', "/").into_bytes(),
                mode: LINK_MODE,
            }));
        }
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Some(Self {
            bytes,
            mode: if is_executable(&metadata) {
                EXECUTABLE_MODE
            } else {
                FILE_MODE
            },
        }))
    }
}

/// git's mode of a plain file.
const FILE_MODE: &str = "100644";
/// git's mode of an executable file.
const EXECUTABLE_MODE: &str = "100755";
/// git's mode of a symbolic link.
const LINK_MODE: &str = "120000";

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// Of `paths` — store-relative, `/`-separated, about to be committed — the ones that may be: a file
/// that holds a token is kept back and recorded, one that no longer does is released. A memory
/// journal passes as it is. `passed` is told each path that may be committed, with the content
/// that was read for the decision, or `None` for a file that is gone.
///
/// # Errors
///
/// A file that cannot be read, a list that cannot be written, or what `passed` says.
pub fn screen_each(
    engine_dir: &Path,
    store: &Path,
    paths: &[String],
    stamp: &str,
    mut passed: impl FnMut(&str, Option<&Content>) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let mut held = Held::read(engine_dir);
    let before = held.clone();
    let kept = Kept::read(engine_dir);
    let mut committed = Vec::new();
    for path in paths {
        let file = store.join(path);
        if is_memory_journal(path) {
            passed(path, Content::read(&file)?.as_ref())?;
            committed.push(path.clone());
            continue;
        }
        let seen = Seen::of(&file);
        if seen.is_some() && held.files.get(path).and_then(|held| held.seen) == seen {
            continue;
        }
        let Some(content) = Content::read(&file)? else {
            // A file gone since it was listed has nothing to commit and nothing to hold.
            held.files.remove(path);
            passed(path, None)?;
            committed.push(path.clone());
            continue;
        };
        let tokens = kept.found_in(&content.bytes);
        if tokens.is_empty() {
            held.files.remove(path);
            passed(path, Some(&content))?;
            committed.push(path.clone());
        } else {
            hold(engine_dir, &mut held, path, tokens, seen, stamp)?;
        }
    }
    if held != before {
        held.write(engine_dir)?;
    }
    Ok(committed)
}

/// Of `paths`, the ones that may be committed (`screen_each`), without staging anything.
///
/// # Errors
///
/// A file that cannot be read, or a list that cannot be written.
pub fn screen(
    engine_dir: &Path,
    store: &Path,
    paths: &[String],
    stamp: &str,
) -> Result<Vec<String>, String> {
    screen_each(engine_dir, store, paths, stamp, |_, _| Ok(()))
}

/// Stages every path of `paths` that may be committed (`screen_each`) with exactly the bytes that
/// were checked: into the object database by `hash-object -w --stdin`, then into the index. A file
/// written again between the check and `git add` would otherwise go in unchecked. A file that is
/// gone leaves the index. Returns the staged paths; committing them is the caller's.
///
/// # Errors
///
/// A file that cannot be read, or git that refused.
pub fn stage(
    engine_dir: &Path,
    store: &Path,
    paths: &[String],
    stamp: &str,
    timeout: std::time::Duration,
) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    let mut gone = Vec::new();
    let staged = screen_each(engine_dir, store, paths, stamp, |path, content| {
        let Some(content) = content else {
            gone.extend_from_slice(path.as_bytes());
            gone.push(0);
            return Ok(());
        };
        let blob = crate::git::run_with_input(
            crate::git::command(store, &["hash-object", "-w", "--stdin"]),
            &content.bytes,
            timeout,
        )?
        .ok_or_else(|| format!("git refused to store {path}"))?;
        entries.extend_from_slice(format!("{} {blob}\t{path}", content.mode).as_bytes());
        entries.push(0);
        Ok(())
    })?;
    if !entries.is_empty() {
        crate::git::run_with_input(
            crate::git::command(store, &["update-index", "-z", "--index-info"]),
            &entries,
            timeout,
        )?
        .ok_or_else(|| "git refused to index what was checked".to_owned())?;
    }
    if !gone.is_empty() {
        crate::git::run_with_input(
            crate::git::command(store, &["update-index", "--force-remove", "-z", "--stdin"]),
            &gone,
            timeout,
        )?
        .ok_or_else(|| "git refused to take gone files out of the index".to_owned())?;
    }
    Ok(staged)
}

/// Records that `path` is kept back for `tokens`, found by a caller that read it itself: the `Stop`
/// hook in a live transcript cut at the last newline, the outbox in the prompt history or a task
/// file.
///
/// # Errors
///
/// The list cannot be written.
pub fn hold_found(
    engine_dir: &Path,
    path: &str,
    tokens: BTreeSet<String>,
    stamp: &str,
) -> Result<(), String> {
    let mut held = Held::read(engine_dir);
    let before = held.clone();
    // The live file keeps growing: the tick reads it again rather than trust a size of a moment.
    hold(engine_dir, &mut held, path, tokens, None, stamp)?;
    if held != before {
        held.write(engine_dir)?;
    }
    Ok(())
}

/// Forgets `path`: it went out as usual, so whatever held it is gone.
///
/// # Errors
///
/// The list cannot be written.
pub fn release(engine_dir: &Path, path: &str) -> Result<(), String> {
    let mut held = Held::read(engine_dir);
    if held.files.remove(path).is_some() {
        held.write(engine_dir)?;
    }
    Ok(())
}
