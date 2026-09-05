//! `migrate --from <dir>`: bringing the old synced folder into the store, and bringing it again.
//!
//! The source is the `.claude` that lived in a cloud folder: `projects/-ALL-/<repo>` for the
//! transcripts, `claude-code-sessions` for Desktop's cards, `CLAUDE.md`, `settings.json` and
//! `skills`. Three things shape everything below.
//!
//! * **The source is never written to.** Not a byte, not a rename. Another machine is still
//!   working out of it, and the archive is the fallback if anything here is wrong.
//! * **Nothing is read before it is known to be on disk.** A cloud client leaves placeholders
//!   whose read is a network download; a migration that reads them either hangs or imports
//!   truncated files. So the plan checks every file's allocated blocks first, and a single
//!   placeholder stops the apply.
//! * **The import is a merge, so it can be run again.** The other machine keeps writing into the
//!   folder until it migrates itself; re-running this brings those records in by the same union
//!   the tick uses. The folder becomes an inbox, and stays one until nobody writes to it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use vibememory_core::desktop::descriptor::Descriptor;
use vibememory_core::export::{Decision, decide};
use vibememory_core::merge::jsonl::merge_jsonl;
use vibememory_core::merge::keep_both::{KeepBothInput, keep_both};

use crate::git;

/// The shared store of the old scheme, path-independent by construction.
const ALL_DIR: &str = "-ALL-";
/// The one store of the old scheme that stays behind: sessions with `cwd = /`, the runners.
const ARCHIVE_ONLY_STORE: &str = "-";
/// Where the old scheme kept Desktop's cards.
const CARDS_DIR: &str = "claude-code-sessions";
/// Files of the config directory the store keeps as managed copies.
const CONFIG_FILES: &[&str] = &["CLAUDE.md", "settings.json"];
/// The skills directory.
const SKILLS_DIR: &str = "skills";
/// How long a git command of the migration may take: the initial commit of a gigabyte of
/// transcripts is not a hook.
const TIMEOUT: Duration = Duration::from_mins(10);

/// What one source file becomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// Copied as it is: a transcript, a side file, a skill, a config file.
    Copy,
    /// A cloud client's conflict copy of a transcript; merged into the main file by union.
    TranscriptCopy {
        /// The store path of the main file.
        into: String,
    },
    /// A conflict copy of anything else; the main file wins, this one goes to the quarantine.
    OtherCopy {
        /// The store path of the main file.
        of: String,
    },
    /// A Desktop card, into the outbox of the machine it belongs to.
    Card {
        /// The machine whose outbox receives it.
        machine: String,
    },
}

/// One file of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Planned {
    /// Relative to the source root, `/`-separated.
    pub source: String,
    /// Relative to the store, `/`-separated.
    pub dest: String,
    /// What happens to it.
    pub kind: Kind,
    /// Size in bytes.
    pub size: u64,
}

/// Everything the migration would do, computed without reading a single file's content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Files that go in.
    pub files: Vec<Planned>,
    /// Files whose content is not on this disk. One of these stops the apply.
    pub dehydrated: Vec<String>,
    /// Files the export gate refuses, with its rule: they never enter a store.
    pub refused: Vec<(String, String)>,
    /// Files of the archive-only store, left where they are.
    pub archived: usize,
    /// Total bytes that go in.
    pub bytes: u64,
}

/// Computes the plan from the source's metadata alone.
///
/// # Errors
///
/// The text of what could not be listed.
pub fn plan(source: &Path, machine_id: &str) -> Result<Plan, String> {
    let mut plan = Plan::default();

    // 1. Transcripts and everything beside them, per old store.
    let all = source.join("projects").join(ALL_DIR);
    for store_dir in list(&all)? {
        let repo = store_dir.file_name().to_string_lossy().into_owned();
        if repo == ARCHIVE_ONLY_STORE {
            plan.archived += count_files(&store_dir.path());
            continue;
        }
        for file in walk(&store_dir.path())? {
            let relative = relative_to(&file, &store_dir.path());
            let dest = format!("projects/{repo}/{relative}");
            let size = file_size(&file);
            match decide(&dest, Some(size)) {
                Decision::Refuse { rule, .. } => {
                    plan.refused
                        .push((relative_to(&file, source), rule.to_owned()));
                    continue;
                }
                Decision::Export => {}
            }
            let kind = match conflict_copy_of(&file) {
                Some(main) if file.extension().is_some_and(|ext| ext == "jsonl") => {
                    Kind::TranscriptCopy {
                        into: format!("projects/{repo}/{}", relative_to(&main, &store_dir.path())),
                    }
                }
                Some(main) => Kind::OtherCopy {
                    of: format!("projects/{repo}/{}", relative_to(&main, &store_dir.path())),
                },
                None => Kind::Copy,
            };
            push(&mut plan, source, &file, dest, kind, size);
        }
    }

    // 2. Desktop's cards. A conflict copy named after another machine is that machine's
    // version of the card, and goes into that machine's outbox.
    for file in walk(&source.join(CARDS_DIR))? {
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !(name.starts_with("local_") || name.starts_with("deleted_")) {
            continue;
        }
        let (machine, dest_name) = match conflict_copy_of(&file) {
            Some(main) => (
                copy_suffix(&file).unwrap_or_else(|| machine_id.to_owned()),
                main.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(name.clone()),
            ),
            None => (machine_id.to_owned(), name.clone()),
        };
        let dest = format!("machines/{machine}/desktop/{dest_name}");
        let size = file_size(&file);
        push(&mut plan, source, &file, dest, Kind::Card { machine }, size);
    }

    // 3. The config files and the skills.
    for name in CONFIG_FILES {
        let file = source.join(name);
        if file.is_file() {
            let size = file_size(&file);
            push(
                &mut plan,
                source,
                &file,
                format!("config/{name}"),
                Kind::Copy,
                size,
            );
        }
    }
    for file in walk(&source.join(SKILLS_DIR))? {
        let relative = relative_to(&file, &source.join(SKILLS_DIR));
        let dest = format!("config/skills/{relative}");
        let size = file_size(&file);
        match decide(&format!("skills/{relative}"), Some(size)) {
            Decision::Refuse { rule, .. } => {
                plan.refused
                    .push((relative_to(&file, source), rule.to_owned()));
            }
            Decision::Export => push(&mut plan, source, &file, dest, Kind::Copy, size),
        }
    }

    plan.files
        .sort_by(|left, right| left.source.cmp(&right.source));
    plan.dehydrated.sort();
    Ok(plan)
}

/// Adds one file to the plan, or to its list of placeholders.
fn push(plan: &mut Plan, source: &Path, file: &Path, dest: String, kind: Kind, size: u64) {
    let relative = relative_to(file, source);
    if is_dehydrated(file) {
        plan.dehydrated.push(relative);
        return;
    }
    plan.bytes += size;
    plan.files.push(Planned {
        source: relative,
        dest,
        kind,
        size,
    });
}

/// Whether a file is a cloud placeholder: it has a size but no blocks on this disk.
#[cfg(unix)]
fn is_dehydrated(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|m| m.size() > 0 && m.blocks() == 0)
}

#[cfg(not(unix))]
fn is_dehydrated(_path: &Path) -> bool {
    false
}

/// The main file a conflict copy duplicates, when this is one: `<stem>-<Machine>.<ext>` beside
/// `<stem>.<ext>`.
///
/// A machine name may itself contain hyphens (`GPD-WIN-MAX2`), so every hyphen of the stem is
/// tried from the left and the first sibling that exists wins. The suffix must look like a
/// machine name — at least one capital letter — because a memory record called `store-naming.md`
/// beside `store.md` is two records, not a copy, and treating it as one would quarantine a real
/// note.
fn conflict_copy_of(file: &Path) -> Option<PathBuf> {
    let (base, _) = split_copy_name(file)?;
    let ext = file.extension().and_then(|ext| ext.to_str());
    let sibling = match ext {
        Some(ext) => format!("{base}.{ext}"),
        None => base,
    };
    Some(file.with_file_name(sibling))
}

/// The machine name a conflict copy carries in its suffix.
fn copy_suffix(file: &Path) -> Option<String> {
    split_copy_name(file).map(|(_, suffix)| suffix)
}

/// `(base, suffix)` of a conflict copy's stem, or `None` when the file is not one.
fn split_copy_name(file: &Path) -> Option<(String, String)> {
    let stem = file.file_stem()?.to_str()?;
    let ext = file.extension().and_then(|ext| ext.to_str());
    for (index, _) in stem.match_indices('-') {
        let base = &stem[..index];
        let suffix = &stem[index + 1..];
        if base.is_empty() || !suffix.chars().any(|c| c.is_ascii_uppercase()) {
            continue;
        }
        let sibling = match ext {
            Some(ext) => format!("{base}.{ext}"),
            None => base.to_owned(),
        };
        if file.with_file_name(sibling).is_file() {
            return Some((base.to_owned(), suffix.to_owned()));
        }
    }
    None
}

/// What applying the plan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Files written as they were.
    pub copied: usize,
    /// Files already in the store with the same bytes.
    pub unchanged: usize,
    /// Transcripts merged by union, with the records each side contributed.
    pub merged: Vec<(String, usize, usize)>,
    /// Files decided whole by keep-both: the store's version stayed, the source's was set aside
    /// or was identical. Verified by that contract, not by hash — the store's bytes are meant to
    /// differ from the source's here.
    pub kept_both: Vec<String>,
    /// Versions set aside in the quarantine.
    pub quarantined: Vec<String>,
    /// Cards repaired with a transcript id taken from another copy of the same card.
    pub repaired_cards: Vec<String>,
    /// Manifest entries that did not match after writing. One of these is a stop.
    pub mismatched: Vec<String>,
    /// The commit made, if anything changed.
    pub committed: bool,
    /// Store paths this run wrote or merged — what the commit holds.
    pub written: Vec<String>,
}

/// The manifest: the hash of every source file at the moment it was read.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Source path → sha256.
    pub sources: BTreeMap<String, String>,
}

/// Applies the plan into the store and commits.
///
/// Refuses outright when the plan holds placeholders. Verifies every plain copy against the
/// manifest and every merge against its own report, and reports rather than deletes when
/// something does not add up: the source is untouched, so a failed apply costs a re-run.
///
/// # Errors
///
/// The text of what went wrong, including a placeholder in the plan.
pub fn apply(
    source: &Path,
    store: &Path,
    engine_dir: &Path,
    plan: &Plan,
    stamp: &str,
) -> Result<Applied, String> {
    if !plan.dehydrated.is_empty() {
        return Err(format!(
            "{} file(s) are cloud placeholders and are not on this disk; pin the folder first",
            plan.dehydrated.len()
        ));
    }
    let mut run = Run {
        source,
        store,
        engine_dir,
        stamp,
        applied: Applied::default(),
        manifest: Manifest::default(),
        written: Vec::new(),
        cards: BTreeMap::new(),
    };
    run.write_files(plan)?;
    run.write_cards()?;
    run.save_manifest()?;
    run.verify(plan);
    if !run.applied.mismatched.is_empty() {
        return Ok(run.applied);
    }
    if !run.written.is_empty() {
        commit(store, &run.written, stamp)?;
        run.applied.committed = true;
    }
    run.applied.written = run.written;
    Ok(run.applied)
}

/// The state of one apply, carried between its stages.
struct Run<'a> {
    source: &'a Path,
    store: &'a Path,
    engine_dir: &'a Path,
    stamp: &'a str,
    applied: Applied,
    manifest: Manifest,
    /// Store paths written or merged this run, for the commit.
    written: Vec<String>,
    /// Cards by destination, every copy of each, so a copy can repair the card it duplicates.
    cards: BTreeMap<String, Vec<Descriptor>>,
}

impl Run<'_> {
    /// Everything but the cards. Main files go first: a copy merges into a main that must
    /// already be there.
    fn write_files(&mut self, plan: &Plan) -> Result<(), String> {
        let mut ordered: Vec<&Planned> = plan.files.iter().collect();
        ordered.sort_by_key(|planned| {
            matches!(
                planned.kind,
                Kind::TranscriptCopy { .. } | Kind::OtherCopy { .. }
            )
        });
        for planned in ordered {
            let bytes = std::fs::read(self.source.join(&planned.source))
                .map_err(|error| format!("{}: {error}", planned.source))?;
            self.manifest
                .sources
                .insert(planned.source.clone(), crate::sha256::hex(&bytes));
            match &planned.kind {
                Kind::Copy => self.write_copy(planned, &bytes)?,
                Kind::TranscriptCopy { into } | Kind::OtherCopy { of: into } => {
                    let main = self.store.join(into);
                    let changed = merge_into(
                        &main,
                        &bytes,
                        into,
                        self.engine_dir,
                        self.stamp,
                        &mut self.applied,
                    )?;
                    // Only a file that actually changed is staged: a re-run over an unchanged
                    // store must not ask git to commit nothing.
                    if changed {
                        self.written.push(into.clone());
                    }
                }
                Kind::Card { .. } => match serde_json::from_slice::<Descriptor>(&bytes) {
                    Ok(descriptor) => self
                        .cards
                        .entry(planned.dest.clone())
                        .or_default()
                        .push(descriptor),
                    // Not a card this build understands: carried as bytes, never interpreted.
                    Err(_) => self.write_copy(planned, &bytes)?,
                },
            }
        }
        Ok(())
    }

    /// One plain file: written when new, merged when the store already changed it, skipped when
    /// identical.
    fn write_copy(&mut self, planned: &Planned, bytes: &[u8]) -> Result<(), String> {
        let dest = self.store.join(&planned.dest);
        if std::fs::read(&dest).is_ok_and(|current| current == bytes) {
            self.applied.unchanged += 1;
            return Ok(());
        }
        if dest.exists() {
            // A re-run over a file the store changed since: union for transcripts, keep-both for
            // the rest. The store version knows more, and only a real change is staged.
            let changed = merge_into(
                &dest,
                bytes,
                &planned.dest,
                self.engine_dir,
                self.stamp,
                &mut self.applied,
            )?;
            if changed {
                self.written.push(planned.dest.clone());
            }
            return Ok(());
        }
        write(&dest, bytes)?;
        self.applied.copied += 1;
        self.written.push(planned.dest.clone());
        Ok(())
    }

    /// Cards last, and every copy of a card consulted before it is written: a machine whose
    /// Desktop lost the transcript id gets it back from a copy that still has it.
    fn write_cards(&mut self) -> Result<(), String> {
        let mut donors: BTreeMap<String, String> = BTreeMap::new();
        for (dest, versions) in &self.cards {
            let name = card_name(dest);
            if let Some(id) = versions.iter().find_map(|card| card.cli_session_id.clone()) {
                donors.entry(name).or_insert(id);
            }
        }
        let cards = std::mem::take(&mut self.cards);
        for (dest, versions) in cards {
            let name = card_name(&dest);
            for descriptor in versions {
                let mut card = descriptor;
                if (card.cli_session_id.is_none() || card.is_transcript_unavailable())
                    && let Some(id) = donors.get(&name)
                {
                    card = card.with_transcript(id.clone());
                    self.applied.repaired_cards.push(name.clone());
                }
                let text =
                    serde_json::to_string_pretty(&card).map_err(|error| error.to_string())?;
                let path = self.store.join(&dest);
                if std::fs::read(&path).is_ok_and(|current| current == text.as_bytes()) {
                    self.applied.unchanged += 1;
                    continue;
                }
                write(&path, text.as_bytes())?;
                self.applied.copied += 1;
                self.written.push(dest.clone());
            }
        }
        Ok(())
    }

    /// The manifest is kept beside the engine, not in the store: it is this machine's record of
    /// what it read, and a second machine's migration writes its own.
    fn save_manifest(&self) -> Result<(), String> {
        std::fs::create_dir_all(self.engine_dir).map_err(|error| error.to_string())?;
        let text =
            serde_json::to_string_pretty(&self.manifest).map_err(|error| error.to_string())?;
        std::fs::write(self.engine_dir.join("migrate-manifest.json"), text)
            .map_err(|error| error.to_string())
    }

    /// Every plain copy must hash to what its source hashed to. A file the store merged is
    /// checked by the merge's own contract instead, and a transcript that already holds every
    /// line of the source is an earlier run's merge, not a mismatch.
    fn verify(&mut self, plan: &Plan) {
        for planned in &plan.files {
            if !matches!(planned.kind, Kind::Copy) {
                continue;
            }
            if self
                .applied
                .merged
                .iter()
                .any(|(path, _, _)| *path == planned.dest)
                || self.applied.kept_both.contains(&planned.dest)
            {
                continue;
            }
            let Some(expected) = self.manifest.sources.get(&planned.source) else {
                continue;
            };
            let actual = std::fs::read(self.store.join(&planned.dest))
                .map_or_else(|_| String::new(), |bytes| crate::sha256::hex(&bytes));
            if actual != *expected
                && !dest_is_union_of(self.store, &planned.dest, self.source, &planned.source)
            {
                self.applied.mismatched.push(planned.dest.clone());
            }
        }
    }
}

/// The file name of a card, which is its identity across machines' outboxes.
fn card_name(dest: &str) -> String {
    Path::new(dest)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Merges incoming bytes into a file the store already holds: union for a transcript, keep-both
/// with the loser quarantined for anything else. Writes only when the result differs.
fn merge_into(
    main: &Path,
    incoming: &[u8],
    store_path: &str,
    engine_dir: &Path,
    stamp: &str,
    applied: &mut Applied,
) -> Result<bool, String> {
    let current = std::fs::read(main).unwrap_or_default();
    if current == incoming {
        applied.unchanged += 1;
        return Ok(false);
    }
    if main.extension().is_some_and(|ext| ext == "jsonl") {
        // No base: the cloud client never had one, so every record either side holds stays.
        let merged = merge_jsonl(&[], &current, incoming).map_err(|error| error.to_string())?;
        if merged.report.ours.dropped > 0 || merged.report.theirs.dropped > 0 {
            return Err(format!(
                "{store_path}: the union dropped records, which it never may"
            ));
        }
        let changed = merged.bytes != current;
        if changed {
            write(main, &merged.bytes)?;
        }
        applied.merged.push((
            store_path.to_owned(),
            merged.report.ours.added,
            merged.report.theirs.added,
        ));
        return Ok(changed);
    }
    let outcome = keep_both(&KeepBothInput {
        path: store_path,
        base: &[],
        ours: &current,
        theirs: incoming,
        stamp,
    });
    let changed = outcome.bytes != current;
    if changed {
        write(main, &outcome.bytes)?;
    }
    if let Some(set_aside) = outcome.quarantined {
        let known = crate::memory::quarantined(engine_dir);
        let path = crate::memory::quarantine(engine_dir, &set_aside.name, &set_aside.bytes)?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // A version the quarantine already held is nothing new, and is not counted as work.
        if !known.contains(&name) {
            applied.quarantined.push(path.display().to_string());
        }
    }
    if changed {
        applied.copied += 1;
    }
    applied.kept_both.push(store_path.to_owned());
    Ok(changed)
}

/// For a re-run: a transcript in the store that already holds every line of the source is not a
/// mismatch, it is a merge that happened on an earlier run.
fn dest_is_union_of(store: &Path, dest: &str, source_root: &Path, source: &str) -> bool {
    if Path::new(dest).extension().is_none_or(|ext| ext != "jsonl") {
        return false;
    }
    let (Ok(have), Ok(want)) = (
        std::fs::read(store.join(dest)),
        std::fs::read(source_root.join(source)),
    ) else {
        return false;
    };
    let have_lines: std::collections::HashSet<&[u8]> = have.split(|b| *b == b'\n').collect();
    want.split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .all(|line| have_lines.contains(line))
}

/// Stages exactly the written paths and commits: the store may hold a live file or two by the
/// time a re-run happens, and `git add -A` would sweep them in mid-write.
fn commit(store: &Path, written: &[String], stamp: &str) -> Result<(), String> {
    for chunk in written.chunks(200) {
        let mut args: Vec<&str> = vec!["add", "--"];
        args.extend(chunk.iter().map(String::as_str));
        git::run_with_timeout(git::command(store, &args), TIMEOUT)?
            .ok_or_else(|| "git refused to stage the migrated files".to_owned())?;
    }
    let message = format!("vibememory: migrate {} file(s) at {stamp}", written.len());
    git::run_with_timeout(
        git::command(store, &["commit", "--quiet", "-m", &message]),
        TIMEOUT,
    )?
    .ok_or_else(|| {
        format!(
            "git refused to commit the migration; staged: {}",
            written.join(", ")
        )
    })?;
    Ok(())
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("vibememory-migrate");
    std::fs::write(&temporary, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn list(dir: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut found: Vec<std::fs::DirEntry> = entries.filter_map(Result::ok).collect();
            found.sort_by_key(std::fs::DirEntry::file_name);
            Ok(found)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("{}: {error}", dir.display())),
    }
}

/// Every regular file under a directory, sorted. Symlinks are not followed: a link inside the
/// old folder is a flattened text file at best and a loop at worst.
fn walk(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in list(&current)? {
            let path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn count_files(dir: &Path) -> usize {
    walk(dir).map_or(0, |files| files.len())
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn relative_to(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
