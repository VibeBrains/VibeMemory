//! Who a presented token is: by id and digest for the cabinet's tokens, as a whole for the one
//! legacy token, and never by trying every digest in the snapshot.

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
    let text = json!({
        "version": 1, "teamCount": 1,
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
