//! Whether a text holds a token, and which, case by case from
//! `fixtures/export/settingsWithToken.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::token::{holds_token, redact, token_ids};

const CASES: &str = include_str!("../../../fixtures/export/settingsWithToken.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[allow(dead_code)]
    provenance: String,
    note: String,
    text: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    holds_token: bool,
    tokens: Vec<String>,
}

#[test]
fn a_copy_holding_a_token_is_recognised_and_nothing_else_is() {
    let file: File = serde_json::from_str(CASES).expect("settingsWithToken.json");
    let mut failures = Vec::new();
    let mut ids = std::collections::BTreeSet::new();
    for case in &file.cases {
        if !ids.insert(case.id.as_str()) || case.note.trim().is_empty() {
            failures.push(format!("{}: repeated id or empty note", case.id));
        }
        if holds_token(case.text.as_bytes()) != case.expect.holds_token {
            failures.push(format!(
                "{}: expected holdsToken {}",
                case.id, case.expect.holds_token
            ));
        }
        let redacted = redact(&case.text);
        if holds_token(redacted.as_bytes()) {
            failures.push(format!("{}: a token survives redaction", case.id));
        }
        if !case.expect.holds_token && redacted != case.text {
            failures.push(format!(
                "{}: redaction changed a text without a token",
                case.id
            ));
        }
        let found: Vec<String> = token_ids(case.text.as_bytes()).into_iter().collect();
        if found != case.expect.tokens {
            failures.push(format!(
                "{}: found {found:?}, expected {:?}",
                case.id, case.expect.tokens
            ));
        }
    }
    assert!(
        ids.len() > 1 && failures.is_empty(),
        "{}",
        failures.join("\n")
    );
}
