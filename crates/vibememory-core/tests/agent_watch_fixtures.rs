//! Data-driven tests for watching an agent without hooks: which logs are new, and whether the agent
//! has gone silent. Every expectation lives in `fixtures/foreign/agentWatch.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use serde::Deserialize;
use vibememory_core::agent_watch::{changed, is_silent};

const CASES: &str = include_str!("../../../fixtures/foreign/agentWatch.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseFile {
    #[allow(dead_code)]
    description: String,
    changed: Vec<ChangedCase>,
    silent: Vec<SilentCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangedCase {
    id: String,
    #[allow(dead_code)]
    note: String,
    seen: BTreeMap<String, u64>,
    found: BTreeMap<String, u64>,
    expect: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SilentCase {
    id: String,
    #[allow(dead_code)]
    note: String,
    newest_log: u64,
    last_put: Option<u64>,
    registered: u64,
    expect: bool,
}

fn cases() -> CaseFile {
    serde_json::from_str(CASES).unwrap()
}

#[test]
fn logs_new_since_the_last_look() {
    for case in cases().changed {
        assert_eq!(changed(&case.seen, &case.found), case.expect, "{}", case.id);
    }
}

#[test]
fn an_agent_goes_silent() {
    for case in cases().silent {
        assert_eq!(
            is_silent(case.newest_log, case.last_put, case.registered),
            case.expect,
            "{}",
            case.id
        );
    }
}
