//! Data-driven tests for the files other agents hand the engine. Every expectation lives in
//! `fixtures/foreign/foreignSessions.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::foreign::{AGENTS_DIR, Checked, Problem, check, session_path};

const SCENARIOS: &str = include_str!("../../../fixtures/foreign/foreignSessions.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Computed,
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
    #[allow(dead_code)]
    provenance: Provenance,
    #[allow(dead_code)]
    note: String,
    /// The file exactly as the agent writes it.
    text: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    lines: Option<usize>,
    spoken: Option<usize>,
    refused: Option<Refused>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Refused {
    line: usize,
    problem: String,
}

/// The name a fixture gives a problem.
fn named(problem: &Problem) -> String {
    match problem {
        Problem::Empty => "empty".to_owned(),
        Problem::NoFinalNewline => "noFinalNewline".to_owned(),
        Problem::NotText => "notText".to_owned(),
        Problem::BlankLine => "blankLine".to_owned(),
        Problem::NotAnObject => "notAnObject".to_owned(),
        Problem::Missing(field) => format!("missing:{field}"),
        Problem::Timestamp => "timestamp".to_owned(),
        Problem::DuplicateUuid => "duplicateUuid".to_owned(),
        Problem::Content => "content".to_owned(),
    }
}

#[test]
fn every_foreign_session_case_gets_its_verdict() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("scenarios");
    assert!(!file.cases.is_empty());
    for case in file.cases {
        let verdict = check(case.text.as_bytes());
        match (&case.expect.refused, verdict) {
            (None, Ok(checked)) => assert_eq!(
                checked,
                Checked {
                    lines: case.expect.lines.expect("lines"),
                    spoken: case.expect.spoken.expect("spoken"),
                },
                "{}",
                case.id
            ),
            (Some(refused), Err(got)) => assert_eq!(
                (got.line, named(&got.problem)),
                (refused.line, refused.problem.clone()),
                "{}",
                case.id
            ),
            (expected, got) => panic!(
                "{}: expected refusal {:?}, got {got:?}",
                case.id,
                expected.as_ref().map(|refused| &refused.problem)
            ),
        }
    }
}

#[test]
fn bytes_that_are_not_text_are_refused_as_a_whole() {
    let refused = check(&[0xFF, 0xFE, b'\n']).expect_err("not text");
    assert_eq!((refused.line, refused.problem), (0, Problem::NotText));
}

#[test]
fn a_foreign_session_never_sits_among_claude_codes() {
    assert_eq!(
        session_path("Promed", None, "s1"),
        "projects/Promed/s1.jsonl"
    );
    let foreign = session_path("Promed", Some("dsh-desktop"), "s1");
    assert_eq!(
        foreign,
        format!("projects/Promed/{AGENTS_DIR}/dsh-desktop/s1.jsonl")
    );
}
