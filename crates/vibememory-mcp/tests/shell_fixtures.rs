//! The forced command of a machine key against `fixtures/shell/originalCommands.json` and the
//! refusal table of `docs/manuals/hostShellSpec.md`: every case is answered as the fixture says,
//! and every code of the table has a case.

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
use vibememory_mcp::layout;
use vibememory_mcp::shell::{self, ShellAction};

fn action_json(action: &ShellAction) -> Value {
    match action {
        ShellAction::Upload { team } => json!({"action": "upload", "team": team}),
        ShellAction::Receive { team } => json!({"action": "receive", "team": team}),
        ShellAction::Mcp { team, agent } => json!({"action": "mcp", "team": team, "agent": agent}),
        ShellAction::Status => json!({"action": "status"}),
    }
}

#[test]
fn every_command_is_answered_as_the_fixture_says() {
    let file = support::fixture("fixtures/shell/originalCommands.json");
    let now = file["now"].as_str().expect("now");
    let codes = support::spec_codes("docs/manuals/hostShellSpec.md", "Отказ `shell`");
    assert!(
        codes.len() >= 7,
        "the refusal table was not found in the spec: {codes:?}"
    );

    let mut failures = Vec::new();
    let mut covered = BTreeSet::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let snapshot = support::snapshot(&[&file["snapshotPatch"], &case["patch"]]);
        let usable = case["snapshotUsable"].as_bool().unwrap_or(true);
        let answer = shell::decide(
            case["command"].as_str().expect("command"),
            case["key"].as_str().expect("key"),
            usable.then_some(&snapshot),
            layout::TEAMS_DIR,
            now,
        );
        let answered = match &answer {
            Ok(action) => action_json(action),
            Err(refusal) => json!({"code": refusal.code}),
        };
        if let Some(code) = case["expect"]["code"].as_str() {
            if codes.contains(code) {
                covered.insert(code.to_owned());
            } else {
                failures.push(format!("{id}: code {code:?} is not in the spec"));
            }
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
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
