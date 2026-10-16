//! Which kind of key a text holds, case by case from `fixtures/secrets/secretsScenarios.json`, and what a memory, a
//! rule and a skill do about it.
//!
//! The fixture never holds a key: it holds the head of one, a filler and a length, and the value is built here. A
//! complete key in a fixture would be flagged by a secret scanner on push and by the privacy gate before that.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;

use serde::Deserialize;
use vibememory_core::memory::{MemoryError, Record};
use vibememory_core::rules::skill::parse_skill;
use vibememory_core::rules::{Rule, RulesError};
use vibememory_core::secrets::{self, SecretKind};

const CASES: &str = include_str!("../../../fixtures/secrets/secretsScenarios.json");

/// Where the built value stands in a case's `text`.
const PLACEHOLDER: &str = "{key}";

const ALL_KINDS: [SecretKind; 9] = [
    SecretKind::OpenAiKey,
    SecretKind::AnthropicKey,
    SecretKind::AwsAccessKey,
    SecretKind::GithubToken,
    SecretKind::SlackToken,
    SecretKind::GoogleApiKey,
    SecretKind::StripeKey,
    SecretKind::PrivateKey,
    SecretKind::VibeMemoryToken,
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[allow(dead_code)]
    description: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    #[allow(dead_code)]
    provenance: String,
    note: String,
    text: String,
    #[serde(default)]
    key: Option<Key>,
    expect: Expect,
}

/// A key as the fixture describes it: pieces of the head, then a filler cycled to a length.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    head: Vec<String>,
    fill: String,
    length: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    /// The code of the kind, or none for a text that holds no key.
    kind: Option<String>,
}

impl Key {
    fn build(&self) -> String {
        let mut value = self.head.concat();
        value.extend(self.fill.chars().cycle().take(self.length));
        value
    }
}

impl Case {
    /// The case's text with the built key in it, and the key itself.
    fn text(&self) -> (String, Option<String>) {
        match &self.key {
            Some(key) => {
                let built = key.build();
                (self.text.replace(PLACEHOLDER, &built), Some(built))
            }
            None => (self.text.clone(), None),
        }
    }
}

fn file() -> File {
    serde_json::from_str(CASES).expect("secretsScenarios.json")
}

fn kind_of(code: &str) -> SecretKind {
    ALL_KINDS
        .into_iter()
        .find(|kind| kind.code() == code)
        .unwrap_or_else(|| panic!("no kind with the code {code}"))
}

#[test]
fn a_text_is_told_to_hold_a_key_of_its_kind_and_nothing_else_is() {
    let mut failures = Vec::new();
    let mut ids = BTreeSet::new();
    for case in &file().cases {
        if !ids.insert(case.id.clone()) || case.note.trim().is_empty() {
            failures.push(format!("{}: repeated id or empty note", case.id));
        }
        let (text, _) = case.text();
        let found = secrets::find(&text).map(SecretKind::code);
        if found != case.expect.kind.as_deref() {
            failures.push(format!(
                "{}: expected {:?}, found {found:?}",
                case.id, case.expect.kind
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn every_kind_has_a_case_that_holds_it_and_the_codes_are_distinct() {
    let scenarios = file();
    let covered: BTreeSet<&str> = scenarios
        .cases
        .iter()
        .filter_map(|case| case.expect.kind.as_deref())
        .collect();
    let codes: BTreeSet<&str> = ALL_KINDS.into_iter().map(SecretKind::code).collect();
    assert_eq!(codes.len(), ALL_KINDS.len(), "two kinds share a code");
    for code in &codes {
        assert!(covered.contains(code), "no case holds {code}");
    }
    for code in covered {
        assert!(codes.contains(code), "a case names the unknown kind {code}");
    }
}

/// A memory record as it is stored, with `body` and `agent` set by the caller.
fn record(body: &str, agent: &str) -> Record {
    serde_json::from_value(serde_json::json!({
        "id": "deploy-setup",
        "kind": "project",
        "project": "VibeMemory",
        "description": "how the service is deployed",
        "body": body,
        "links": [],
        "metadata": { "note": "none" },
        "agent": agent,
        "createdAt": "2026-10-16T10:00:00Z",
        "updatedAt": "2026-10-16T10:00:00Z"
    }))
    .expect("a record")
}

#[test]
fn a_memory_with_a_key_is_refused_by_kind_and_the_message_does_not_carry_the_value() {
    for case in file()
        .cases
        .iter()
        .filter(|case| case.expect.kind.is_some())
    {
        let (text, value) = case.text();
        let kind = kind_of(case.expect.kind.as_deref().unwrap());
        let refused = record(&text, "claude-code").validate().expect_err(&case.id);
        assert_eq!(
            refused,
            MemoryError::HoldsSecret {
                id: "deploy-setup".to_owned(),
                kind
            },
            "{}",
            case.id
        );
        assert_eq!(refused.code(), "holdsSecret");
        let message = refused.to_string();
        assert!(message.contains(kind.name()), "{}: {message}", case.id);
        // The head of a PEM header is not the key; every other value is built whole and must not come back
        if let Some(value) = value.filter(|value| value.len() > 16) {
            assert!(!message.contains(&value), "{}: {message}", case.id);
        }
    }
}

#[test]
fn a_key_is_found_wherever_it_sits_in_a_memory_and_a_clean_one_is_accepted() {
    let key = Key {
        head: vec!["gh".to_owned(), "p_".to_owned()],
        fill: "k9".to_owned(),
        length: 40,
    }
    .build();
    let in_agent = record("the key is in an environment variable", &key).validate();
    assert!(matches!(
        in_agent,
        Err(MemoryError::HoldsSecret {
            kind: SecretKind::GithubToken,
            ..
        })
    ));
    let mut in_metadata = record("the key is in an environment variable", "claude-code");
    in_metadata.metadata.insert("note".to_owned(), key);
    assert!(in_metadata.validate().is_err());
    assert_eq!(
        record(
            "the key is in the environment variable DEPLOY_KEY",
            "claude-code"
        )
        .validate(),
        Ok(())
    );
}

#[test]
fn a_rule_and_a_skill_with_a_key_are_refused_by_kind() {
    let key = Key {
        head: vec!["AK".to_owned(), "IA".to_owned()],
        fill: "Q7".to_owned(),
        length: 16,
    }
    .build();
    let rule = Rule::parse(&format!(
        "---\nid: deploy\ntitle: Deploy\nlevel: personal\n---\nThe key is {key}\n"
    ));
    assert_eq!(
        rule.expect_err("a rule with a key"),
        RulesError::HoldsSecret(SecretKind::AwsAccessKey)
    );
    let in_title = Rule::parse(&format!(
        "---\nid: deploy\ntitle: Key {key}\nlevel: personal\n---\nbody\n"
    ));
    assert_eq!(
        in_title.expect_err("a rule with a key in the title"),
        RulesError::HoldsSecret(SecretKind::AwsAccessKey)
    );
    let skill = parse_skill(
        "deploy",
        &format!("---\nname: deploy\ndescription: ships it\n---\nUse {key}\n"),
    );
    assert_eq!(
        skill.expect_err("a skill with a key"),
        RulesError::HoldsSecret(SecretKind::AwsAccessKey)
    );
    let message = RulesError::HoldsSecret(SecretKind::AwsAccessKey).to_string();
    assert!(!message.contains(&key), "{message}");
    assert!(message.contains("environment variable"), "{message}");
}
