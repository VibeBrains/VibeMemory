//! Who a presented token is: by id and digest for the cabinet's tokens, as a whole for the one
//! legacy token, and never by trying every digest in the snapshot; nobody while their ban lasts.
//! Which of two snapshots is in force: never one older than what the host applied.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing
)]

use serde_json::json;
use vibememory_mcp::access::{self, Admission, Snapshot};

const LEGACY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ISSUED: &str = "vmt_7q2m9x4a_the-secret-part";
const EXPIRES: &str = "vmt_b3c4d5e6_another-secret";
const NOW: &str = "2026-09-23T12:00:00Z";

fn digest(token: &str) -> String {
    vibememory_cli::sha256::hex(token.as_bytes())
}

fn snapshot() -> Snapshot {
    with(&json!({}))
}

/// The snapshot of these tests in the cabinet's format, its top-level fields replaced by `fields`.
fn with(fields: &serde_json::Value) -> Snapshot {
    let mut text = json!({
        "version": 2, "serial": 5, "bans": [], "teamCount": 1,
        "teams": { "personal": {
            "adopted": true, "repo": "/home/vm/vibememory/store.git", "writable": true,
            "limits": { "maxRecords": 5000, "maxRecordBytes": 65536 },
            "members": { "borodatych": "owner" }
        }},
        "tokens": [
            { "id": "tk_legacy", "legacy": true, "sha256": digest(LEGACY), "team": "personal",
              "member": "borodatych", "agent": "claude-code", "role": "writer", "history": true,
              "projects": null, "expiresAt": null },
            { "id": "tk_7q2m9x4a", "sha256": digest(ISSUED), "team": "personal",
              "member": "borodatych", "agent": "vibeide", "role": "writer", "history": true,
              "projects": null, "expiresAt": null },
            { "id": "tk_b3c4d5e6", "sha256": digest(EXPIRES), "team": "personal",
              "member": "borodatych", "agent": "vibeidea", "role": "reader", "history": false,
              "projects": null, "expiresAt": NOW }
        ],
        "keys": []
    });
    for (name, value) in fields.as_object().expect("fields") {
        text[name] = value.clone();
    }
    access::check(text.to_string().as_bytes()).expect("a valid snapshot")
}

fn id_of(admission: Admission<'_>) -> Option<String> {
    match admission {
        Admission::Granted(token) => Some(token.id.clone()),
        Admission::Expired(token) => Some(format!("expired {}", token.id)),
        Admission::Unknown => None,
    }
}

#[test]
fn a_token_is_found_by_its_id_and_proved_by_its_digest() {
    let snapshot = snapshot();
    assert_eq!(
        id_of(snapshot.admit(ISSUED, NOW)).as_deref(),
        Some("tk_7q2m9x4a")
    );
    assert_eq!(
        id_of(snapshot.admit(LEGACY, NOW)).as_deref(),
        Some("tk_legacy")
    );

    // The right id with another secret, the legacy value dressed as an issued token, an id that
    // is no id, and nothing at all: all unknown.
    for presented in [
        "vmt_7q2m9x4a_the-secret-parT",
        "vmt_legacy_0123456789abcdef",
        &format!("vmt_7q2m9x4a_{LEGACY}"),
        "vmt_ILOU1234_the-secret-part",
        "vmt_7q2m9x4a_",
        "vmt_7q2m9x4a",
        "",
        "0123456789abcdef",
    ] {
        assert_eq!(id_of(snapshot.admit(presented, NOW)), None, "{presented:?}");
    }
}

#[test]
fn a_token_stops_working_at_the_moment_it_expires() {
    let snapshot = snapshot();
    assert_eq!(
        id_of(snapshot.admit(EXPIRES, "2026-09-23T11:59:59Z")).as_deref(),
        Some("tk_b3c4d5e6")
    );
    assert_eq!(
        id_of(snapshot.admit(EXPIRES, NOW)).as_deref(),
        Some("expired tk_b3c4d5e6")
    );
}

#[test]
fn a_banned_member_is_nobody_until_the_ban_ends() {
    let for_good = with(&json!({"bans": [{"member": "borodatych", "until": null}]}));
    assert_eq!(id_of(for_good.admit(ISSUED, NOW)), None);
    assert_eq!(
        id_of(for_good.admit(LEGACY, NOW)),
        None,
        "the legacy token too"
    );

    let until_noon = with(&json!({"bans": [{"member": "borodatych", "until": NOW}]}));
    assert_eq!(
        id_of(until_noon.admit(ISSUED, "2026-09-23T11:59:59Z")),
        None
    );
    assert_eq!(
        id_of(until_noon.admit(ISSUED, NOW)).as_deref(),
        Some("tk_7q2m9x4a"),
        "the ban is over at its moment"
    );

    let someone_else = with(&json!({"bans": [{"member": "alice", "until": null}]}));
    assert_eq!(
        id_of(someone_else.admit(ISSUED, NOW)).as_deref(),
        Some("tk_7q2m9x4a")
    );
}

#[test]
fn a_snapshot_older_than_the_one_applied_is_not_in_force() {
    let applied = with(&json!({"serial": 7}));
    let older = with(&json!({"serial": 6}));
    let newer = with(&json!({"serial": 8, "tokens": []}));
    let rival = with(&json!({"serial": 7, "tokens": []}));

    assert_eq!(access::in_force(&older, Some(&applied)), &applied);
    assert_eq!(access::in_force(&newer, Some(&applied)), &newer);
    assert_eq!(
        access::in_force(&applied.clone(), Some(&applied)),
        &applied,
        "the same one again"
    );
    assert_eq!(
        access::in_force(&rival, Some(&applied)),
        &applied,
        "two decisions under one serial: the host keeps the one it took"
    );
    assert_eq!(
        access::in_force(&older, None),
        &older,
        "nothing applied yet"
    );

    // Hand-written snapshots have no serial and follow the file; once the cabinet's format was
    // applied, a hand-written file is older than it.
    let earlier = with(&json!({"version": 1, "serial": null, "bans": null}));
    let edited = with(&json!({"version": 1, "serial": null, "bans": null, "tokens": []}));
    assert_eq!(access::in_force(&edited, Some(&earlier)), &edited);
    assert_eq!(access::in_force(&edited, Some(&applied)), &applied);
}
