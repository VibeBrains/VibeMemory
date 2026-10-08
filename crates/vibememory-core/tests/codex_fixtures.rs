//! Data-driven tests for reading the log of Codex: rollout files in the shape of real ones, and the session the
//! engine makes of each. Every expectation lives in `fixtures/foreign/codexLogs.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use serde_json::Value;
use vibememory_core::codex::{Problem, convert, is_log_name};
use vibememory_core::foreign::check;

const CASES: &str = include_str!("../../../fixtures/foreign/codexLogs.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CaseFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
    log_names: Vec<LogName>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[allow(dead_code)]
    note: String,
    log: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    session: Option<String>,
    cwd: Option<String>,
    lines: Option<Vec<Value>>,
    problem: Option<String>,
    line: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogName {
    name: String,
    expect: bool,
}

fn cases() -> CaseFile {
    serde_json::from_str(CASES).unwrap()
}

fn problem_of(expect: &Expect) -> Problem {
    match expect.problem.as_deref() {
        Some("empty") => Problem::Empty,
        Some("noHeader") => Problem::NoHeader,
        Some("record") => Problem::Record {
            line: expect.line.unwrap(),
        },
        other => panic!("unknown problem {other:?}"),
    }
}

#[test]
fn a_log_becomes_the_conversation_once() {
    for case in cases().cases {
        let result = convert(case.log.as_bytes());
        let Some(lines) = &case.expect.lines else {
            assert_eq!(result, Err(problem_of(&case.expect)), "{}", case.id);
            continue;
        };
        let session = result.unwrap_or_else(|problem| panic!("{}: {problem:?}", case.id));
        assert_eq!(
            Some(&session.id),
            case.expect.session.as_ref(),
            "{}",
            case.id
        );
        assert_eq!(Some(&session.cwd), case.expect.cwd.as_ref(), "{}", case.id);
        let got: Vec<Value> = session
            .file
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(&got, lines, "{}", case.id);
        assert_eq!(session.records, lines.len(), "{}", case.id);
        // what the engine reads it makes in the format it accepts from every other agent
        let checked = check(session.file.as_bytes())
            .unwrap_or_else(|refusal| panic!("{}: {refusal:?}", case.id));
        assert_eq!(checked.lines, lines.len(), "{}", case.id);
    }
}

#[test]
fn a_log_is_known_by_its_name() {
    for case in cases().log_names {
        assert_eq!(is_log_name(&case.name), case.expect, "{}", case.name);
    }
}
