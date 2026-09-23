//! What `pre-receive` decides, against `fixtures/access/preReceiveUpdates.json` and the table of
//! `docs/manuals/hostShellSpec.md`: every case is answered with its code and paths, and every code
//! of the table has a case.

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

use std::collections::BTreeSet;

use serde_json::{Value, json};
use vibememory_mcp::receive::{self, Push, Sizes, Update};

fn size(case: &Value, defaults: &Value, name: &str) -> u64 {
    case["sizes"][name]
        .as_u64()
        .or_else(|| defaults[name].as_u64())
        .unwrap_or_else(|| panic!("size {name}"))
}

#[test]
fn every_push_is_answered_as_the_fixture_says() {
    let file = support::fixture("fixtures/access/preReceiveUpdates.json");
    let codes = support::spec_codes("docs/manuals/hostShellSpec.md", "`pre-receive`");
    assert!(
        codes.len() >= 10,
        "the pre-receive table was not found in the spec: {codes:?}"
    );
    let defaults = &file["defaults"];

    let mut failures = Vec::new();
    let mut covered = BTreeSet::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let snapshot = support::snapshot(&[&file["snapshotPatch"], &case["patch"]]);
        let usable = case["snapshotUsable"].as_bool().unwrap_or(true);
        let updates: Vec<Update> = case["updates"]
            .as_array()
            .expect("updates")
            .iter()
            .map(|line| Update {
                old: line[0].as_str().expect("old").to_owned(),
                new: line[1].as_str().expect("new").to_owned(),
                name: line[2].as_str().expect("ref").to_owned(),
            })
            .collect();
        let changed: Vec<String> = case["changed"]
            .as_array()
            .expect("changed")
            .iter()
            .map(|path| path.as_str().expect("path").to_owned())
            .collect();
        let push = Push {
            key: case["key"].as_str(),
            team: case["team"].as_str().expect("team"),
            updates: &updates,
            changed: &changed,
            sizes: Sizes {
                repo_bytes: size(case, defaults, "repoBytes"),
                incoming_bytes: size(case, defaults, "incomingBytes"),
                free_bytes: size(case, defaults, "freeBytes"),
                reserve_bytes: size(case, defaults, "reserveBytes"),
            },
        };
        let answer = receive::decide(&push, usable.then_some(&snapshot));
        let answered = match &answer {
            Ok(()) => json!({"code": "ok"}),
            Err(refusal) if refusal.paths.is_empty() => json!({"code": refusal.code}),
            Err(refusal) => json!({"code": refusal.code, "paths": refusal.paths}),
        };
        let code = case["expect"]["code"].as_str().unwrap_or_default();
        if code == "ok" || codes.contains(code) {
            covered.insert(code.to_owned());
        } else {
            failures.push(format!("{id}: code {code:?} is not in the spec"));
        }
        if answered != case["expect"] {
            let detail = answer
                .err()
                .map(|refusal| refusal.detail)
                .unwrap_or_default();
            failures.push(format!(
                "{id}: answered {answered} ({detail}), expected {}",
                case["expect"]
            ));
        }
    }
    for code in &codes {
        if !covered.contains(code) {
            failures.push(format!("spec code {code} has no case"));
        }
    }
    assert!(covered.contains("ok"), "no case of a push that lands");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn update_lines_are_read_strictly() {
    let zero = "0".repeat(40);
    let one = "1".repeat(40);
    let lines = format!("{zero} {one} refs/heads/main\n{one} {zero} refs/tags/v1\n");
    let updates = receive::parse_updates(&lines).expect("two lines");
    assert_eq!(updates.len(), 2);
    assert_eq!(
        receive::main_update(&updates).map(|update| update.new.as_str()),
        Some(one.as_str())
    );
    assert!(updates[1].deletes());
    assert!(receive::parse_updates(&format!("{zero} {one}\n")).is_err());
    assert!(receive::parse_updates(&format!("{zero} {one} refs/heads/main extra\n")).is_err());
}
