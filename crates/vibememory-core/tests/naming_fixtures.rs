//! Data-driven tests: every expectation about store naming lives in `fixtures/naming/*.json`.
//! A case records where its input came from (`provenance`), what it proves (`note`), the input
//! and either an `ok` value or an error `code`. Mismatches are collected and reported together.

// Integration-test helpers are test code even though clippy's `allow-*-in-tests` only sees
// `#[test]` functions: a broken fixture file or a violated invariant must stop the run.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde::Deserialize;
use serde_json::{Value, json};
use vibememory_core::naming::{
    GitProbe, IgnoreReason, NamingConfig, NamingError, NamingInput, PathSyntax, RawNamingConfig,
    Resolution, StoreName, canonical_cwd, conflict_copies, enc_from_transcript_path, encode_cwd,
    resolve_store_name,
};

const ENC_FROM_TRANSCRIPT_PATH: &str =
    include_str!("../../../fixtures/naming/encFromTranscriptPath.json");
const ENCODE_CWD: &str = include_str!("../../../fixtures/naming/encodeCwd.json");
const RESOLVE_STORE_NAME: &str = include_str!("../../../fixtures/naming/resolveStoreName.json");
const NAMING_CONFIG: &str = include_str!("../../../fixtures/naming/namingConfig.json");
const CANONICAL_CWD: &str = include_str!("../../../fixtures/naming/canonicalCwd.json");
const CONFLICT_COPIES: &str = include_str!("../../../fixtures/naming/conflictCopies.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct FixtureFile<I> {
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    #[allow(dead_code)]
    cli_versions: Vec<String>,
    cases: Vec<Case<I>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case<I> {
    id: String,
    provenance: Provenance,
    note: String,
    input: I,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Expect {
    #[serde(default)]
    ok: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TranscriptPathInput {
    transcript_path: String,
    syntax: PathSyntax,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CwdInput {
    cwd: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ResolveInput {
    cwd: String,
    syntax: PathSyntax,
    #[serde(default)]
    project_dir_name: Option<String>,
    /// Absent means "git must not be consulted": the closure panics if called.
    #[serde(default)]
    git: Option<GitProbe>,
    #[serde(default)]
    config: Option<RawNamingConfig>,
    #[serde(default)]
    existing: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CanonicalInput {
    raw: String,
    syntax: PathSyntax,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameInput {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigInput {
    config: RawNamingConfig,
}

/// Runs every case of one fixture file through `run` and returns the mismatches.
fn check<I: for<'de> Deserialize<'de>>(
    file_name: &str,
    text: &str,
    run: impl Fn(&I) -> Result<Value, NamingError>,
) -> Vec<String> {
    let file: FixtureFile<I> = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("{file_name}: invalid fixture file: {e}"));
    let mut failures = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for case in &file.cases {
        let label = format!("{file_name}#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }
        let actual = run(&case.input);
        match (&case.expect.ok, &case.expect.error, actual) {
            (Some(expected), None, Ok(value)) if *expected == value => {}
            (Some(expected), None, Ok(value)) => {
                failures.push(format!("{label}: expected {expected}, got {value}"));
            }
            (Some(expected), None, Err(e)) => {
                failures.push(format!(
                    "{label}: expected {expected}, got error {} ({e})",
                    e.code()
                ));
            }
            (None, Some(code), Err(e)) if code == e.code() => {}
            (None, Some(code), Err(e)) => {
                failures.push(format!(
                    "{label}: expected error {code}, got error {} ({e})",
                    e.code()
                ));
            }
            (None, Some(code), Ok(value)) => {
                failures.push(format!("{label}: expected error {code}, got {value}"));
            }
            _ => failures.push(format!(
                "{label}: expect must have exactly one of `ok` / `error`"
            )),
        }
    }
    failures
}

fn resolution_value(resolution: &Resolution) -> Value {
    match resolution {
        Resolution::Named { name, source } => {
            json!({ "named": { "name": name.as_str(), "source": source.code() } })
        }
        Resolution::Ignored {
            reason: IgnoreReason::Pattern(pattern),
        } => json!({ "ignored": { "pattern": pattern } }),
        Resolution::Ignored {
            reason: IgnoreReason::ProjectDirName(name),
        } => {
            json!({ "ignored": { "projectDirName": name } })
        }
    }
}

fn resolve(input: &ResolveInput) -> Result<Value, NamingError> {
    let config = match &input.config {
        Some(raw) => NamingConfig::from_raw(raw)?,
        None => NamingConfig::default(),
    };
    let existing = input
        .existing
        .iter()
        .map(|name| StoreName::parse(name))
        .collect::<Result<Vec<_>, _>>()?;
    let naming = NamingInput {
        cwd: &input.cwd,
        syntax: input.syntax,
        project_dir_name: input.project_dir_name.as_deref(),
    };
    let probe = || {
        input
            .git
            .clone()
            .unwrap_or_else(|| panic!("git must not be consulted for cwd {:?}", input.cwd))
    };
    resolve_store_name(&naming, probe, &config, &existing).map(|r| resolution_value(&r))
}

fn assert_no_failures(failures: &[String]) {
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn enc_from_transcript_path_fixtures() {
    let failures = check(
        "encFromTranscriptPath.json",
        ENC_FROM_TRANSCRIPT_PATH,
        |input: &TranscriptPathInput| {
            enc_from_transcript_path(&input.transcript_path, input.syntax)
                .map(|enc| json!(enc.as_str()))
        },
    );
    assert_no_failures(&failures);
}

#[test]
fn encode_cwd_fixtures() {
    let failures = check("encodeCwd.json", ENCODE_CWD, |input: &CwdInput| {
        encode_cwd(&input.cwd).map(|enc| json!(enc.as_str()))
    });
    assert_no_failures(&failures);
}

#[test]
fn resolve_store_name_fixtures() {
    let failures = check("resolveStoreName.json", RESOLVE_STORE_NAME, resolve);
    assert_no_failures(&failures);
}

#[test]
fn canonical_cwd_fixtures() {
    let failures = check(
        "canonicalCwd.json",
        CANONICAL_CWD,
        |input: &CanonicalInput| {
            let once = canonical_cwd(&input.raw, input.syntax);
            // Canonicalization happens once, at the edge — but a second pass has to be a no-op,
            // or "once" is a claim nobody can check.
            let twice = canonical_cwd(&once, input.syntax);
            assert_eq!(once, twice, "canonicalization is not idempotent");
            Ok(json!(once))
        },
    );
    assert_no_failures(&failures);
}

#[test]
fn conflict_copies_fixtures() {
    let failures = check(
        "conflictCopies.json",
        CONFLICT_COPIES,
        |input: &NameInput| {
            let found = conflict_copies(&input.name);
            // Every candidate must name a shorter original than the one before it: the caller
            // takes the first one it can find on disk, so the order is part of the contract.
            for pair in found.windows(2) {
                assert!(
                    pair[0].original.len() > pair[1].original.len(),
                    "candidates must run from the longest original down: {found:?}"
                );
            }
            Ok(json!(
                found
                    .iter()
                    .map(|copy| json!({"original": copy.original, "machine": copy.machine}))
                    .collect::<Vec<_>>()
            ))
        },
    );
    assert_no_failures(&failures);
}

#[test]
fn naming_config_fixtures() {
    let failures = check("namingConfig.json", NAMING_CONFIG, |input: &ConfigInput| {
        NamingConfig::from_raw(&input.config).map(|_| json!(true))
    });
    assert_no_failures(&failures);
}
