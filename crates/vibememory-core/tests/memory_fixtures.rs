//! Data-driven tests for memory as records: folding the journal, projecting it into markdown and
//! reading edits back. Every expectation lives in `fixtures/memory/memoryScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::memory::markdown::{self, Import};
use vibememory_core::memory::{Event, Memory, RecordId, fold, journal};

const SCENARIOS: &str = include_str!("../../../fixtures/memory/memoryScenarios.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenarioFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    provenance: Provenance,
    note: String,
    /// The journal as raw lines, exactly as they would sit in the file.
    journal: Vec<String>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    records: Vec<ExpectedRecord>,
    forgotten: Vec<String>,
    unreadable: usize,
    /// Files of the projection whose text is pinned here.
    #[serde(default)]
    files: Vec<ExpectedFile>,
    /// The names the projection should hold, when only the set matters.
    #[serde(default)]
    file_names: Option<Vec<String>>,
    /// Links a record's body points at.
    #[serde(default)]
    links: Option<std::collections::BTreeMap<String, Vec<String>>>,
    #[serde(default)]
    import: Option<ExpectedImport>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedRecord {
    id: String,
    version: String,
    rivals: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedFile {
    name: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExpectedImport {
    stamp: String,
    agent: String,
    /// The member the import is made by; absent for the store's owner.
    #[serde(default)]
    member: Option<String>,
    /// Read the projection back untouched instead of listing documents.
    #[serde(default)]
    use_projection: bool,
    #[serde(default)]
    documents: Vec<ExpectedFile>,
    events: Vec<ExpectedEvent>,
    rejected: Vec<ExpectedRejection>,
    /// Projections the caller has to remove, because their record was forgotten.
    #[serde(default)]
    stale: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExpectedEvent {
    uuid: String,
    parent: Option<String>,
    id: String,
    body_contains: String,
    created_at: String,
    /// What the event's record must say about the fact still holding. Absent means `active`,
    /// which is what every case written before the status existed expects.
    #[serde(default = "active_status")]
    status: String,
    /// The member the event's record names. Absent means none.
    #[serde(default)]
    member: Option<String>,
    /// Rival versions the event settles besides its parent. Absent means none.
    #[serde(default)]
    merges: Vec<String>,
    /// The metadata the event's record carries, when the case pins it.
    #[serde(default)]
    metadata: Option<std::collections::BTreeMap<String, String>>,
}

fn active_status() -> String {
    "active".to_owned()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedRejection {
    name: String,
    code: String,
}

fn read_journal(lines: &[String]) -> Memory {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    let (events, unreadable) = journal::parse(&bytes);
    fold(&events, unreadable)
}

/// One event an import wrote, against the one the case expects.
fn check_event(
    label: &str,
    got: &Event,
    want: &ExpectedEvent,
    expected: &ExpectedImport,
    failures: &mut Vec<String>,
) {
    if got.uuid != want.uuid {
        failures.push(format!(
            "{label}: event uuid {}, expected {}",
            got.uuid, want.uuid
        ));
    }
    if got.parent != want.parent {
        failures.push(format!(
            "{label}: parent {:?}, expected {:?}",
            got.parent, want.parent
        ));
    }
    if got.merges != want.merges {
        failures.push(format!(
            "{label}: merges {:?}, expected {:?}",
            got.merges, want.merges
        ));
    }
    if got.id().as_str() != want.id {
        failures.push(format!(
            "{label}: event about {}, expected {}",
            got.id().as_str(),
            want.id
        ));
    }
    let vibememory_core::memory::Action::Upsert { record } = &got.action else {
        failures.push(format!("{label}: expected an upsert"));
        return;
    };
    if !record.body.contains(&want.body_contains) {
        failures.push(format!(
            "{label}: body does not contain {:?}",
            want.body_contains
        ));
    }
    if record.created_at != want.created_at {
        failures.push(format!(
            "{label}: createdAt {}, expected {}",
            record.created_at, want.created_at
        ));
    }
    if record.agent != expected.agent || record.updated_at != expected.stamp {
        failures.push(format!(
            "{label}: the event does not carry the caller's agent and stamp"
        ));
    }
    if record.status.as_str() != want.status {
        failures.push(format!(
            "{label}: status {}, expected {}",
            record.status.as_str(),
            want.status
        ));
    }
    if want
        .metadata
        .as_ref()
        .is_some_and(|metadata| *metadata != record.metadata)
    {
        failures.push(format!(
            "{label}: metadata {:?}, expected {:?}",
            record.metadata, want.metadata
        ));
    }
    if record.member != want.member {
        failures.push(format!(
            "{label}: member {:?}, expected {:?}",
            record.member, want.member
        ));
    }
    // Every event must survive the journal round trip, or it cannot be written at all.
    let line = journal::encode(got).expect("event encodes");
    let (parsed, unreadable) = journal::parse(&line);
    if parsed.as_slice() != std::slice::from_ref(got) || !unreadable.is_empty() {
        failures.push(format!(
            "{label}: the event does not survive being written and read back"
        ));
    }
}

fn check_import(
    label: &str,
    memory: &Memory,
    expected: &ExpectedImport,
    failures: &mut Vec<String>,
) {
    let documents: Vec<(String, Vec<u8>)> = if expected.use_projection {
        markdown::project(memory).files
    } else {
        expected
            .documents
            .iter()
            .map(|file| (file.name.clone(), file.text.clone().into_bytes()))
            .collect()
    };
    let Import {
        events,
        rejected,
        stale,
    } = markdown::import(
        &documents,
        memory,
        &expected.stamp,
        &expected.agent,
        expected.member.as_deref(),
    );

    if events.len() != expected.events.len() {
        failures.push(format!(
            "{label}: {} events, expected {}",
            events.len(),
            expected.events.len()
        ));
    }
    for (got, want) in events.iter().zip(&expected.events) {
        check_event(label, got, want, expected, failures);
    }
    let codes: Vec<(String, String)> = rejected
        .iter()
        .map(|(name, error)| (name.clone(), error.code().to_owned()))
        .collect();
    let wanted: Vec<(String, String)> = expected
        .rejected
        .iter()
        .map(|rejection| (rejection.name.clone(), rejection.code.clone()))
        .collect();
    if codes != wanted {
        failures.push(format!("{label}: rejected {codes:?}, expected {wanted:?}"));
    }
    if stale != expected.stale {
        failures.push(format!(
            "{label}: stale {stale:?}, expected {:?}",
            expected.stale
        ));
    }
}

/// What the journal folded into, compared with what the case says it should be.
fn check_state(label: &str, memory: &Memory, expect: &Expect, failures: &mut Vec<String>) {
    let got: Vec<(String, String, Vec<String>)> = memory
        .records
        .values()
        .map(|entry| {
            (
                entry.record.id.as_str().to_owned(),
                entry.version.clone(),
                entry
                    .rivals
                    .iter()
                    .map(|(version, _)| version.clone())
                    .collect(),
            )
        })
        .collect();
    let want: Vec<(String, String, Vec<String>)> = expect
        .records
        .iter()
        .map(|record| {
            (
                record.id.clone(),
                record.version.clone(),
                record.rivals.clone(),
            )
        })
        .collect();
    if got != want {
        failures.push(format!("{label}: records {got:?}, expected {want:?}"));
    }

    let forgotten: Vec<String> = memory
        .forgotten
        .iter()
        .map(|id| id.as_str().to_owned())
        .collect();
    if forgotten != expect.forgotten {
        failures.push(format!(
            "{label}: forgotten {forgotten:?}, expected {:?}",
            expect.forgotten
        ));
    }
    if memory.unreadable.len() != expect.unreadable {
        failures.push(format!(
            "{label}: {} unreadable lines, expected {}",
            memory.unreadable.len(),
            expect.unreadable
        ));
    }
    if let Some(links) = &expect.links {
        for (id, expected) in links {
            let id = RecordId::parse(id).expect("fixture id");
            let got: Vec<String> = memory
                .records
                .get(&id)
                .map(|entry| {
                    entry
                        .record
                        .links
                        .iter()
                        .map(|link| link.as_str().to_owned())
                        .collect()
                })
                .unwrap_or_default();
            if got != *expected {
                failures.push(format!(
                    "{label}: links of {} are {got:?}, expected {expected:?}",
                    id.as_str()
                ));
            }
        }
    }
}

/// The files the memory projects into, compared with the ones the case pins.
fn check_projection(label: &str, memory: &Memory, expect: &Expect, failures: &mut Vec<String>) {
    let projection = markdown::project(memory);
    for expected in &expect.files {
        match projection
            .files
            .iter()
            .find(|(name, _)| *name == expected.name)
        {
            None => failures.push(format!("{label}: the projection has no {}", expected.name)),
            Some((_, bytes)) => {
                let text = String::from_utf8_lossy(bytes);
                if text != expected.text {
                    failures.push(format!(
                        "{label}: {} differs\n--- got ---\n{text}--- expected ---\n{}",
                        expected.name, expected.text
                    ));
                }
            }
        }
    }
    if let Some(names) = &expect.file_names {
        let got: Vec<String> = projection
            .file_names()
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        if got != *names {
            failures.push(format!("{label}: files {got:?}, expected {names:?}"));
        }
    }
    // Whatever the case says, a projection read straight back must produce nothing: the tick runs
    // every two minutes and may not write a version each time.
    let untouched = markdown::import(
        &projection.files,
        memory,
        "2026-01-01T00:00:00Z",
        "gate",
        None,
    );
    if !untouched.events.is_empty() {
        failures.push(format!(
            "{label}: reading back an untouched projection wrote {} events",
            untouched.events.len()
        ));
    }
}

#[test]
fn memory_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("memoryScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();

    for case in &file.cases {
        let label = format!("memoryScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }

        let memory = read_journal(&case.journal);
        check_state(&label, &memory, &case.expect, &mut failures);
        check_projection(&label, &memory, &case.expect, &mut failures);
        if let Some(expected) = &case.expect.import {
            check_import(&label, &memory, expected, &mut failures);
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
