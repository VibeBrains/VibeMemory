//! Data-driven tests for the survey of a transcript: what the tick writes into `tails.json` and
//! what the freshness gate compares against. Expectations live in
//! `fixtures/merge/surveyScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::merge::jsonl::survey;

const SCENARIOS: &str = include_str!("../../../fixtures/merge/surveyScenarios.json");

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
    /// A transcript fixture next to this file; exactly one of `file` and `text` is given.
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    text: Option<String>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    lines: usize,
    truncated: usize,
    last_uuid: Option<String>,
    resume_leaf: Option<String>,
    opaque: usize,
}

/// Transcript fixtures are compiled in, not read: the purity gate of the crate forbids I/O even
/// in tests, and a name that is not in this list is a fixture nobody added on purpose.
fn transcript(name: &str) -> &'static [u8] {
    match name {
        "cli255Session.jsonl" => include_bytes!("../../../fixtures/merge/cli255Session.jsonl"),
        "cli255Isolated.jsonl" => include_bytes!("../../../fixtures/merge/cli255Isolated.jsonl"),
        "cli232Isolated.jsonl" => include_bytes!("../../../fixtures/merge/cli232Isolated.jsonl"),
        "cli255Subagent.jsonl" => include_bytes!("../../../fixtures/merge/cli255Subagent.jsonl"),
        "cli255Journal.jsonl" => include_bytes!("../../../fixtures/merge/cli255Journal.jsonl"),
        other => {
            panic!("unknown transcript {other:?}: add it to fixtures/merge/ and to this match")
        }
    }
}

fn bytes_of(case: &Case) -> Vec<u8> {
    match (&case.file, &case.text) {
        (Some(name), None) => transcript(name).to_vec(),
        (None, Some(text)) => text.clone().into_bytes(),
        _ => panic!("{}: give exactly one of file and text", case.id),
    }
}

#[test]
fn survey_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("surveyScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();

    for case in &file.cases {
        let label = format!("surveyScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }

        let bytes = bytes_of(case);
        let got = survey(&bytes);
        let want = &case.expect;
        if got.lines != want.lines {
            failures.push(format!(
                "{label}: {} lines, expected {}",
                got.lines, want.lines
            ));
        }
        if got.truncated != want.truncated {
            failures.push(format!(
                "{label}: {} truncated bytes, expected {}",
                got.truncated, want.truncated
            ));
        }
        if got.last_uuid != want.last_uuid {
            failures.push(format!(
                "{label}: last uuid {:?}, expected {:?}",
                got.last_uuid, want.last_uuid
            ));
        }
        if got.resume_leaf != want.resume_leaf {
            failures.push(format!(
                "{label}: resume leaf {:?}, expected {:?}",
                got.resume_leaf, want.resume_leaf
            ));
        }
        if got.opaque != want.opaque {
            failures.push(format!(
                "{label}: {} opaque lines, expected {}",
                got.opaque, want.opaque
            ));
        }

        // The survey may not depend on how much of the file has been written: appending a
        // half-written record must change only the truncated count.
        let mut half = bytes.clone();
        half.extend_from_slice(b"{\"type\":\"user\"");
        let after = survey(&half);
        if after.lines != got.lines || after.last_uuid != got.last_uuid {
            failures.push(format!(
                "{label}: a half-written record changed the survey of the complete lines"
            ));
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
