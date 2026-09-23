//! The host's report against its fixtures: `host.json` from `fixtures/host/status.json`, and the
//! answer a machine key gets from `fixtures/shell/statusAnswer.json`. A report written by the host
//! reads back to itself, and a field the reader does not know is refused.

// The gate reads the repository on purpose.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use serde_json::Value;
use vibememory_mcp::status::{self, Applied, Facts, HostReport};

#[test]
fn host_json_is_built_as_the_fixture_says() {
    let file = support::fixture("fixtures/host/status.json");
    let snapshot = support::snapshot(&[&file["snapshotPatch"]]);
    let mut failures = Vec::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let usable = case["snapshotUsable"].as_bool().unwrap_or(true);
        let applied: Option<Applied> =
            serde_json::from_value(case["applied"].clone()).expect("applied reads");
        let facts: Facts = serde_json::from_value(case["facts"].clone()).expect("facts read");
        let report = status::host_report(usable.then_some(&snapshot), applied, facts);
        let written = serde_json::to_value(&report).expect("the report encodes");
        if written != case["expect"] {
            failures.push(format!(
                "{id}: built {written}, expected {}",
                case["expect"]
            ));
        }
        let read: HostReport = serde_json::from_value(written).expect("the report reads back");
        if read != report {
            failures.push(format!("{id}: the report does not read back to itself"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_report_with_an_unknown_field_is_refused() {
    let file = support::fixture("fixtures/host/status.json");
    let mut report = file["cases"][0]["expect"].clone();
    report["cabinetHint"] = Value::Bool(true);
    assert!(serde_json::from_value::<HostReport>(report).is_err());
}

#[test]
fn a_key_sees_only_its_own_teams() {
    let file = support::fixture("fixtures/shell/statusAnswer.json");
    let snapshot = support::snapshot(&[&file["snapshotPatch"]]);
    let report: HostReport = serde_json::from_value(file["host"].clone()).expect("host reads");
    let mut failures = Vec::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let answer = status::answer(&snapshot, &report, case["key"].as_str().expect("key"));
        let answered = serde_json::to_value(&answer).expect("the answer encodes");
        if answered != case["expect"] {
            failures.push(format!(
                "{id}: answered {answered}, expected {}",
                case["expect"]
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn only_store_names_are_team_directories() {
    use vibememory_mcp::layout::{self, TeamDir};
    assert_eq!(
        layout::team_dir("acme.git"),
        Some(TeamDir::Store("acme".to_owned()))
    );
    assert_eq!(
        layout::team_dir("acme.deleted-2026-09-01.git"),
        Some(TeamDir::Deleted {
            slug: "acme".to_owned(),
            date: "2026-09-01".to_owned()
        })
    );
    for name in [
        "acme",
        "Acme.git",
        "acme.deleted-2026-02-30.git",
        "acme.deleted-.git",
        ".git",
        "-acme.git",
    ] {
        assert_eq!(layout::team_dir(name), None, "{name}");
    }
    assert_eq!(layout::store_dir("acme"), "acme.git");
    assert_eq!(
        layout::retired_dir("acme", "2026-09-01"),
        "acme.deleted-2026-09-01.git"
    );
}
