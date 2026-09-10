//! Data-driven tests for merging JSONL transcripts: every expectation lives in
//! `fixtures/merge/mergeScenarios.json`, built from recorded transcripts of real sessions.
//! Besides the recorded expectation, every case is checked against the laws a merge must obey
//! and against an oracle that recomputes line identity independently of the crate.

// Integration-test helpers are test code even though clippy's `allow-*-in-tests` only sees
// `#[test]` functions: a broken fixture or a violated invariant must stop the run.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use vibememory_core::merge::jsonl::{MergeOutput, merge_jsonl, snapshot_boundary};

const SCENARIOS: &str = include_str!("../../../fixtures/merge/mergeScenarios.json");

/// The recorded transcripts a scenario may draw from. A new fixture file is one line here.
fn transcript(name: &str) -> &'static [u8] {
    match name {
        "cli255Session.jsonl" => include_bytes!("../../../fixtures/merge/cli255Session.jsonl"),
        "cli255Isolated.jsonl" => include_bytes!("../../../fixtures/merge/cli255Isolated.jsonl"),
        "cli232Isolated.jsonl" => include_bytes!("../../../fixtures/merge/cli232Isolated.jsonl"),
        "cli255Subagent.jsonl" => include_bytes!("../../../fixtures/merge/cli255Subagent.jsonl"),
        "cli255Journal.jsonl" => include_bytes!("../../../fixtures/merge/cli255Journal.jsonl"),
        other => {
            panic!("unknown transcript {other:?}: add it to fixtures/merge/ and to this match")
        }
    }
}

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
    #[allow(dead_code)]
    cli_versions: Vec<String>,
    #[allow(dead_code)]
    transcripts: HashMap<String, Value>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    provenance: Provenance,
    note: String,
    input: Input,
    expect: Expect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    base: Option<Doc>,
    ours: Doc,
    theirs: Doc,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Expect {
    result: Vec<Pick>,
    report: Value,
}

/// A recipe for one input: a window of a recorded transcript, then edits.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Doc {
    #[serde(default)]
    empty: bool,
    #[serde(default)]
    file: Option<String>,
    #[serde(default)]
    lines: Option<[usize; 2]>,
    #[serde(default)]
    ops: Vec<Op>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
enum Op {
    /// Cut bytes off the end: 1 removes the final newline, more tears the last record.
    CutTailBytes(usize),
    /// Append complete lines (UTF-8 only).
    AppendLines(Vec<String>),
    /// Append raw bytes, for content no JSON string can carry.
    AppendHex(String),
    /// Remove lines by index in the state before this operation.
    DropLines(Vec<usize>),
    /// Insert a copy of a line right after it.
    DuplicateLine(usize),
    /// Replace one line.
    SetLine(usize, String),
}

/// A slice of an already materialized input: how an expected result is written down.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Pick {
    from: Source,
    #[serde(default)]
    lines: Option<[usize; 2]>,
    /// Individual line numbers, when a range would not do.
    #[serde(default, rename = "pick")]
    indices: Option<Vec<usize>>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Source {
    Base,
    Ours,
    Theirs,
}

fn materialize(doc: &Doc) -> Vec<u8> {
    if doc.empty {
        assert!(
            doc.file.is_none() && doc.ops.is_empty(),
            "an empty doc takes no other field"
        );
        return Vec::new();
    }
    let file = doc.file.as_deref().expect("a doc needs `empty` or `file`");
    let all = split(transcript(file));
    let [from, to] = doc.lines.unwrap_or([0, all.len()]);
    let mut lines: Vec<Vec<u8>> = all[from..to].iter().map(|line| line.to_vec()).collect();
    let mut bytes = join(&lines);
    for op in &doc.ops {
        match op {
            Op::CutTailBytes(count) => {
                bytes = join(&lines);
                bytes.truncate(bytes.len() - count);
                lines = split(&bytes).iter().map(|line| line.to_vec()).collect();
                continue;
            }
            Op::AppendLines(new) => lines.extend(new.iter().map(|line| line.as_bytes().to_vec())),
            Op::AppendHex(hex) => {
                bytes = join(&lines);
                bytes.extend_from_slice(&from_hex(hex));
                lines = split(&bytes).iter().map(|line| line.to_vec()).collect();
                continue;
            }
            Op::DropLines(indices) => {
                let mut kept: Vec<Vec<u8>> = Vec::new();
                for (index, line) in lines.iter().enumerate() {
                    if !indices.contains(&index) {
                        kept.push(line.clone());
                    }
                }
                lines = kept;
            }
            Op::DuplicateLine(index) => lines.insert(index + 1, lines[*index].clone()),
            Op::SetLine(index, line) => lines[*index] = line.as_bytes().to_vec(),
        }
        bytes = join(&lines);
    }
    bytes
}

fn from_hex(hex: &str) -> Vec<u8> {
    assert!(
        hex.len().is_multiple_of(2),
        "hex must have an even number of digits"
    );
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digits"))
        .collect()
}

/// Lines of a byte string, without the torn tail: exactly what the merge sees.
fn split(bytes: &[u8]) -> Vec<&[u8]> {
    let body = &bytes[..snapshot_boundary(bytes)];
    match body.strip_suffix(b"\n") {
        Some(joined) => joined.split(|byte| *byte == b'\n').collect(),
        None => Vec::new(),
    }
}

fn join(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line);
        bytes.push(b'\n');
    }
    bytes
}

fn expected(picks: &[Pick], base: &[u8], ours: &[u8], theirs: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for pick in picks {
        let source = match pick.from {
            Source::Base => base,
            Source::Ours => ours,
            Source::Theirs => theirs,
        };
        let lines = split(source);
        let indices: Vec<usize> = match (&pick.lines, &pick.indices) {
            (Some([from, to]), None) => (*from..*to).collect(),
            (None, Some(list)) => list.clone(),
            _ => panic!("a pick takes exactly one of `lines` / `pick`"),
        };
        for index in indices {
            out.extend_from_slice(lines[index]);
            out.push(b'\n');
        }
    }
    out
}

/// Line identity as the scenarios understand it, recomputed here so that a bug in the crate
/// cannot hide behind its own definition.
///
/// The carriage return is dropped for the same reason the crate drops it — a terminator git
/// converted is not part of the record — but the arithmetic is written out again here rather than
/// borrowed, which is the whole point of a second opinion.
fn keys(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut ordinals: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for line in split(bytes)
        .into_iter()
        .map(|line| match line.split_last() {
            Some((b'\r', head)) => head,
            _ => line,
        })
    {
        let uuid = line
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .filter(|byte| **byte == b'{')
            .and_then(|_| serde_json::from_slice::<Value>(line).ok())
            .and_then(|value| value.get("uuid").and_then(Value::as_str).map(str::to_owned))
            .filter(|uuid| !uuid.is_empty());
        let id = uuid.map_or_else(
            || format!("bytes:{}", String::from_utf8_lossy(line)),
            |uuid| format!("uuid:{uuid}"),
        );
        let ordinal = ordinals.entry(id.clone()).or_insert(0);
        *ordinal += 1;
        out.push((id, *ordinal));
    }
    out
}

fn sorted_keys(bytes: &[u8]) -> Vec<(String, usize)> {
    let mut keys = keys(bytes);
    keys.sort();
    keys
}

/// Whether two files hold at least one record in common: the precondition of the second-machine
/// law, and the same question the merge asks before treating a side as a different file.
fn shares_record(left: &[u8], right: &[u8]) -> bool {
    let left_uuids: Vec<(String, usize)> = keys(left)
        .into_iter()
        .filter(|(id, _)| id.starts_with("uuid:"))
        .collect();
    let right_uuids: Vec<(String, usize)> = keys(right)
        .into_iter()
        .filter(|(id, _)| id.starts_with("uuid:"))
        .collect();
    left_uuids.iter().any(|key| right_uuids.contains(key))
}

fn trimmed(bytes: &[u8]) -> Vec<u8> {
    bytes[..snapshot_boundary(bytes)].to_vec()
}

fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> MergeOutput {
    merge_jsonl(base, ours, theirs).expect("merge must not violate its own postcondition")
}

/// The laws every case must satisfy, whatever its inputs are.
fn check_laws(
    label: &str,
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    result: &[u8],
    failures: &mut Vec<String>,
) {
    let mut check = |what: &str, ok: bool| {
        if !ok {
            failures.push(format!("{label}: law `{what}` violated"));
        }
    };
    for (name, input) in [("base", base), ("ours", ours), ("theirs", theirs)] {
        let idempotent = merge(input, input, input);
        check(
            &format!("merge(x,x,x) = x for {name}"),
            idempotent.bytes == trimmed(input),
        );
        let ours_only = merge(b"", input, b"");
        let theirs_only = merge(b"", b"", input);
        check(
            &format!("merge(empty,x,empty) = x for {name}"),
            ours_only.bytes == trimmed(input),
        );
        check(
            &format!("merge(empty,empty,x) = x for {name}"),
            theirs_only.bytes == trimmed(input),
        );
    }
    let swapped = merge(base, theirs, ours);
    check(
        "keys are commutative",
        sorted_keys(&swapped.bytes) == sorted_keys(result),
    );
    // The other machine merges the same fork the other way round and then sees this result: it
    // must not gain or lose lines by doing so. The law holds only while the two files still
    // share a record: a side that shares none is treated as a different file (`absent`), the
    // base stops arbitrating and deletions are not honoured — found by fuzzing the reference
    // implementation, 181 violations out of 9000 triples, all of this one shape.
    for (name, input) in [("ours", ours), ("theirs", theirs)] {
        if !shares_record(input, result) {
            continue;
        }
        let again = merge(input, input, result);
        check(
            &format!("second machine keeps the result's keys via {name}"),
            sorted_keys(&again.bytes) == sorted_keys(result),
        );
    }
}

/// Invariants of a single merge, recomputed from the inputs rather than from the crate.
fn check_invariants(
    label: &str,
    ours: &[u8],
    theirs: &[u8],
    result: &[u8],
    failures: &mut Vec<String>,
) {
    let mut inputs: Vec<Vec<u8>> = split(ours).iter().map(|line| line.to_vec()).collect();
    inputs.extend(split(theirs).iter().map(|line| line.to_vec()));
    for line in split(result) {
        if !inputs.iter().any(|input| input == line) {
            failures.push(format!(
                "{label}: result holds a line that is in neither input"
            ));
            break;
        }
    }
    if !(result.is_empty() || result.ends_with(b"\n")) {
        failures.push(format!("{label}: result does not end with a newline"));
    }
}

#[test]
fn merge_scenarios() {
    let file: ScenarioFile = serde_json::from_str(SCENARIOS).expect("mergeScenarios.json");
    let mut failures: Vec<String> = Vec::new();
    let mut ids: Vec<&str> = Vec::new();
    for case in &file.cases {
        let label = format!("mergeScenarios.json#{} [{:?}]", case.id, case.provenance);
        if ids.contains(&case.id.as_str()) {
            failures.push(format!("{label}: duplicate id"));
        }
        ids.push(&case.id);
        if case.note.trim().is_empty() {
            failures.push(format!("{label}: empty note"));
        }
        let base = case
            .input
            .base
            .as_ref()
            .map(materialize)
            .unwrap_or_default();
        let ours = materialize(&case.input.ours);
        let theirs = materialize(&case.input.theirs);
        let output = merge(&base, &ours, &theirs);
        let want = expected(&case.expect.result, &base, &ours, &theirs);
        if output.bytes != want {
            failures.push(format!(
                "{label}: result differs — got {} lines, expected {} lines{}",
                split(&output.bytes).len(),
                split(&want).len(),
                first_difference(&output.bytes, &want),
            ));
        }
        let report = serde_json::to_value(&output.report).expect("report serializes");
        if report != case.expect.report {
            failures.push(format!(
                "{label}: report differs\n    got      {report}\n    expected {}",
                case.expect.report
            ));
        }
        check_invariants(&label, &ours, &theirs, &output.bytes, &mut failures);
        check_laws(&label, &base, &ours, &theirs, &output.bytes, &mut failures);
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

fn first_difference(got: &[u8], want: &[u8]) -> String {
    let got_lines = split(got);
    let want_lines = split(want);
    for index in 0..got_lines.len().max(want_lines.len()) {
        let left = got_lines.get(index).copied().unwrap_or_default();
        let right = want_lines.get(index).copied().unwrap_or_default();
        if left != right {
            return format!(
                "\n    first difference at line {index}:\n      got      {}\n      expected {}",
                preview(left),
                preview(right)
            );
        }
    }
    String::new()
}

/// A line is transcript content: only its shape is printed, never its text.
fn preview(line: &[u8]) -> String {
    if line.is_empty() {
        return "<missing>".to_owned();
    }
    let value: Option<Value> = serde_json::from_slice(line).ok();
    let kind = value
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("<opaque>");
    let uuid = value
        .as_ref()
        .and_then(|value| value.get("uuid"))
        .and_then(Value::as_str)
        .map_or_else(String::new, |uuid| {
            format!(" uuid={}", &uuid[..8.min(uuid.len())])
        });
    format!("type={kind}{uuid} bytes={}", line.len())
}
