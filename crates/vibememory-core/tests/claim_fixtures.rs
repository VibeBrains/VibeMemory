//! Data-driven tests for the claim: what the cabinet's answer gives the engine and what it is
//! refused for (`fixtures/claim/claimAnswers.json`), and who `icacls` says may reach a token file
//! (`fixtures/claim/icaclsOutput.json`). The cabinet's own test reads the first file too.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use vibememory_core::claim::{
    AccessList, Claim, ClaimFailure, access_list, client_fragment, read_answer, token_sidecar,
};

const ANSWERS: &str = include_str!("../../../fixtures/claim/claimAnswers.json");
const ICACLS: &str = include_str!("../../../fixtures/claim/icaclsOutput.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<AnswerCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerCase {
    id: String,
    #[allow(dead_code)]
    provenance: Provenance,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
    exit: i32,
    #[serde(default)]
    body: Option<serde_json::Value>,
    #[serde(default)]
    raw: Option<String>,
    expect: AnswerExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
enum AnswerExpect {
    Token(serde_json::Map<String, serde_json::Value>),
    Key(serde_json::Map<String, serde_json::Value>),
    Refused(Option<()>),
    Unreachable(Option<()>),
    Malformed(Option<()>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IcaclsFile {
    #[allow(dead_code)]
    description: String,
    cases: Vec<IcaclsCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IcaclsCase {
    id: String,
    #[allow(dead_code)]
    provenance: Provenance,
    #[serde(default)]
    #[allow(dead_code)]
    note: Option<String>,
    path: String,
    user: String,
    output: String,
    expect: IcaclsExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
enum IcaclsExpect {
    Others(Vec<String>),
    Unreadable(Option<()>),
}

/// What stdout carried in a case: the JSON body as the cabinet serializes it, or the raw text.
fn stdout_of(case: &AnswerCase) -> String {
    match (&case.body, &case.raw) {
        (Some(body), None) => serde_json::to_string(body).unwrap(),
        (None, Some(raw)) => raw.clone(),
        _ => panic!("{}: a case has a body or a raw stdout, not both", case.id),
    }
}

/// Every field the case pins, compared with the grant as JSON.
fn assert_fields(
    id: &str,
    grant: &serde_json::Value,
    pinned: &serde_json::Map<String, serde_json::Value>,
) {
    for (field, value) in pinned {
        assert_eq!(&grant[field], value, "{id}: {field}");
    }
}

#[test]
fn every_claim_answer_reads_as_the_fixture_says() {
    let file: AnswerFile = serde_json::from_str(ANSWERS).unwrap();
    assert!(!file.cases.is_empty(), "no claim answers were checked");
    for case in &file.cases {
        let answer = read_answer(case.exit, &stdout_of(case));
        match (&case.expect, &answer) {
            (AnswerExpect::Token(pinned), Ok(Claim::Token(grant))) => {
                let as_json = serde_json::json!({
                    "team": grant.team, "member": grant.member, "agent": grant.agent,
                    "tokenId": grant.token_id, "cabinet": grant.cabinet, "mcpUrl": grant.mcp_url,
                });
                assert_fields(&case.id, &as_json, pinned);
            }
            (AnswerExpect::Key(pinned), Ok(Claim::Key(grant))) => {
                let as_json = serde_json::json!({
                    "team": grant.team, "storeName": grant.store_name, "keyId": grant.key_id,
                    "mode": grant.mode, "remote": grant.remote,
                });
                assert_fields(&case.id, &as_json, pinned);
            }
            (AnswerExpect::Refused(nothing), Err(ClaimFailure::Refused))
            | (AnswerExpect::Unreachable(nothing), Err(ClaimFailure::Unreachable { .. }))
            | (AnswerExpect::Malformed(nothing), Err(ClaimFailure::Malformed(_))) => {
                assert!(
                    nothing.is_none(),
                    "{}: a refusal pins nothing, write null",
                    case.id
                );
            }
            (_, other) => panic!("{}: got {other:?}", case.id),
        }
    }
}

#[test]
fn a_grant_never_prints_its_token() {
    let file: AnswerFile = serde_json::from_str(ANSWERS).unwrap();
    let case = file.cases.iter().find(|case| case.id == "token").unwrap();
    let Ok(Claim::Token(grant)) = read_answer(case.exit, &stdout_of(case)) else {
        panic!("the token case must read")
    };
    let secret = grant.token.split('_').nth(2).unwrap();
    assert!(
        !format!("{grant:?}").contains(secret),
        "Debug shows the token"
    );
    assert!(
        !token_sidecar(&grant).contains(secret),
        "the sidecar holds the token"
    );
    // the one other file that may hold it: the client fragment, written with the token's rights
    assert!(client_fragment(&grant).contains(&format!("Bearer {}", grant.token)));
}

#[test]
fn a_token_file_is_reached_by_its_owner_alone() {
    let file: IcaclsFile = serde_json::from_str(ICACLS).unwrap();
    assert!(!file.cases.is_empty(), "no icacls outputs were checked");
    for case in &file.cases {
        let verdict = access_list(&case.output, &case.path, &case.user);
        let expected = match &case.expect {
            IcaclsExpect::Others(others) if others.is_empty() => AccessList::OwnerOnly,
            IcaclsExpect::Others(others) => AccessList::Others(others.clone()),
            IcaclsExpect::Unreadable(nothing) => {
                assert!(nothing.is_none(), "{}: write null", case.id);
                AccessList::Unreadable
            }
        };
        assert_eq!(verdict, expected, "{}", case.id);
    }
}
