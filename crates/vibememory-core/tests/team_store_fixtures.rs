//! Data-driven tests for the record of a team store (`fixtures/claim/storeRecords.json`), and for
//! the record `connect` makes of each key answer of `fixtures/claim/claimAnswers.json`: a team
//! whose sessions are on gets one, a team whose sessions are off gets none — and for the host keys
//! `connect --refresh` rewrites the team's `known_hosts` from (`fixtures/claim/hostKeysAnswers.json`).

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::claim::{Claim, read_answer};
use vibememory_core::team_store::{RecordError, StoreRecord, known_hosts, refreshed_known_hosts};

const RECORDS: &str = include_str!("../../../fixtures/claim/storeRecords.json");
const ANSWERS: &str = include_str!("../../../fixtures/claim/claimAnswers.json");
const HOST_KEYS: &str = include_str!("../../../fixtures/claim/hostKeysAnswers.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<RecordCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordCase {
    id: String,
    #[allow(dead_code)]
    provenance: String,
    note: String,
    text: String,
    expect: RecordExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordExpect {
    #[serde(default)]
    ok: Option<RecordOk>,
    /// `invalid` or `field`.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecordOk {
    git_url: String,
}

fn code(error: &RecordError) -> &'static str {
    match error {
        RecordError::Invalid(_) => "invalid",
        RecordError::Field(_) => "field",
    }
}

#[test]
fn store_record_fixtures() {
    let file: RecordFile = serde_json::from_str(RECORDS).expect("storeRecords.json");
    let mut failures = Vec::new();
    for case in &file.cases {
        let label = format!("storeRecords.json#{}", case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }
        match (
            StoreRecord::parse(&case.text),
            &case.expect.ok,
            &case.expect.error,
        ) {
            (Ok(record), Some(want), None) => {
                if record.git_url() != want.git_url {
                    failures.push(format!("{label}: gitUrl {}", record.git_url()));
                }
                // what connect writes reads back as the same record
                if StoreRecord::parse(&record.to_json()).as_ref() != Ok(&record) {
                    failures.push(format!("{label}: to_json does not read back"));
                }
            }
            (Err(error), None, Some(want)) if code(&error) == want => {}
            (actual, _, _) => failures.push(format!("{label}: got {actual:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[derive(Deserialize)]
struct AnswerFile {
    asked: String,
    cases: Vec<serde_json::Value>,
}

#[test]
fn a_record_only_for_a_team_with_sessions() {
    let file: AnswerFile = serde_json::from_str(ANSWERS).expect("claimAnswers.json");
    let key = file
        .cases
        .iter()
        .find(|case| case["id"] == "key")
        .expect("a key case");
    let body = key["body"]
        .as_str()
        .map_or_else(|| key["body"].to_string(), str::to_owned);
    let Ok(Claim::Key(grant)) = read_answer(0, 200, &body, &file.asked) else {
        panic!("claimAnswers.json#key is not a key answer");
    };
    let record = StoreRecord::from_grant(&grant).expect("a sync team has a record");
    assert_eq!(record.store_name, grant.store_name);
    assert_eq!(StoreRecord::parse(&record.to_json()), Ok(record));

    let mut off = grant.clone();
    off.mode = "memory".to_owned();
    assert_eq!(StoreRecord::from_grant(&off), None);

    let hosts = known_hosts(&grant.ssh_host, &grant.host_keys).expect("host keys");
    assert!(
        hosts
            .lines()
            .all(|line| line.starts_with(&format!("{} ssh-ed25519 ", grant.ssh_host)))
    );
    assert_eq!(known_hosts(&grant.ssh_host, &[]), None);
}

#[test]
fn host_keys_fixtures() {
    let records: RecordFile = serde_json::from_str(RECORDS).expect("storeRecords.json");
    let written = records
        .cases
        .iter()
        .find(|case| case.id == "written")
        .expect("storeRecords.json#written");
    let record = StoreRecord::parse(&written.text).expect("the written record");
    let file: serde_json::Value = serde_json::from_str(HOST_KEYS).expect("hostKeysAnswers.json");
    let mut failures = Vec::new();
    for case in file["cases"].as_array().expect("cases") {
        let label = format!("hostKeysAnswers.json#{}", case["id"]);
        let actual = refreshed_known_hosts(case["body"].as_str().expect("body"), &record);
        match (&case["expect"]["ok"], actual) {
            (serde_json::Value::String(want), Ok(text)) if *want == text => {}
            (serde_json::Value::Null, Err(_)) if case["expect"].get("error").is_some() => {}
            (_, actual) => failures.push(format!("{label}: got {actual:?}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
