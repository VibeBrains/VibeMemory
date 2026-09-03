//! Data-driven tests for choosing whole files in the store: every expectation lives in
//! `fixtures/merge/keepBothScenarios.json`. Besides the recorded expectation, each case is
//! checked against the invariants the driver must hold whatever it is given.

// Integration-test helpers are test code even though clippy's `allow-*-in-tests` only sees
// `#[test]` functions: a broken fixture or a violated invariant must stop the run.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use serde_json::Value;
use vibememory_core::merge::keep_both::{KeepBothInput, KeepBothOutcome, Resolution, keep_both};

const SCENARIOS: &str = include_str!("../../../fixtures/merge/keepBothScenarios.json");
/// Longest file name accepted by APFS and NTFS, in bytes of UTF-8.
const MAX_NAME_BYTES: usize = 255;

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
    input: Input,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    stamp: String,
    base: Blob,
    ours: Blob,
    theirs: Blob,
}

/// One version of a file: text when it is text, hex when no JSON string could carry the bytes.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    #[serde(default)]
    empty: bool,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    hex: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    /// Which input the merged bytes must equal.
    result: Source,
    quarantine: Option<QuarantineExpectation>,
    report: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarantineExpectation {
    name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Source {
    Base,
    Ours,
    Theirs,
}

impl Blob {
    fn bytes(&self) -> Vec<u8> {
        match (self.empty, &self.text, &self.hex) {
            (true, None, None) => Vec::new(),
            (false, Some(text), None) => text.as_bytes().to_vec(),
            (false, None, Some(hex)) => from_hex(hex),
            _ => panic!("a blob takes exactly one of `empty` / `text` / `hex`"),
        }
    }
}

fn from_hex(hex: &str) -> Vec<u8> {
    assert!(
        hex.len().is_multiple_of(2),
        "hex must have an even number of digits"
    );
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digits"))
        .collect()
}

/// One case with its versions turned into bytes.
struct Materialized<'a> {
    path: &'a str,
    stamp: &'a str,
    base: Vec<u8>,
    ours: Vec<u8>,
    theirs: Vec<u8>,
}

impl Materialized<'_> {
    fn merge(&self, ours: &[u8], theirs: &[u8], base: &[u8]) -> KeepBothOutcome {
        keep_both(&KeepBothInput {
            path: self.path,
            base,
            ours,
            theirs,
            stamp: self.stamp,
        })
    }
}

/// Invariants that hold for every input, whatever the recorded expectation says.
fn check_invariants(
    label: &str,
    case: &Materialized<'_>,
    outcome: &KeepBothOutcome,
    failures: &mut Vec<String>,
) {
    let (ours, theirs) = (&case.ours, &case.theirs);
    let mut check = |what: &str, ok: bool| {
        if !ok {
            failures.push(format!("{label}: {what}"));
        }
    };
    check(
        "the result is not one of the inputs",
        outcome.bytes == *ours || outcome.bytes == *theirs,
    );
    check(
        "a version was set aside without a conflict, or a conflict lost a version",
        outcome.quarantined.is_some() == (outcome.report.resolution == Resolution::KeptBoth),
    );
    if let Some(quarantined) = &outcome.quarantined {
        check(
            "the quarantined bytes are not the incoming version",
            quarantined.bytes == *theirs,
        );
        check(
            "the quarantined name is not one path component",
            !quarantined.name.contains('/') && !quarantined.name.contains('\\'),
        );
        check(
            "the quarantined name is longer than a file name may be",
            quarantined.name.len() <= MAX_NAME_BYTES,
        );
        check(
            "the quarantined name is empty",
            !quarantined.name.is_empty(),
        );
    }
    // Merging the same versions twice must not change anything, and the machine that receives
    // this result must settle on it instead of quarantining it again.
    let again = case.merge(&outcome.bytes, theirs, &case.base);
    check(
        "merging the result again changes it",
        again.bytes == outcome.bytes,
    );
    let other_machine = case.merge(theirs, &outcome.bytes, theirs);
    check(
        "the other machine does not converge on the result",
        other_machine.bytes == outcome.bytes,
    );
}

#[test]
fn keep_both_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("keepBothScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for case in &file.cases {
        let label = format!("keepBothScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }
        let materialized = Materialized {
            path: &case.input.path,
            stamp: &case.input.stamp,
            base: case.input.base.bytes(),
            ours: case.input.ours.bytes(),
            theirs: case.input.theirs.bytes(),
        };
        let outcome =
            materialized.merge(&materialized.ours, &materialized.theirs, &materialized.base);

        let expected_bytes = match case.expect.result {
            Source::Base => &materialized.base,
            Source::Ours => &materialized.ours,
            Source::Theirs => &materialized.theirs,
        };
        if outcome.bytes != *expected_bytes {
            failures.push(format!(
                "{label}: result differs — got {} bytes, expected the {:?} version ({} bytes)",
                outcome.bytes.len(),
                case.expect.result,
                expected_bytes.len()
            ));
        }
        match (&outcome.quarantined, &case.expect.quarantine) {
            (Some(got), Some(want)) if got.name == want.name => {}
            (Some(got), Some(want)) => failures.push(format!(
                "{label}: quarantined name differs\n    got      {}\n    expected {}",
                got.name, want.name
            )),
            (None, None) => {}
            (Some(got), None) => {
                failures.push(format!("{label}: quarantined {} unexpectedly", got.name));
            }
            (None, Some(want)) => {
                failures.push(format!(
                    "{label}: expected {} in quarantine, got nothing",
                    want.name
                ));
            }
        }
        let report = serde_json::to_value(outcome.report).expect("report serializes");
        if report != case.expect.report {
            failures.push(format!(
                "{label}: report differs\n    got      {report}\n    expected {}",
                case.expect.report
            ));
        }
        check_invariants(&label, &materialized, &outcome, &mut failures);
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
