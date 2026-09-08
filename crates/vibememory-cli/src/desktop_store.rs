//! Desktop's session cards: finding them on this machine, and moving them through the outbox.
//!
//! The store of cards is a real local directory — Desktop refuses reparse points under its own
//! root — so these files cannot be linked into the git store the way transcripts are. They are
//! copied, in both directions, and every copy passes the guard in the core.
//!
//! The guard exists because a card can lose its transcript here and only here: Desktop clears
//! `cliSessionId` when a resume misses and never puts it back, and it marks
//! `transcriptUnavailable` after looking at *this* disk. Exporting either would spread one
//! machine's bad luck to all of them.

use std::path::{Path, PathBuf};

use vibememory_core::desktop::descriptor::Descriptor;
use vibememory_core::desktop::guard::{
    ExportVerdict, ImportVerdict, MachineFacts, export_verdict, import_verdict,
};
use vibememory_core::desktop::roots::Roots;

/// Prefix of a card file.
const CARD_PREFIX: &str = "local_";
/// Suffix of a card file.
const CARD_SUFFIX: &str = ".json";
/// Where cards live inside the outbox.
pub const OUTBOX_DIR: &str = "desktop";

/// Where Desktop keeps its cards on macOS, under the user's home.
#[must_use]
pub fn default_store(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("Claude")
        .join("claude-code-sessions")
}

/// What one round of card replication did, and what it refused to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cards {
    /// Cards published to the outbox.
    pub exported: Vec<String>,
    /// Cards held back, with the rule that held them.
    pub withheld: Vec<(String, String)>,
    /// Cards written into the local Desktop store.
    pub imported: Vec<String>,
    /// Cards skipped on the way in, with the rule.
    pub skipped: Vec<(String, String)>,
}

/// Publishes this machine's cards into its outbox, translating their directories.
///
/// # Errors
///
/// The text of what went wrong.
pub fn publish(
    desktop_store: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
) -> Result<Cards, String> {
    let mut cards = Cards::default();
    let out = store.join("machines").join(machine_id).join(OUTBOX_DIR);
    for (name, descriptor) in read_cards(desktop_store) {
        let exported = read_card(&out.join(&name));
        let portable = portable_pair(roots, &descriptor);
        match export_verdict(exported.as_ref(), &descriptor, portable) {
            ExportVerdict::Export { descriptor } => {
                write_card(&out.join(&name), &descriptor)?;
                cards.exported.push(name);
            }
            ExportVerdict::Withhold { rule, .. } => {
                cards.withheld.push((name, rule.to_owned()));
            }
        }
    }
    Ok(cards)
}

/// Brings other machines' cards into the local Desktop store, when this machine can actually use
/// them.
///
/// `confirmed` answers, for one transcript id, with the `transcript_path` this machine has proven
/// — a link the reconciler merely predicted is not enough: on a resume miss Desktop erases
/// `cliSessionId` and never restores it.
///
/// # Errors
///
/// The text of what went wrong.
pub fn import(
    desktop_store: &Path,
    store: &Path,
    machine_id: &str,
    roots: &Roots,
    confirmed: &dyn Fn(&str) -> Option<String>,
) -> Result<Cards, String> {
    let mut cards = Cards::default();
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Ok(cards);
    };
    let incoming = incoming_dir(desktop_store);
    for machine in machines.filter_map(Result::ok) {
        if machine.file_name().to_string_lossy() == machine_id {
            continue;
        }
        let dir = machine.path().join(OUTBOX_DIR);
        for (name, descriptor) in read_cards(&dir) {
            let local = local_pair(roots, &descriptor);
            let cwd_exists = local
                .as_ref()
                .map(|(cwd, _)| cwd.clone())
                .filter(|cwd| Path::new(cwd).is_dir());
            let transcript = descriptor.cli_session_id.clone();
            let confirmed_path = transcript.as_deref().and_then(confirmed);
            let facts = MachineFacts {
                cwd_exists: cwd_exists.as_deref(),
                transcript_in_store: transcript
                    .as_ref()
                    .is_some_and(|id| transcript_in_store(store, id)),
                confirmed_transcript_path: confirmed_path.as_deref(),
            };
            match import_verdict(&descriptor, local, facts) {
                ImportVerdict::Import { descriptor } => {
                    write_card(&incoming.join(&name), &descriptor)?;
                    cards.imported.push(name);
                }
                ImportVerdict::Skip { rule, .. } => cards.skipped.push((name, rule.to_owned())),
            }
        }
    }
    Ok(cards)
}

/// Whether the store holds the transcript a card names, under any project.
fn transcript_in_store(store: &Path, cli_session_id: &str) -> bool {
    let Ok(projects) = std::fs::read_dir(store.join("projects")) else {
        return false;
    };
    projects.filter_map(Result::ok).any(|project| {
        project
            .path()
            .join(format!("{cli_session_id}.jsonl"))
            .exists()
    })
}

/// The card's directories in portable form, or `None` when no root covers them.
fn portable_pair(roots: &Roots, descriptor: &Descriptor) -> Option<(String, Option<String>)> {
    let cwd = roots.to_portable(descriptor.cwd.as_deref()?).ok()?;
    let origin = descriptor
        .origin_cwd
        .as_deref()
        .and_then(|path| roots.to_portable(path).ok());
    Some((cwd, origin))
}

/// The card's directories as this machine writes them.
fn local_pair(roots: &Roots, descriptor: &Descriptor) -> Option<(String, Option<String>)> {
    let cwd = roots.to_local(descriptor.cwd.as_deref()?).ok()?;
    let origin = descriptor
        .origin_cwd
        .as_deref()
        .and_then(|path| roots.to_local(path).ok());
    Some((cwd, origin))
}

/// How deep below the store a card may sit. Desktop keeps them two levels down, under the
/// account and the organisation; the outbox keeps them flat. One spare level is there so that a
/// release that adds a level is still read, and nothing walks a whole home directory.
const MAX_CARD_DEPTH: usize = 3;

/// Every card at or below a directory, by file name. A directory that is not there holds none.
///
/// The search is recursive because Desktop does not keep cards where the store begins: they live
/// in `<store>/<account>/<organisation>/`, and a reader that only looks at the top level finds an
/// empty store on every real machine. The outbox is flat, and a flat directory is just the
/// zero-depth case of the same walk.
fn read_cards(dir: &Path) -> Vec<(String, Descriptor)> {
    let mut cards = Vec::new();
    collect_cards(dir, MAX_CARD_DEPTH, &mut cards);
    cards.sort_by(|left, right| left.0.cmp(&right.0));
    cards
}

/// The walk behind `read_cards`, one directory per call.
fn collect_cards(dir: &Path, depth: usize, cards: &mut Vec<(String, Descriptor)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_cards(&path, depth - 1, cards);
            }
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(CARD_PREFIX) || !name.ends_with(CARD_SUFFIX) {
            continue;
        }
        // A card this build cannot parse is left where it is: Desktop's format is undocumented
        // and changes, and refusing to touch what we do not understand costs nothing.
        if let Some(descriptor) = read_card(&path) {
            cards.push((name, descriptor));
        }
    }
}

/// Where an arriving card has to be written for Desktop to see it.
///
/// Not the store's own root: Desktop reads `<store>/<account>/<organisation>/`, and a card in the
/// root is invisible to it. The directory that already holds this machine's cards is the answer,
/// because it is the one Desktop itself chose; when there is none — a machine that has never run
/// Desktop — the root is all we can honestly guess.
fn incoming_dir(desktop_store: &Path) -> PathBuf {
    let mut found = Vec::new();
    collect_card_dirs(desktop_store, MAX_CARD_DEPTH, &mut found);
    found.sort();
    found
        .into_iter()
        .max_by_key(|(count, _)| *count)
        .map_or_else(|| desktop_store.to_path_buf(), |(_, dir)| dir)
}

/// Every directory holding cards, with how many it holds.
fn collect_card_dirs(dir: &Path, depth: usize, found: &mut Vec<(usize, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut here = 0;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_card_dirs(&path, depth - 1, found);
            }
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(CARD_PREFIX) && name.ends_with(CARD_SUFFIX) {
            here += 1;
        }
    }
    if here > 0 {
        found.push((here, dir.to_path_buf()));
    }
}

fn read_card(path: &Path) -> Option<Descriptor> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes a card whole, through a temporary file: Desktop reads these while we write them.
fn write_card(path: &Path, descriptor: &Descriptor) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(descriptor).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}
