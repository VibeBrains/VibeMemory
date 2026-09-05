//! Reading and writing `machines/<id>/links.json` — this machine's half of the link map.
//!
//! Only this machine writes its own file, so there is nothing to merge and no conflict to have.
//! Everything careful about links lives in what the records *say*: a record written because a
//! session actually started here is `observed` and carries the `transcript_path` that proved it;
//! a record invented by a reconciler from somebody else's file is `predicted`, and stays marked
//! that way until a session confirms it.

use std::path::{Path, PathBuf};

use vibememory_core::links::{LINKS_VERSION, LinkRecord, LinkSource, LinksFile};
use vibememory_core::naming::PathSyntax;

/// The file's name inside a machine's directory.
pub const LINKS_FILE: &str = "links.json";

/// Where one machine's file lives.
#[must_use]
pub fn path(store: &Path, machine_id: &str) -> PathBuf {
    store.join("machines").join(machine_id).join(LINKS_FILE)
}

/// Reads one machine's file. Anything unreadable is an empty list: a broken file may not make the
/// engine forget links it already created, and it will be rewritten from what this machine knows.
#[must_use]
pub fn read(store: &Path, machine_id: &str) -> LinksFile {
    std::fs::read_to_string(path(store, machine_id))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| LinksFile {
            version: LINKS_VERSION,
            links: Vec::new(),
        })
}

/// Every machine's records, with the machine each came from.
#[must_use]
pub fn read_all(store: &Path) -> Vec<(String, LinkRecord)> {
    let Ok(machines) = std::fs::read_dir(store.join("machines")) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for machine in machines.filter_map(Result::ok) {
        let name = machine.file_name().to_string_lossy().into_owned();
        for record in read(store, &name).links {
            found.push((name.clone(), record));
        }
    }
    found
}

/// What this machine observed about one link.
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    /// The encoded directory name, as the CLI writes it.
    pub enc: &'a str,
    /// The store directory it points at.
    pub name: &'a str,
    /// The working directory in portable form.
    pub cwd: &'a str,
    /// The syntax of this machine's paths.
    pub syntax: PathSyntax,
    /// How the link came to be known.
    pub source: LinkSource,
    /// The `transcript_path` that proved it, when a session provided one.
    pub confirmed_by: Option<&'a str>,
}

/// Records what this machine knows about one link, replacing whatever it said before about the
/// same encoded directory.
///
/// A record that has been confirmed keeps its confirmation: a later run that cannot see the
/// transcript path must not turn a proven link back into a guess.
///
/// # Errors
///
/// The text of what went wrong.
pub fn record(store: &Path, machine_id: &str, observation: &Observation<'_>) -> Result<(), String> {
    let &Observation {
        enc,
        name,
        cwd,
        syntax,
        source,
        confirmed_by,
    } = observation;
    let file_path = path(store, machine_id);
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = read(store, machine_id);
    let previous = file.links.iter().find(|record| record.enc == enc).cloned();
    let confirmed = confirmed_by.map(str::to_owned).or_else(|| {
        previous
            .as_ref()
            .and_then(|record| record.confirmed_by.clone())
    });
    // Once observed, always observed: a reconciler's later guess about the same directory says
    // less than a session that actually ran here.
    let source = match previous {
        Some(record) if record.source == LinkSource::Observed => LinkSource::Observed,
        _ => source,
    };

    file.version = LINKS_VERSION;
    file.links.retain(|record| record.enc != enc);
    file.links.push(LinkRecord {
        enc: enc.to_owned(),
        name: name.to_owned(),
        cwd: cwd.to_owned(),
        syntax,
        source,
        predicted: source != LinkSource::Observed && confirmed.is_none(),
        confirmed_by: confirmed,
    });
    file.links.sort_by(|left, right| left.enc.cmp(&right.enc));

    let text = serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?;
    let temporary = file_path.with_extension("json.tmp");
    std::fs::write(&temporary, text.as_bytes()).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &file_path).map_err(|error| error.to_string())
}
