//! Shared scaffolding for the host's fixture gates: the repository root, snapshots built from the
//! base of `fixtures/access/accessSnapshots.json` with patches applied, and the codes a spec lists.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    dead_code
)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value};
use vibememory_mcp::access::{self, Snapshot};

/// The root of the repository, where `fixtures/` and `docs/` live.
pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

/// A fixture of the repository, parsed.
pub fn fixture(relative: &str) -> Value {
    let text = fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|error| panic!("{relative} is readable: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{relative} is JSON: {error}"))
}

/// RFC 7396: objects merge key by key, `null` removes a key, anything else replaces the value whole.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let target = target.as_object_mut().expect("made an object above");
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// The base snapshot of the access fixtures with `patches` applied in order, as the checker
/// passes it: a fixture built on a snapshot the checker refuses would test nothing. A `null` patch
/// is a case without one.
pub fn snapshot(patches: &[&Value]) -> Snapshot {
    let mut snapshot = fixture("fixtures/access/accessSnapshots.json")["base"].clone();
    for patch in patches.iter().filter(|patch| !patch.is_null()) {
        merge_patch(&mut snapshot, patch);
    }
    let bytes = serde_json::to_vec(&snapshot).expect("a JSON value encodes");
    access::check(&bytes).unwrap_or_else(|refusal| {
        panic!(
            "the patched snapshot is refused: {} ({})",
            refusal.code, refusal.detail
        )
    })
}

/// The codes of one table of a spec: the first backticked cell of each row in the section whose
/// title starts with `section`, up to the next section.
pub fn spec_codes(spec: &str, section: &str) -> BTreeSet<String> {
    let text = fs::read_to_string(repo_root().join(spec))
        .unwrap_or_else(|error| panic!("{spec} is readable: {error}"));
    let part = text
        .split("\n## ")
        .find(|part| part.starts_with(section))
        .unwrap_or_else(|| panic!("{spec} has a section {section:?}"));
    part.lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|rest| rest.split_once('`'))
        .map(|(code, _)| code.to_owned())
        .collect()
}

/// The cases of a fixture, each checked for a unique id, a known provenance and a note; what is
/// wrong goes to `failures`.
pub fn cases<'a>(file: &'a Value, failures: &mut Vec<String>) -> Vec<(String, &'a Value)> {
    let mut ids = BTreeSet::new();
    let mut cases = Vec::new();
    for case in file["cases"].as_array().expect("`cases` is an array") {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        if id.is_empty() || !ids.insert(id.clone()) {
            failures.push(format!("case id missing or repeated: {id:?}"));
        }
        if !matches!(
            case["provenance"].as_str(),
            Some("computed" | "observed" | "unverified")
        ) {
            failures.push(format!("{id}: provenance"));
        }
        if case["note"].as_str().is_none_or(str::is_empty) {
            failures.push(format!("{id}: note"));
        }
        cases.push((id, case));
    }
    cases
}
