//! The application of a snapshot against `fixtures/host/applyPlans.json`: the steps for the team
//! directories, the problems with codes the spec defines, and `vmgit`'s `authorized_keys`.

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

use serde_json::{Value, json};
use vibememory_mcp::apply::{self, Step};

fn step_json(step: &Step) -> Value {
    match step {
        Step::Create { slug } => json!({"create": slug}),
        Step::Keep { slug } => json!({"keep": slug}),
        Step::Retire { slug, to } => json!({"retire": slug, "to": to}),
    }
}

#[test]
fn every_host_is_planned_as_the_fixture_says() {
    let file = support::fixture("fixtures/host/applyPlans.json");
    let codes = support::spec_codes("docs/manuals/hostStatusSpec.md", "Коды `problems`");
    assert!(
        codes.len() >= 4,
        "the problems table was not found in the spec: {codes:?}"
    );
    let mut failures = Vec::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let snapshot = support::snapshot(&[&file["snapshotPatch"], &case["patch"]]);
        let dirs: Vec<String> = case["dirs"]
            .as_array()
            .expect("dirs")
            .iter()
            .map(|dir| dir.as_str().expect("a directory name").to_owned())
            .collect();
        let plan = apply::plan(&snapshot, &dirs);
        let planned = json!({
            "steps": plan.steps.iter().map(step_json).collect::<Vec<_>>(),
            "problems": serde_json::to_value(&plan.problems).expect("problems encode"),
        });
        if planned != case["expect"] {
            failures.push(format!(
                "{id}: planned {planned}, expected {}",
                case["expect"]
            ));
        }
        for problem in &plan.problems {
            if !codes.contains(&problem.code) {
                failures.push(format!("{id}: code {} is not in the spec", problem.code));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_key_is_bound_to_the_shell() {
    let file = support::fixture("fixtures/host/applyPlans.json");
    let snapshot = support::snapshot(&[&file["snapshotPatch"]]);
    let wanted = &file["authorizedKeys"];
    let written = apply::authorized_keys(&snapshot, wanted["binary"].as_str().expect("binary"));
    assert_eq!(written, wanted["expect"].as_str().expect("expect"));
}
