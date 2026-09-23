//! What must never be in the fixtures.
//!
//! The fixtures are recordings of a real machine, so the rules in `fixtures/README.md` are the
//! only thing standing between a recording and a leak — and a rule nobody checks is a rule that
//! holds until the day it does not. Credentials, other people's paths and live session
//! identifiers are checked here, on every run.

// The gate reads the repository on purpose, so the purity rule that keeps the library free of I/O
// is lifted for this file alone.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::fs;
use std::path::{Path, PathBuf};

/// Substrings that must not appear in a fixture, with the reason each one is forbidden.
const FORBIDDEN: &[(&str, &str)] = &[
    ("-----BEGIN", "a key or certificate"),
    ("sk-ant-", "an Anthropic key"),
    ("ghp_", "a GitHub token"),
    ("Bearer ", "a bearer token"),
    ("Authorization:", "an authorization header"),
    ("@gmail.com", "an e-mail address"),
    ("OneDrive", "a path into the cloud folder"),
    ("ssh-rsa", "a public key"),
    ("BEGIN OPENSSH", "a private key"),
];

/// The ssh key type fixtures may carry, and only in its synthetic form.
const ED25519: &str = "ssh-ed25519 ";

/// The one session identifier the scrubber writes into transcript fixtures.
const SYNTHETIC_SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

/// Standard base64 without line breaks; `None` on anything else.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let data = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for &c in data {
        buffer = (buffer << 6) | u32::from(value(c)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// An ssh-ed25519 blob whose key bytes are all one value: the form every fixture key must have.
fn is_synthetic_ed25519(base64: &str) -> bool {
    let Some(blob) = base64_decode(base64) else {
        return false;
    };
    let read_len = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = blob.get(at..at + 4)?.try_into().ok()?;
        usize::try_from(u32::from_be_bytes(bytes)).ok()
    };
    let Some(type_len) = read_len(0) else {
        return false;
    };
    let key_at = 4 + type_len;
    let Some(key_len) = read_len(key_at) else {
        return false;
    };
    let Some(key) = blob.get(key_at + 4..key_at + 4 + key_len) else {
        return false;
    };
    blob.get(4..key_at) == Some(b"ssh-ed25519".as_slice())
        && key
            .first()
            .is_some_and(|first| key.iter().all(|byte| byte == first))
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries =
            fs::read_dir(&current).unwrap_or_else(|e| panic!("read {}: {e}", current.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn fixtures_carry_nothing_that_must_stay_on_the_machine() {
    let fixtures = repo_root().join("fixtures");
    let mut failures: Vec<String> = Vec::new();

    for file in files_under(&fixtures) {
        let name = file
            .file_name()
            .expect("file name")
            .to_string_lossy()
            .into_owned();
        let Ok(text) = fs::read_to_string(&file) else {
            continue; // binary fixture: nothing to read as text, and none exists today
        };
        for (needle, what) in FORBIDDEN {
            if text.contains(needle) {
                failures.push(format!("{name}: contains {what} ({needle})"));
            }
        }
        // An ed25519 public key is not a secret, but a real one names a real machine. Access
        // fixtures need well-formed keys, so they carry synthetic ones: every one of the key bytes
        // the same value, which no generated key ever is.
        for encoded in text.split(ED25519).skip(1) {
            let body: String = encoded
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
                .collect();
            if !is_synthetic_ed25519(&body) {
                failures.push(format!(
                    "{name}: ssh-ed25519 key {body:.24}… is not synthetic"
                ));
            }
        }
        // Transcript fixtures are scrubbed, and the scrubber gives every one of them the same
        // synthetic session: a real one would mean an unscrubbed line slipped through.
        if file
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            for (number, line) in text.lines().enumerate() {
                let Some(rest) = line.split("\"sessionId\":\"").nth(1) else {
                    continue;
                };
                let session = rest.split('"').next().unwrap_or_default();
                if session != SYNTHETIC_SESSION_ID {
                    failures.push(format!(
                        "{name}:{}: session id {session} is not the synthetic one",
                        number + 1
                    ));
                }
            }
        }
    }

    assert!(
        is_synthetic_ed25519(
            "AAAAC3NzaC1lZDI1NTE5AAAAIBERERERERERERERERERERERERERERERERERERERERER"
        ),
        "the synthetic-key rule does not accept its own sample"
    );
    assert!(
        !is_synthetic_ed25519(
            "AAAAC3NzaC1lZDI1NTE5AAAAIBERERERERERERERERERERERERERERERERERERERERES"
        ),
        "the synthetic-key rule accepts a key whose last byte differs"
    );

    assert!(
        !failures.is_empty() || !files_under(&fixtures).is_empty(),
        "no fixtures were checked"
    );
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}
