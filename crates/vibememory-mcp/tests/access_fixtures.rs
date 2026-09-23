//! The snapshot fixtures, the spec they are written against, and the checker that enforces it.
//!
//! `fixtures/access/accessSnapshots.json` is the contract the memory server and the cabinet are both
//! built to. Three things must agree: every case is well formed and names a code the spec defines,
//! every code the spec defines has a case, and the checker answers every case with its code — the
//! first rule broken, in the spec's order.

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

use serde_json::Value;
use support::merge_patch;
use vibememory_mcp::access;

/// The only code that cannot have a case: a file that cannot be read has no content to patch.
const CODE_WITHOUT_CASE: &str = "snapshotUnreadable";

#[test]
fn every_case_is_well_formed_and_every_code_has_a_case() {
    let file = support::fixture("fixtures/access/accessSnapshots.json");
    let base = file["base"].clone();
    assert!(base.is_object(), "`base` is a snapshot object");

    let codes = support::spec_codes("docs/manuals/accessSnapshotSpec.md", "Проверка");
    assert!(
        codes.len() > 30,
        "the check table was not found in the spec: {codes:?}"
    );

    let mut covered = BTreeSet::new();
    let mut failures = Vec::new();
    for (id, case) in support::cases(&file, &mut failures) {
        let bytes = match (case.get("patch"), case.get("raw")) {
            (Some(patch), None) => {
                let mut snapshot = base.clone();
                merge_patch(&mut snapshot, patch);
                if !snapshot.is_object() {
                    failures.push(format!("{id}: the patch does not leave a snapshot object"));
                }
                serde_json::to_vec(&snapshot).expect("a JSON value encodes")
            }
            (None, Some(Value::String(raw))) => raw.clone().into_bytes(),
            _ => {
                failures.push(format!("{id}: exactly one of `patch` or `raw` (text)"));
                continue;
            }
        };
        let code = case["expect"]["code"].as_str().unwrap_or_default();
        if code == "ok" || codes.contains(code) {
            covered.insert(code.to_owned());
        } else {
            failures.push(format!("{id}: code {code:?} is not in the spec"));
        }
        let answered = match access::check(&bytes) {
            Ok(_) => "ok".to_owned(),
            Err(refusal) => format!("{} ({})", refusal.code, refusal.detail),
        };
        if answered.split(' ').next() != Some(code) {
            failures.push(format!(
                "{id}: the checker answered {answered}, expected {code}"
            ));
        }
    }
    for code in &codes {
        if code != CODE_WITHOUT_CASE && !covered.contains(code) {
            failures.push(format!("spec code {code} has no case"));
        }
    }
    assert!(covered.contains("ok"), "no case of a valid snapshot");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_merge_patch_removes_with_null_and_replaces_arrays_whole() {
    let mut target = serde_json::json!({"a": {"b": 1, "c": 2}, "list": [1, 2]});
    merge_patch(
        &mut target,
        &serde_json::json!({"a": {"b": null, "d": 3}, "list": [9]}),
    );
    assert_eq!(
        target,
        serde_json::json!({"a": {"c": 2, "d": 3}, "list": [9]})
    );
}
