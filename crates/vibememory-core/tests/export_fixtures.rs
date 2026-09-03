//! Data-driven tests for the export gate: what leaves the machine and what stays.
//! Every expectation lives in `fixtures/export/exportScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::export::{self, Decision, Refusal};

const SCENARIOS: &str = include_str!("../../../fixtures/export/exportScenarios.json");

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
    /// Path relative to the config directory, `/`-separated.
    path: String,
    /// The file's size; a directory and a path whose size does not matter have none.
    #[serde(default)]
    size: Option<u64>,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    decision: ExpectedDecision,
    #[serde(default)]
    refusal: Option<ExpectedRefusal>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedDecision {
    Export,
    Refuse,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedRefusal {
    Secret,
    MachineLocal,
    NotListed,
    TooLarge,
}

impl ExpectedRefusal {
    fn matches(&self, refusal: Refusal) -> bool {
        matches!(
            (self, refusal),
            (Self::Secret, Refusal::Secret)
                | (Self::MachineLocal, Refusal::MachineLocal)
                | (Self::NotListed, Refusal::NotListed)
                | (Self::TooLarge, Refusal::TooLarge)
        )
    }
}

#[test]
fn export_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("exportScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();

    for case in &file.cases {
        let label = format!("exportScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }

        match (export::decide(&case.path, case.size), &case.expect.decision) {
            (Decision::Export, ExpectedDecision::Export) => {
                if case.expect.refusal.is_some() {
                    failures.push(format!("{label}: an exported path cannot name a refusal"));
                }
            }
            (Decision::Refuse { refusal, rule }, ExpectedDecision::Refuse) => {
                let Some(expected) = &case.expect.refusal else {
                    failures.push(format!("{label}: a refused path must name its refusal"));
                    continue;
                };
                if !expected.matches(refusal) {
                    failures.push(format!(
                        "{label}: refused as {refusal:?}, expected {expected:?}"
                    ));
                }
                if rule.trim().is_empty() {
                    failures.push(format!("{label}: the refusal carries no reason to show"));
                }
            }
            (got, expected) => {
                failures.push(format!("{label}: {got:?}, expected {expected:?}"));
            }
        }
    }

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

/// The rule the whole gate rests on: nothing is exported because it looks harmless. A path is
/// carried out only if the allow list names it, so an empty path and a stray one both stay.
#[test]
fn nothing_leaves_without_a_rule() {
    for path in ["", "/", "unknown", "a/b/c", ".hidden"] {
        assert!(
            !export::decide(path, None).is_export(),
            "{path:?} left the machine without a rule"
        );
    }
}
