//! The projection: records written out as the markdown Claude Code already reads, and the edits
//! to those files read back as records.
//!
//! The model writes memory as files and will go on doing so; the engine meets it there. Every
//! document carries the version it was projected from, so an edit knows what it was based on and
//! the journal can tell "written after" from "written at the same time" without consulting a
//! clock that belongs to another machine.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::error::MemoryError;
use super::journal::{Action, Entry, Event, Memory};
use super::record::{Record, RecordId, RecordKind, RecordStatus};

/// The index every memory directory has.
pub const INDEX_FILE: &str = "MEMORY.md";
/// Extension of a projected record.
const DOCUMENT_EXTENSION: &str = "md";
/// What sets a rival file apart from the document of its record: `<id>.rival-<version>.md`.
const RIVAL_MARKER: &str = ".rival-";
/// Keys of `metadata` this format names itself; everything else is carried untouched.
const KNOWN_METADATA: &[&str] = &[
    "type", "project", "status", "agent", "member", "created", "updated", "version", "merged",
];

/// Fence around the frontmatter block.
const FRONTMATTER_FENCE: &str = "---";
/// Indent of a nested frontmatter key.
const NESTED_INDENT: &str = "  ";

/// The files a memory directory should contain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// File name and content, sorted by name, index first.
    pub files: Vec<(String, Vec<u8>)>,
}

impl Projection {
    /// The names the directory should hold; anything else in it is stale or hand-written.
    #[must_use]
    pub fn file_names(&self) -> Vec<&str> {
        self.files.iter().map(|(name, _)| name.as_str()).collect()
    }
}

/// Writes the memory out as files.
///
/// Records that two machines wrote at once get a second document beside the first, and the index
/// says so: the engine never picks a winner for the person, it shows both and asks.
#[must_use]
pub fn project(memory: &Memory) -> Projection {
    let mut files = vec![(INDEX_FILE.to_owned(), index(memory))];
    for entry in memory.records.values() {
        files.push((
            document_name(&entry.record.id),
            document(&entry.record, &entry.version),
        ));
        for (version, rival) in &entry.rivals {
            files.push((
                rival_name(&entry.record.id, version),
                document(rival, version),
            ));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Projection { files }
}

/// `<id>.md`
#[must_use]
pub fn document_name(id: &RecordId) -> String {
    format!("{}.{DOCUMENT_EXTENSION}", id.as_str())
}

/// `<id>.rival-<version>.md` — a version written at the same time as the one in `<id>.md`.
fn rival_name(id: &RecordId, version: &str) -> String {
    format!(
        "{}{RIVAL_MARKER}{version}.{DOCUMENT_EXTENSION}",
        id.as_str()
    )
}

fn index(memory: &Memory) -> Vec<u8> {
    let mut text = String::from("# Memory\n\n");
    if memory.records.is_empty() {
        text.push_str("Nothing remembered yet.\n");
        return text.into_bytes();
    }
    text.push_str("One line per memory; the file holds the whole of it.\n\n");
    for entry in memory.records.values() {
        let record = &entry.record;
        // A stale record is marked where the reader actually looks. Leaving the mark only inside
        // the document would let anyone scanning the index rely on a fact already known to be
        // out of date — which is the whole failure this status exists to prevent.
        let mark = if record.status == RecordStatus::Stale {
            "**stale**, "
        } else {
            ""
        };
        let _ = writeln!(
            text,
            "- [{}]({}) — {mark}{}",
            record.title(),
            document_name(&record.id),
            record.description
        );
    }
    let divergent: Vec<&Entry> = memory
        .records
        .values()
        .filter(|entry| entry.is_divergent())
        .collect();
    if !divergent.is_empty() {
        text.push_str("\n## Written on two machines at once\n\n");
        text.push_str(
            "Both versions are kept. To settle one, write the merged text into the memory's own \
             file and add the line shown under its `metadata`; the rival file then goes away by \
             itself.\n\n",
        );
        for entry in divergent {
            for (version, _) in &entry.rivals {
                let _ = writeln!(
                    text,
                    "- [{}]({}) also exists as [{}]({}) — `merged: {}`",
                    entry.record.title(),
                    document_name(&entry.record.id),
                    version,
                    rival_name(&entry.record.id, version),
                    version
                );
            }
        }
    }
    text.into_bytes()
}

fn document(record: &Record, version: &str) -> Vec<u8> {
    let mut text = String::with_capacity(record.body.len() + 256);
    text.push_str(FRONTMATTER_FENCE);
    text.push('\n');
    let _ = writeln!(text, "name: {}", record.id.as_str());
    let _ = writeln!(text, "description: {}", record.description);
    text.push_str("metadata:\n");
    // `status` only when it is not the default: writing `status: active` into every document
    // would rewrite every projection on the machine the day this field appeared, and say nothing.
    let status = (!record.status.is_default()).then(|| ("status", record.status.as_str()));
    // What the CLI wrote and this format does not interpret goes back first, in its own order:
    // dropping it would mean the engine silently deletes fields somebody else owns.
    for (key, value) in &record.metadata {
        let _ = writeln!(text, "{NESTED_INDENT}{key}: {value}");
    }
    for (key, value) in [
        Some(("type", record.kind.as_str())),
        Some(("project", record.project.as_str())),
        status,
        Some(("agent", record.agent.as_str())),
        record.member.as_deref().map(|member| ("member", member)),
        Some(("created", record.created_at.as_str())),
        Some(("updated", record.updated_at.as_str())),
        Some(("version", version)),
    ]
    .into_iter()
    .flatten()
    {
        let _ = writeln!(text, "{NESTED_INDENT}{key}: {value}");
    }
    text.push_str(FRONTMATTER_FENCE);
    text.push_str("\n\n");
    text.push_str(record.body.trim_end());
    text.push('\n');
    text.into_bytes()
}

/// A document as read back from disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// The record it describes.
    pub record: Record,
    /// The version it was projected from, when it still says so.
    pub version: Option<String>,
    /// Rival versions the writer merged into it (`merged: <version>, <version>`).
    pub merged: Vec<String>,
}

/// Reads a projected document. Unknown frontmatter keys are kept out of the way rather than
/// rejected: the model edits these files, and a stray key is not a reason to lose a memory.
pub fn parse_document(path: &str, bytes: &[u8]) -> Result<Document, MemoryError> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(FRONTMATTER_FENCE) {
        return Err(MemoryError::MissingFrontmatter {
            path: path.to_owned(),
        });
    }
    let mut top: BTreeMap<String, String> = BTreeMap::new();
    let mut nested: BTreeMap<String, String> = BTreeMap::new();
    let mut in_metadata = false;
    let mut body = String::new();
    let mut in_body = false;
    for line in lines {
        if !in_body {
            if line.trim() == FRONTMATTER_FENCE {
                in_body = true;
                continue;
            }
            let indented = line.starts_with(NESTED_INDENT);
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let (key, value) = (key.trim().to_owned(), value.trim().to_owned());
            if indented && in_metadata {
                nested.insert(key, value);
            } else {
                in_metadata = key == "metadata";
                top.insert(key, value);
            }
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    let required = |map: &BTreeMap<String, String>, field: &'static str| {
        map.get(field)
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or(MemoryError::MissingField {
                path: path.to_owned(),
                field,
            })
    };
    let id = RecordId::parse(&required(&top, "name")?)?;
    let status = match nested.get("status") {
        Some(word) => RecordStatus::parse(word)?,
        None => RecordStatus::default(),
    };
    // Everything in `metadata` this format does not name is carried, not dropped: the real files
    // hold `node_type`, `originSessionId` and `modified`, written by the CLI, and they have to
    // survive the projection being rewritten.
    let carried: BTreeMap<String, String> = nested
        .iter()
        .filter(|(key, _)| !KNOWN_METADATA.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let record = Record {
        kind: RecordKind::parse(&required(&nested, "type")?)?,
        status,
        metadata: carried,
        project: nested.get("project").cloned().unwrap_or_default(),
        description: required(&top, "description")?,
        links: links_in(&body),
        body: body.trim().to_owned(),
        agent: nested.get("agent").cloned().unwrap_or_default(),
        member: nested
            .get("member")
            .filter(|member| !member.is_empty())
            .cloned(),
        created_at: nested.get("created").cloned().unwrap_or_default(),
        updated_at: nested.get("updated").cloned().unwrap_or_default(),
        id,
    };
    record.validate()?;
    Ok(Document {
        record,
        version: nested.get("version").cloned(),
        merged: nested
            .get("merged")
            .map(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|version| !version.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// The records a body points at, in the order they appear. A link to a record that does not exist
/// yet is not an error — the memory format uses it to mark what is worth writing later.
fn links_in(body: &str) -> Vec<RecordId> {
    let mut found = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        let after = rest.get(start + 2..).unwrap_or_default();
        let Some(end) = after.find("]]") else {
            break;
        };
        if let Ok(id) = RecordId::parse(after.get(..end).unwrap_or_default())
            && !found.contains(&id)
        {
            found.push(id);
        }
        rest = after.get(end + 2..).unwrap_or_default();
    }
    found
}

/// What reading the directory back produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Import {
    /// Events to append to the journal, in file order.
    pub events: Vec<Event>,
    /// Documents that could not be read; their files stay untouched.
    pub rejected: Vec<(String, MemoryError)>,
    /// Files nobody touched since they were projected, which the projection no longer holds:
    /// documents of forgotten records, and rival files of versions that stopped being rivals.
    /// The caller deletes them, and no event is written.
    pub stale: Vec<String>,
}

/// Reads the edits made to the projection back into events.
///
/// A file is an edit only when it differs from the version it names: that is the version it was
/// projected from. Comparing it with the current record instead would take a projection the
/// record moved past — an agent wrote over MCP between two ticks — for somebody's edit.
///
/// A file that changed becomes a new version whose parent is the version the file was projected
/// from — so an edit made from an old projection is recognised as concurrent instead of
/// overwriting newer work. A file that disappeared is *not* a deletion: forgetting is an explicit
/// act, and a projection can go missing for a dozen innocent reasons.
///
/// The other way round, a file that is still there after its record was forgotten elsewhere is
/// *not* an edit either: the delete arrived with the journal, the file stayed behind. Only when
/// the file differs from the version it names did somebody write it, and then it is an edit
/// concurrent with the delete, which keeps the record.
///
/// A rival file is a copy of a version the journal already holds and is never read as an edit.
/// Once that version stops being a rival — a later write made it the projected one, or the record
/// was forgotten — the untouched file is stale; a rival file somebody edited stays where it is.
///
/// A rival is settled from the document of its record: `merged: <version>, <version>` in its
/// `metadata` closes those versions along with the one the file was projected from. The key is
/// never kept in the record — it lives in the event, as `merges`.
///
/// A new version is signed by whoever imports it — `agent`, and `member` when the writer is a
/// member of a team — whatever the file said: the file names the author of the version it was
/// projected from, and the edit is somebody else's work.
#[must_use]
pub fn import(
    documents: &[(String, Vec<u8>)],
    memory: &Memory,
    stamp: &str,
    agent: &str,
    member: Option<&str>,
) -> Import {
    let mut import = Import::default();
    for (path, bytes) in documents {
        if path == INDEX_FILE {
            continue; // the index is generated
        }
        let rival = path.contains(RIVAL_MARKER);
        let document = match parse_document(path, bytes) {
            Ok(document) => document,
            // A rival file is never read as an edit, so there is nothing to reject.
            Err(_) if rival => continue,
            Err(error) => {
                import.rejected.push((path.clone(), error));
                continue;
            }
        };
        let id = &document.record.id;
        let known = memory.records.get(id);
        let untouched = document
            .version
            .as_deref()
            .and_then(|version| memory.versions.get(version))
            .is_some_and(|projected| projected.id == *id && unchanged(projected, &document.record));
        if rival {
            let still_rival = document.version.as_deref().is_some_and(|version| {
                *path == rival_name(id, version)
                    && known
                        .is_some_and(|entry| entry.rivals.iter().any(|(rival, _)| rival == version))
            });
            if untouched && !still_rival {
                import.stale.push(path.clone());
            }
            continue;
        }
        if let Some(unknown) = document.merged.iter().find(|version| {
            memory
                .versions
                .get(version.as_str())
                .is_none_or(|merged| merged.id != *id)
        }) {
            import.rejected.push((
                path.clone(),
                MemoryError::UnknownMergedVersion {
                    path: path.clone(),
                    version: unknown.clone(),
                },
            ));
            continue;
        }
        // `merged` is an act of its own: keeping the projected text and dropping the rival is a
        // way to settle it too, so an unchanged text does not make the file untouched
        let settles = !document.merged.is_empty();
        if untouched && !settles {
            if known.is_none() && memory.forgotten.contains(id) {
                import.stale.push(path.clone());
            }
            continue; // the next projection rewrites it from the current version
        }
        // A file that names no version the journal holds — written by hand, or projected from a
        // version whose line has not arrived yet — can only be held against the current record.
        let named = document
            .version
            .as_deref()
            .is_some_and(|version| memory.versions.contains_key(version));
        if !settles
            && !named
            && known.is_some_and(|entry| unchanged(&entry.record, &document.record))
        {
            continue;
        }
        let mut record = document.record;
        agent.clone_into(&mut record.agent);
        record.member = member.map(str::to_owned);
        stamp.clone_into(&mut record.updated_at);
        if let Some(entry) = known {
            record.created_at.clone_from(&entry.record.created_at);
        } else if record.created_at.is_empty() {
            stamp.clone_into(&mut record.created_at);
        }
        let parent = document
            .version
            .or_else(|| known.map(|entry| entry.version.clone()));
        import.events.push(Event {
            uuid: format!("{stamp}-{}", record.id.as_str()),
            parent,
            merges: document.merged,
            action: Action::Upsert { record },
        });
    }
    import
}

/// Whether the file says the same as the record it was projected from. The fields the writer
/// cannot see in the file — who wrote it and when — are not part of the question.
fn unchanged(known: &Record, edited: &Record) -> bool {
    known.description == edited.description
        && known.body == edited.body
        && known.kind == edited.kind
        && known.links == edited.links
        // Visible in the file, so it has to be part of the question: otherwise marking a memory
        // stale by hand would be silently undone by the next projection.
        && known.status == edited.status
        && known.metadata == edited.metadata
}
