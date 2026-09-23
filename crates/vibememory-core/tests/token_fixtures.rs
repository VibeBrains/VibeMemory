//! Whether a managed copy holds a token, case by case from `fixtures/export/settingsWithToken.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::token::holds_token;

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
    }
    assert!(
        ids.len() > 1 && failures.is_empty(),
        "{}",
        failures.join("\n")
    );
}
