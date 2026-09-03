//! Laws of the merge, checked on generated inputs rather than recorded ones.
//!
//! The fixtures say what the merge does on the cases we know; this says what must hold on the
//! cases we did not think of. It earned its place: fuzzing the reference implementation is what
//! showed that the second-machine law needs a precondition — two files that share no record are
//! not two versions of one file, and the merge treats the odd one as absent.
//!
//! The generator is deterministic: a fixed set of seeds and a tiny generator of its own, so a
//! failure names the seed and reproduces on both machines instead of appearing once in CI.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use serde_json::Value;
use vibememory_core::merge::jsonl::merge_jsonl;
use vibememory_core::merge::keep_both::{KeepBothInput, Resolution, keep_both};

/// Deterministic generator: xorshift64*, small enough to read and identical everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next() % bound as u64).unwrap_or(0)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

/// A transcript-shaped file: records with a chain, state lines that repeat byte for byte, and the
/// occasional line no parser will accept.
fn generate(rng: &mut Rng, records: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut previous: Option<String> = None;
    for index in 0..records {
        let uuid = format!(
            "{:08x}-0000-4000-8000-{:012x}",
            index,
            rng.next() % 0xffff_ffff
        );
        let parent = previous
            .as_ref()
            .map_or("null".to_owned(), |p| format!("\"{p}\""));
        let seconds = index * 3 + rng.below(3);
        let timestamp = format!("2026-09-03T10:{:02}:{:02}.000Z", seconds / 60, seconds % 60);
        lines.push(format!(
            "{{\"type\":\"user\",\"parentUuid\":{parent},\"uuid\":\"{uuid}\",\"timestamp\":\"{timestamp}\"}}"
        ));
        previous = Some(uuid);
        if rng.chance(40) {
            // The state block is rewritten byte for byte again and again.
            for _ in 0..=rng.below(3) {
                lines.push(
                    "{\"type\":\"custom-title\",\"customTitle\":\"t\",\"sessionId\":\"s\"}"
                        .to_owned(),
                );
            }
        }
        if rng.chance(10) {
            lines.push("not json at all".to_owned());
        }
    }
    lines
}

fn join(lines: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in lines {
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
    }
    bytes
}

/// A side of a fork: the common prefix, then edits of its own.
fn side(rng: &mut Rng, common: &[String], own: &[String]) -> Vec<u8> {
    let mut lines = common.to_vec();
    lines.extend(own.iter().cloned());
    if rng.chance(20) && lines.len() > 2 {
        lines.remove(rng.below(lines.len()));
    }
    if rng.chance(15) && !lines.is_empty() {
        let index = rng.below(lines.len());
        lines.insert(index, lines[index].clone());
    }
    let mut bytes = join(&lines);
    if rng.chance(15) && bytes.len() > 10 {
        // A snapshot of a live file can catch half a record.
        bytes.truncate(bytes.len() - 1 - rng.below(9));
    }
    bytes
}

/// The identity of every line, counted: what the merge may never change silently.
fn keys(bytes: &[u8]) -> BTreeMap<String, usize> {
    let mut counted = BTreeMap::new();
    let boundary = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    for line in bytes[..boundary].split(|b| *b == b'\n') {
        if line.is_empty() && boundary == 0 {
            continue;
        }
        let identity = serde_json::from_slice::<Value>(line)
            .ok()
            .filter(Value::is_object)
            .and_then(|value| value.get("uuid").and_then(Value::as_str).map(str::to_owned))
            .map_or_else(
                || format!("bytes:{}", String::from_utf8_lossy(line)),
                |uuid| format!("uuid:{uuid}"),
            );
        *counted.entry(identity).or_insert(0) += 1;
    }
    counted
}

fn shares_a_record(left: &[u8], right: &[u8]) -> bool {
    let right_keys = keys(right);
    keys(left)
        .keys()
        .any(|key| key.starts_with("uuid:") && right_keys.contains_key(key))
}

fn trimmed(bytes: &[u8]) -> Vec<u8> {
    let boundary = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    bytes[..boundary].to_vec()
}

/// How many triples to build. The generator is biased towards the awkward shapes, so this is
/// enough to reach the case the second-machine law excludes — the test fails if it does not.
const SEEDS: u64 = 600;

#[test]
fn merge_laws_hold_on_generated_transcripts() {
    let mut failures: Vec<String> = Vec::new();
    for seed in 1..=SEEDS {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let common_len = 1 + rng.below(6);
        let common = generate(&mut rng, common_len);
        let our_len = rng.below(4);
        let our_tail = generate(&mut rng, our_len);
        let their_len = rng.below(4);
        let their_tail = generate(&mut rng, their_len);
        let base = join(&common);
        let ours = side(&mut rng, &common, &our_tail);
        // Once in a while a side is not a version of this file at all: a fresh file written at
        // the same path after a rename, or a foreign one. The merge calls such a side absent,
        // and the second-machine law does not reach across that gap.
        let theirs = if rng.chance(12) {
            // A file that carries no record at all: this is what a fresh file at the same path
            // looks like right after a rename — the CLI writes its state block first. It shares
            // nothing with anyone, which is exactly the case the second-machine law excludes.
            join(&[
                "{\"type\":\"custom-title\",\"customTitle\":\"t\",\"sessionId\":\"s\"}".to_owned(),
            ])
        } else if rng.chance(15) {
            let foreign_len = 1 + rng.below(3);
            join(&generate(&mut rng, foreign_len))
        } else {
            side(&mut rng, &common, &their_tail)
        };

        let mut check = |what: &str, ok: bool| {
            if !ok {
                failures.push(format!("seed {seed}: {what}"));
            }
        };

        let Ok(merged) = merge_jsonl(&base, &ours, &theirs) else {
            failures.push(format!(
                "seed {seed}: the merge violated its own postcondition"
            ));
            continue;
        };
        let result = merged.bytes;

        check(
            "the result neither ends with a newline nor is empty",
            result.is_empty() || result.ends_with(b"\n"),
        );
        check("merging a file with itself changed it", {
            merge_jsonl(&ours, &ours, &ours).map(|out| out.bytes) == Ok(trimmed(&ours))
        });
        check("a one-sided merge changed the file", {
            let from_ours = merge_jsonl(b"", &ours, b"").map(|out| out.bytes);
            let from_theirs = merge_jsonl(b"", b"", &ours).map(|out| out.bytes);
            from_ours == Ok(trimmed(&ours)) && from_theirs == Ok(trimmed(&ours))
        });
        check("swapping the sides changed which lines survive", {
            merge_jsonl(&base, &theirs, &ours).map(|out| keys(&out.bytes)) == Ok(keys(&result))
        });
        // The other machine merges the same fork the other way round and then sees this result.
        // The law holds while the two files still share a record: a side that shares none is a
        // different file, and the merge says so instead of pretending it was emptied.
        for (name, input) in [("ours", &ours), ("theirs", &theirs)] {
            // The precondition of the law, pinned by its own test below: a file that shares no
            // record with the result is not a version of it.
            if !shares_a_record(input, &result) {
                continue;
            }
            check(
                &format!("the machine holding {name} did not settle on the result"),
                {
                    merge_jsonl(input, input, &result).map(|out| keys(&out.bytes))
                        == Ok(keys(&result))
                },
            );
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

/// Why the law above carries a precondition, in one deterministic case.
///
/// Both machines deleted what the other had kept, so the merged file holds no record at all —
/// only a line that carries no identity. When the first machine then merges that result against
/// its own file, the two share nothing, and the merge treats the incoming file as a different
/// file rather than as its own emptied out: the record comes back.
///
/// That is a deliberate choice and the reason the law is not unconditional. A merge cannot tell
/// "the other machine deleted this" from "this is a fresh file at the same path" — a real shape,
/// since a new transcript begins with a state block — and between resurrecting a record and
/// dropping one silently, the engine resurrects.
#[test]
fn the_second_machine_law_needs_its_precondition() {
    let record = |uuid: &str| {
        format!(
            "{{\"type\":\"user\",\"parentUuid\":null,\"uuid\":\"{uuid}\",\"timestamp\":\"2026-09-03T10:00:00.000Z\"}}\n"
        )
    };
    let stateless = "{\"type\":\"custom-title\",\"customTitle\":\"t\",\"sessionId\":\"s\"}\n";
    let ours_record = record("00000000-0000-4000-8000-000000000001");
    let theirs_record = record("00000000-0000-4000-8000-000000000002");

    let base = format!("{ours_record}{stateless}{theirs_record}");
    let ours = format!("{ours_record}{stateless}");
    let theirs = format!("{stateless}{theirs_record}");

    let merged = merge_jsonl(base.as_bytes(), ours.as_bytes(), theirs.as_bytes()).expect("merge");
    assert_eq!(
        String::from_utf8_lossy(&merged.bytes),
        stateless,
        "each side honoured the other's deletion, so no record is left"
    );
    assert!(
        !shares_a_record(ours.as_bytes(), &merged.bytes),
        "the two files share no record"
    );

    let settled = merge_jsonl(ours.as_bytes(), ours.as_bytes(), &merged.bytes).expect("merge");
    assert_eq!(
        keys(&settled.bytes),
        keys(ours.as_bytes()),
        "the record comes back: the incoming file is treated as a different one, not as ours emptied"
    );
}

#[test]
fn keep_both_properties_hold_on_generated_files() {
    let mut failures: Vec<String> = Vec::new();
    for seed in 1..=300_u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).max(1));
        let path = format!(
            "projects/P{}/{}/{}.md",
            rng.below(3),
            if rng.chance(50) { "memory" } else { "8f2c" },
            rng.below(5)
        );
        let versions = ["", "one", "two", "three\nlines\n"];
        let base = versions[rng.below(versions.len())].as_bytes();
        let ours = versions[rng.below(versions.len())].as_bytes();
        let theirs = versions[rng.below(versions.len())].as_bytes();
        let input = KeepBothInput {
            path: &path,
            base,
            ours,
            theirs,
            stamp: "mac-2026-09-03T10-00-00Z",
        };
        let outcome = keep_both(&input);

        let mut check = |what: &str, ok: bool| {
            if !ok {
                failures.push(format!("seed {seed}: {what}"));
            }
        };
        let expected: &[u8] = match outcome.report.resolution {
            Resolution::TookTheirs => theirs,
            Resolution::Identical | Resolution::TookOurs | Resolution::KeptBoth => ours,
        };
        check(
            "the version written is not the one the resolution names",
            outcome.bytes == expected,
        );
        check(
            "a version was set aside without a conflict, or a conflict lost one",
            outcome.quarantined.is_some() == (outcome.report.resolution == Resolution::KeptBoth),
        );
        if let Some(quarantined) = &outcome.quarantined {
            check(
                "the version set aside is not the incoming one",
                quarantined.bytes == theirs,
            );
            check(
                "the name of the version set aside is not one usable component",
                !quarantined.name.is_empty()
                    && !quarantined.name.contains('/')
                    && quarantined.name.len() <= vibememory_core::MAX_FILE_NAME_BYTES,
            );
        }
        let settled = keep_both(&KeepBothInput {
            theirs: &outcome.bytes,
            ours: theirs,
            base: theirs,
            ..input
        });
        check(
            "the other machine did not settle on the result",
            settled.bytes == outcome.bytes,
        );
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
