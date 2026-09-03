//! Data-driven tests for Desktop session cards: the export ratchet, the import gate, the local
//! repair and the translation of a working directory between machines. Every expectation lives
//! in `fixtures/desktop/descriptorScenarios.json`.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use serde::Deserialize;
use vibememory_core::desktop::{
    Descriptor, ExportVerdict, ImportVerdict, MachineFacts, RootError, Roots, SkipReason,
    WithholdReason, export_verdict, import_verdict, shadow_repair,
};
use vibememory_core::naming::PathSyntax;

const SCENARIOS: &str = include_str!("../../../fixtures/desktop/descriptorScenarios.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Provenance {
    Observed,
    Computed,
    Unverified,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScenarioFile {
    #[allow(dead_code)]
    description: String,
    machines: BTreeMap<String, Machine>,
    root_cases: Vec<RootCase>,
    export_cases: Vec<ExportCase>,
    import_cases: Vec<ImportCase>,
    repair_cases: Vec<RepairCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Machine {
    syntax: PathSyntax,
    roots: BTreeMap<String, String>,
}

impl Machine {
    fn roots(&self) -> Roots {
        Roots::new(self.roots.clone(), self.syntax)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RootCase {
    id: String,
    provenance: Provenance,
    note: String,
    machine: String,
    #[serde(default)]
    to_portable: Option<String>,
    #[serde(default)]
    to_local: Option<String>,
    /// Translate the portable result back on this machine: the cross-machine round trip.
    #[serde(default)]
    then_to_local_on: Option<String>,
    expect: RootExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootExpect {
    #[serde(default)]
    portable: Option<String>,
    #[serde(default)]
    local: Option<String>,
    #[serde(default)]
    error: Option<ExpectedRootError>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedRootError {
    Outside,
    UnknownRoot,
    NotPortable,
}

impl ExpectedRootError {
    fn matches(&self, error: &RootError) -> bool {
        matches!(
            (self, error),
            (Self::Outside, RootError::Outside { .. })
                | (Self::UnknownRoot, RootError::UnknownRoot { .. })
                | (Self::NotPortable, RootError::NotPortable { .. })
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExportCase {
    id: String,
    provenance: Provenance,
    note: String,
    machine: String,
    /// The version last written to the outbox, when there is one.
    #[serde(default)]
    exported: Option<Descriptor>,
    local: Descriptor,
    expect: ExportExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ExportExpect {
    verdict: ExpectedExport,
    #[serde(default)]
    reason: Option<ExpectedWithhold>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    origin_cwd: Option<String>,
    /// Keys of the card that the engine does not interpret and must hand on untouched.
    #[serde(default)]
    keeps: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedExport {
    Export,
    Withhold,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedWithhold {
    LostTranscript,
    MarkedUnavailable,
    CwdOutsideRoots,
}

impl ExpectedWithhold {
    fn matches(&self, reason: WithholdReason) -> bool {
        matches!(
            (self, reason),
            (Self::LostTranscript, WithholdReason::LostTranscript)
                | (Self::MarkedUnavailable, WithholdReason::MarkedUnavailable)
                | (Self::CwdOutsideRoots, WithholdReason::CwdOutsideRoots)
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ImportCase {
    id: String,
    provenance: Provenance,
    note: String,
    machine: String,
    remote: Descriptor,
    facts: Facts,
    expect: ImportExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ImportExpect {
    verdict: ExpectedImport,
    #[serde(default)]
    reason: Option<ExpectedSkip>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    origin_cwd: Option<String>,
    #[serde(default)]
    keeps: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedImport {
    Import,
    Skip,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum ExpectedSkip {
    NoTranscript,
    TranscriptMissing,
    CwdMissing,
    LinkUnconfirmed,
    RootUnknown,
}

impl ExpectedSkip {
    fn matches(&self, reason: SkipReason) -> bool {
        matches!(
            (self, reason),
            (Self::NoTranscript, SkipReason::NoTranscript)
                | (Self::TranscriptMissing, SkipReason::TranscriptMissing)
                | (Self::CwdMissing, SkipReason::CwdMissing)
                | (Self::LinkUnconfirmed, SkipReason::LinkUnconfirmed)
                | (Self::RootUnknown, SkipReason::RootUnknown)
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Facts {
    cwd_exists: Option<String>,
    transcript_in_store: bool,
    confirmed_transcript_path: Option<String>,
}

impl Facts {
    fn as_facts(&self) -> MachineFacts<'_> {
        MachineFacts {
            cwd_exists: self.cwd_exists.as_deref(),
            transcript_in_store: self.transcript_in_store,
            confirmed_transcript_path: self.confirmed_transcript_path.as_deref(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RepairCase {
    id: String,
    provenance: Provenance,
    note: String,
    local: Descriptor,
    exported: Descriptor,
    facts: Facts,
    expect: RepairExpect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RepairExpect {
    repaired: bool,
    #[serde(default)]
    cli_session_id: Option<String>,
}

/// Translates a card's directories, or reports that they do not translate.
fn portable_cwd(roots: &Roots, card: &Descriptor) -> Option<(String, Option<String>)> {
    let cwd = roots.to_portable(card.cwd.as_deref()?).ok()?;
    let origin = match card.origin_cwd.as_deref() {
        None => None,
        Some(origin) => Some(roots.to_portable(origin).ok()?),
    };
    Some((cwd, origin))
}

fn local_cwd(roots: &Roots, card: &Descriptor) -> Option<(String, Option<String>)> {
    let cwd = roots.to_local(card.cwd.as_deref()?).ok()?;
    let origin = match card.origin_cwd.as_deref() {
        None => None,
        Some(origin) => Some(roots.to_local(origin).ok()?),
    };
    Some((cwd, origin))
}

fn check_note(
    label: &str,
    note: &str,
    id: &str,
    seen: &mut Vec<String>,
    failures: &mut Vec<String>,
) {
    if seen.contains(&id.to_owned()) {
        failures.push(format!("{label}: duplicate id"));
    }
    seen.push(id.to_owned());
    if note.trim().is_empty() {
        failures.push(format!("{label}: empty note"));
    }
}

fn check_kept(label: &str, card: &Descriptor, keeps: &[String], failures: &mut Vec<String>) {
    for key in keeps {
        if !card.rest.contains_key(key) {
            failures.push(format!(
                "{label}: the card lost the key {key:?} the engine does not interpret"
            ));
        }
    }
}

/// The roots of the machine a case names; a case naming an undeclared machine is a broken case.
fn machine_roots(file: &ScenarioFile, name: &str) -> Roots {
    file.machines
        .get(name)
        .unwrap_or_else(|| panic!("machine {name} is not declared"))
        .roots()
}

fn check_root_cases(file: &ScenarioFile, seen: &mut Vec<String>, failures: &mut Vec<String>) {
    for case in &file.root_cases {
        let label = format!("rootCases#{} [{:?}]", case.id, case.provenance);
        check_note(&label, &case.note, &case.id, seen, failures);
        let roots = machine_roots(file, &case.machine);

        if let Some(path) = &case.to_portable {
            match roots.to_portable(path) {
                Ok(portable) => {
                    if case.expect.portable.as_deref() != Some(portable.as_str()) {
                        failures.push(format!(
                            "{label}: portable {portable:?}, expected {:?}",
                            case.expect.portable
                        ));
                    }
                    if let Some(other) = &case.then_to_local_on {
                        match machine_roots(file, other).to_local(&portable) {
                            Ok(local) => {
                                if case.expect.local.as_deref() != Some(local.as_str()) {
                                    failures.push(format!(
                                        "{label}: local on {other} {local:?}, expected {:?}",
                                        case.expect.local
                                    ));
                                }
                            }
                            Err(error) => {
                                failures.push(format!("{label}: round trip failed with {error:?}"));
                            }
                        }
                    }
                }
                Err(error) => match &case.expect.error {
                    Some(expected) if expected.matches(&error) => {}
                    Some(expected) => {
                        failures.push(format!("{label}: {error:?}, expected {expected:?}"));
                    }
                    None => failures.push(format!("{label}: unexpected {error:?}")),
                },
            }
        }

        if let Some(text) = &case.to_local {
            match roots.to_local(text) {
                Ok(local) => {
                    if case.expect.local.as_deref() != Some(local.as_str()) {
                        failures.push(format!(
                            "{label}: local {local:?}, expected {:?}",
                            case.expect.local
                        ));
                    }
                }
                Err(error) => match &case.expect.error {
                    Some(expected) if expected.matches(&error) => {}
                    Some(expected) => {
                        failures.push(format!("{label}: {error:?}, expected {expected:?}"));
                    }
                    None => failures.push(format!("{label}: unexpected {error:?}")),
                },
            }
        }
    }
}

fn check_export_cases(file: &ScenarioFile, seen: &mut Vec<String>, failures: &mut Vec<String>) {
    for case in &file.export_cases {
        let label = format!("exportCases#{} [{:?}]", case.id, case.provenance);
        check_note(&label, &case.note, &case.id, seen, failures);
        let roots = machine_roots(file, &case.machine);
        let translated = portable_cwd(&roots, &case.local);

        match (
            export_verdict(case.exported.as_ref(), &case.local, translated),
            &case.expect.verdict,
        ) {
            (ExportVerdict::Export { descriptor }, ExpectedExport::Export) => {
                if descriptor.cwd.as_deref() != case.expect.cwd.as_deref() {
                    failures.push(format!(
                        "{label}: cwd {:?}, expected {:?}",
                        descriptor.cwd, case.expect.cwd
                    ));
                }
                if case.expect.origin_cwd.is_some()
                    && descriptor.origin_cwd.as_deref() != case.expect.origin_cwd.as_deref()
                {
                    failures.push(format!(
                        "{label}: originCwd {:?}, expected {:?}",
                        descriptor.origin_cwd, case.expect.origin_cwd
                    ));
                }
                if descriptor.session_id != case.local.session_id {
                    failures.push(format!("{label}: the card changed identity"));
                }
                check_kept(&label, &descriptor, &case.expect.keeps, failures);
            }
            (ExportVerdict::Withhold { reason, rule }, ExpectedExport::Withhold) => {
                let Some(expected) = &case.expect.reason else {
                    failures.push(format!("{label}: a withheld card must name its reason"));
                    continue;
                };
                if !expected.matches(reason) {
                    failures.push(format!("{label}: {reason:?}, expected {expected:?}"));
                }
                if rule.trim().is_empty() {
                    failures.push(format!("{label}: no reason to show"));
                }
            }
            (got, expected) => {
                failures.push(format!("{label}: {got:?}, expected {expected:?}"));
            }
        }
    }
}

fn check_import_cases(file: &ScenarioFile, seen: &mut Vec<String>, failures: &mut Vec<String>) {
    for case in &file.import_cases {
        let label = format!("importCases#{} [{:?}]", case.id, case.provenance);
        check_note(&label, &case.note, &case.id, seen, failures);
        let roots = machine_roots(file, &case.machine);
        let translated = local_cwd(&roots, &case.remote);

        match (
            import_verdict(&case.remote, translated, case.facts.as_facts()),
            &case.expect.verdict,
        ) {
            (ImportVerdict::Import { descriptor }, ExpectedImport::Import) => {
                if descriptor.cwd.as_deref() != case.expect.cwd.as_deref() {
                    failures.push(format!(
                        "{label}: cwd {:?}, expected {:?}",
                        descriptor.cwd, case.expect.cwd
                    ));
                }
                if case.expect.origin_cwd.is_some()
                    && descriptor.origin_cwd.as_deref() != case.expect.origin_cwd.as_deref()
                {
                    failures.push(format!(
                        "{label}: originCwd {:?}, expected {:?}",
                        descriptor.origin_cwd, case.expect.origin_cwd
                    ));
                }
                check_kept(&label, &descriptor, &case.expect.keeps, failures);
            }
            (ImportVerdict::Skip { reason, rule }, ExpectedImport::Skip) => {
                let Some(expected) = &case.expect.reason else {
                    failures.push(format!("{label}: a skipped card must name its reason"));
                    continue;
                };
                if !expected.matches(reason) {
                    failures.push(format!("{label}: {reason:?}, expected {expected:?}"));
                }
                if rule.trim().is_empty() {
                    failures.push(format!("{label}: no reason to show"));
                }
            }
            (got, expected) => {
                failures.push(format!("{label}: {got:?}, expected {expected:?}"));
            }
        }
    }
}

fn check_repair_cases(file: &ScenarioFile, seen: &mut Vec<String>, failures: &mut Vec<String>) {
    for case in &file.repair_cases {
        let label = format!("repairCases#{} [{:?}]", case.id, case.provenance);
        check_note(&label, &case.note, &case.id, seen, failures);

        let repaired = shadow_repair(&case.local, &case.exported, case.facts.as_facts());
        match (&repaired, case.expect.repaired) {
            (Some(card), true) => {
                if card.cli_session_id.as_deref() != case.expect.cli_session_id.as_deref() {
                    failures.push(format!(
                        "{label}: cliSessionId {:?}, expected {:?}",
                        card.cli_session_id, case.expect.cli_session_id
                    ));
                }
                if card.is_transcript_unavailable() {
                    failures.push(format!(
                        "{label}: the repair left the stale mark, so Desktop will mark it again"
                    ));
                }
                if card.cwd != case.local.cwd {
                    failures.push(format!("{label}: the repair moved the working directory"));
                }
            }
            (None, false) => {}
            (got, expected) => {
                failures.push(format!(
                    "{label}: repaired {}, expected {expected}",
                    got.is_some()
                ));
            }
        }
    }
}

#[test]
fn desktop_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("descriptorScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    check_root_cases(&file, &mut seen, &mut failures);
    check_export_cases(&file, &mut seen, &mut failures);
    check_import_cases(&file, &mut seen, &mut failures);
    check_repair_cases(&file, &mut seen, &mut failures);

    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

/// Every card the engine carries has to survive the trip: Desktop's format is internal and the
/// next release may add a key, so a descriptor read and written back keeps what it held.
#[test]
fn an_unknown_key_survives_the_round_trip() {
    let text = r#"{"sessionId":"local_x","cliSessionId":"t","cwd":"/a",
        "somethingTheNextReleaseAdded":{"nested":[1,2,3]},"isStarred":true}"#;
    let card: Descriptor = serde_json::from_str(text).expect("a card");
    let written = serde_json::to_value(&card).expect("json");
    let original: serde_json::Value = serde_json::from_str(text).expect("json");
    assert_eq!(written, original, "the card did not survive the round trip");
}
