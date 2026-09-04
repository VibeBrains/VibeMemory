//! Data-driven tests for `machines/<id>/links.json`: what the file accepts, what it refuses and
//! what survives being written back. Expectations live in `fixtures/links/linksScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::links::{LinkRecord, LinksFile};

const SCENARIOS: &str = include_str!("../../../fixtures/links/linksScenarios.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScenarioFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
    same_name: Vec<SameNameCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    provenance: Provenance,
    note: String,
    /// The file as it sits on disk.
    text: String,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    parses: bool,
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    records: Option<Vec<ExpectedRecord>>,
    /// The text the file must produce when written back, when it is pinned.
    #[serde(default)]
    round_trip: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExpectedRecord {
    enc: String,
    name: String,
    predicted: bool,
    confirmed: bool,
    /// Whether the name is a legal store name; omitted means it is.
    #[serde(default)]
    name_parses: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SameNameCase {
    id: String,
    provenance: Provenance,
    note: String,
    left: String,
    right: String,
    same: bool,
}

fn record_with_name(name: &str) -> LinkRecord {
    let text = format!(
        "{{\"enc\":\"-x\",\"name\":\"{name}\",\"cwd\":\"{{P}}/x\",\
         \"syntax\":\"posix\",\"source\":\"observed\"}}"
    );
    serde_json::from_str(&text).expect("record")
}

fn check_case(case: &Case, failures: &mut Vec<String>) {
    let label = format!("linksScenarios.json#{} [{:?}]", case.id, case.provenance);
    if case.note.trim().is_empty() {
        failures.push(format!("{label}: empty note"));
    }

    let parsed: Result<LinksFile, _> = serde_json::from_str(&case.text);
    let file = match (parsed, case.expect.parses) {
        (Ok(_), false) => {
            failures.push(format!("{label}: the file was accepted and must not be"));
            return;
        }
        (Err(error), true) => {
            failures.push(format!("{label}: rejected — {error}"));
            return;
        }
        (Err(_), false) => return,
        (Ok(file), true) => file,
    };

    if let Some(version) = case.expect.version
        && file.version != version
    {
        failures.push(format!(
            "{label}: version {}, expected {version}",
            file.version
        ));
    }

    if let Some(expected) = &case.expect.records {
        if file.links.len() != expected.len() {
            failures.push(format!(
                "{label}: {} records, expected {}",
                file.links.len(),
                expected.len()
            ));
        }
        for (got, want) in file.links.iter().zip(expected) {
            if got.enc != want.enc || got.name != want.name {
                failures.push(format!(
                    "{label}: record {}→{}, expected {}→{}",
                    got.enc, got.name, want.enc, want.name
                ));
            }
            if got.predicted != want.predicted {
                failures.push(format!(
                    "{label}: {} predicted {}, expected {}",
                    got.enc, got.predicted, want.predicted
                ));
            }
            if got.is_confirmed() != want.confirmed {
                failures.push(format!(
                    "{label}: {} confirmed {}, expected {}",
                    got.enc,
                    got.is_confirmed(),
                    want.confirmed
                ));
            }
            let name_is_legal = got.name().is_ok();
            if name_is_legal != want.name_parses.unwrap_or(true) {
                failures.push(format!(
                    "{label}: {} name parses {name_is_legal}, expected otherwise",
                    got.enc
                ));
            }
        }
    }

    // Whatever the case pins, the file must survive being read and written: the reconciler
    // rewrites it on every tick, and a field lost in that circle is a link that quietly changes
    // meaning.
    let written = serde_json::to_string(&file).expect("write");
    let again: LinksFile = serde_json::from_str(&written).expect("read back");
    if again != file {
        failures.push(format!("{label}: the file does not survive a round trip"));
    }
    if let Some(expected) = &case.expect.round_trip
        && written != *expected
    {
        failures.push(format!(
            "{label}: written as\n{written}\nexpected\n{expected}"
        ));
    }
}

#[test]
fn links_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("linksScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();

    for case in &file.cases {
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("duplicate id {}", case.id));
        }
        ids.push(&case.id);
        check_case(case, &mut failures);
    }

    for case in &file.same_name {
        let label = format!("linksScenarios.json#{} [{:?}]", case.id, case.provenance);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }
        let left = record_with_name(&case.left);
        let right = record_with_name(&case.right);
        if left.same_name_as(&right) != case.same {
            failures.push(format!(
                "{label}: {} vs {} — same {}, expected {}",
                case.left,
                case.right,
                left.same_name_as(&right),
                case.same
            ));
        }
        // The relation may not depend on the order it is asked in.
        if left.same_name_as(&right) != right.same_name_as(&left) {
            failures.push(format!("{label}: the comparison is not symmetric"));
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
