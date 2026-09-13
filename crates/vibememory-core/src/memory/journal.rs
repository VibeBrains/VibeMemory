//! The journal: every version of every memory, in the order the machines wrote them.
//!
//! Each line is one event with its own `uuid`, which is not decoration — it is what lets the
//! transcript merge driver merge journals between machines with no rules of its own: union by
//! `uuid`, nothing lost, nothing invented.
//!
//! An event also names the version its writer had seen (`parent`). That single field is the
//! difference between a memory store and a race: when two machines write from the same version,
//! the fold sees two leaves instead of one and keeps both, instead of letting the later clock
//! erase the earlier work.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::error::MemoryError;
use super::record::{Record, RecordId};

/// One entry of the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    /// Identity of this version. The merge driver keys lines by it.
    pub uuid: String,
    /// The version its writer had seen, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// What the writer did.
    #[serde(flatten)]
    pub action: Action,
}

/// Writing a memory, or asking for it to be forgotten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    /// A version of a record.
    Upsert {
        /// The record as written.
        record: Record,
    },
    /// A request to forget it. Nothing is erased from the journal: the record stops being
    /// projected, and the versions stay where they are.
    Delete {
        /// Which record.
        id: RecordId,
        /// Which agent asked.
        agent: String,
        /// When, in the writer's clock.
        updated_at: String,
    },
}

impl Event {
    /// Which record the event is about.
    #[must_use]
    pub fn id(&self) -> &RecordId {
        match &self.action {
            Action::Upsert { record } => &record.id,
            Action::Delete { id, .. } => id,
        }
    }

    /// When it was written.
    #[must_use]
    pub fn updated_at(&self) -> &str {
        match &self.action {
            Action::Upsert { record } => &record.updated_at,
            Action::Delete { updated_at, .. } => updated_at,
        }
    }
}

/// A line of the journal that could not be read. It is reported rather than dropped: a line the
/// engine does not understand may be one a newer version wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableLine {
    /// Position in the file, counting from one.
    pub line: usize,
    /// What serde said.
    pub reason: String,
}

/// What the journal says right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    /// The live records, by identity.
    pub records: BTreeMap<RecordId, Entry>,
    /// Records whose last word was "forget it", with the version the delete saw.
    pub forgotten: BTreeMap<RecordId, Forgotten>,
    /// Lines that could not be read.
    pub unreadable: Vec<UnreadableLine>,
}

/// What a delete asked to forget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// The version the delete named as its parent, when the journal still holds it. A projection
    /// written from exactly this version is what the delete meant to remove, not a new edit.
    pub seen: Option<(String, Record)>,
}

/// A record as it stands, and the versions that disagree with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The version to show and to project.
    pub record: Record,
    /// The `uuid` of that version: what the next writer names as its parent.
    pub version: String,
    /// Versions written from the same parent by someone else. They are kept — memory is the one
    /// thing both machines edit every session, and "the newer one wins" loses half of it.
    pub rivals: Vec<(String, Record)>,
}

impl Entry {
    /// Whether two machines wrote this record without seeing each other.
    #[must_use]
    pub fn is_divergent(&self) -> bool {
        !self.rivals.is_empty()
    }
}

/// Reads a journal file. Unreadable lines are collected, never silently skipped.
#[must_use]
pub fn parse(bytes: &[u8]) -> (Vec<Event>, Vec<UnreadableLine>) {
    let mut events = Vec::new();
    let mut unreadable = Vec::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Event>(line) {
            Ok(event) => events.push(event),
            Err(error) => unreadable.push(UnreadableLine {
                line: index + 1,
                reason: error.to_string(),
            }),
        }
    }
    (events, unreadable)
}

/// Writes one event as a journal line, terminator included.
pub fn encode(event: &Event) -> Result<Vec<u8>, MemoryError> {
    let mut line = serde_json::to_vec(event).map_err(|error| MemoryError::Unserializable {
        id: event.id().as_str().to_owned(),
        reason: error.to_string(),
    })?;
    line.push(b'\n');
    Ok(line)
}

/// Folds the journal into the memory it describes.
///
/// A version is superseded when another event names it as parent; what is left are the leaves.
/// One leaf is the record. Several leaves mean two machines wrote from the same version: the
/// first by `(updatedAt, uuid)` is projected and the rest are kept beside it as rivals. A delete
/// wins only when nothing else was written after the version it saw — if someone edited the
/// record meanwhile, they wanted it, and it stays.
#[must_use]
pub fn fold(events: &[Event], unreadable: Vec<UnreadableLine>) -> Memory {
    let mut by_id: BTreeMap<&RecordId, Vec<&Event>> = BTreeMap::new();
    for event in events {
        by_id.entry(event.id()).or_default().push(event);
    }

    let mut memory = Memory {
        unreadable,
        ..Memory::default()
    };
    for (id, mut group) in by_id {
        // Deterministic order first, so equal clocks never depend on the order of the file.
        group.sort_by(|left, right| {
            left.updated_at()
                .cmp(right.updated_at())
                .then_with(|| left.uuid.cmp(&right.uuid))
        });
        let superseded: BTreeSet<&str> = group
            .iter()
            .filter_map(|event| event.parent.as_deref())
            .collect();
        let leaves: Vec<&&Event> = group
            .iter()
            .filter(|event| !superseded.contains(event.uuid.as_str()))
            .collect();

        let mut versions = Vec::new();
        let mut deleted: Option<&Event> = None;
        for leaf in leaves {
            match &leaf.action {
                Action::Upsert { record } => versions.push((leaf.uuid.clone(), record.clone())),
                Action::Delete { .. } => deleted = Some(leaf),
            }
        }
        match versions.split_first() {
            // Someone asked to forget it and nobody wrote it afterwards.
            None => {
                if let Some(delete) = deleted {
                    let seen = delete.parent.as_deref().and_then(|parent| {
                        group.iter().find_map(|event| match &event.action {
                            Action::Upsert { record } if event.uuid == parent => {
                                Some((event.uuid.clone(), record.clone()))
                            }
                            _ => None,
                        })
                    });
                    memory.forgotten.insert(id.clone(), Forgotten { seen });
                }
            }
            Some(((version, record), rivals)) => {
                memory.records.insert(
                    id.clone(),
                    Entry {
                        record: record.clone(),
                        version: version.clone(),
                        rivals: rivals.to_vec(),
                    },
                );
            }
        }
    }
    memory
}
