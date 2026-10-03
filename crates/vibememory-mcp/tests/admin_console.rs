//! The host console of a host without the cabinet: every change of the snapshot, made on the base
//! snapshot of `fixtures/access/accessSnapshots.json` and checked as the host checks it — and the
//! grants it hands out, read back by the engine's own claim reader.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value
)]

use serde_json::Value;
use vibememory_core::claim::{Claim, read_answer};
use vibememory_mcp::access::{self, Admission};
use vibememory_mcp::admin::{HostFacts, Operation, Outcome, Role, apply, empty, render};

const SNAPSHOTS: &str = include_str!("../../../fixtures/access/accessSnapshots.json");
const TODAY: &str = "2026-10-03";
const NOW: &str = "2026-10-03T12:00:00Z";
const HOST_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMz";
const MACHINE_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIERERERERERERERERERERERERERERERERERERERERERE";

fn base() -> Value {
    let file: Value = serde_json::from_str(SNAPSHOTS).unwrap();
    file["base"].clone()
}

fn facts() -> HostFacts {
    HostFacts {
        domain: "memory.example.com".to_owned(),
        ssh_user: "vmgit".to_owned(),
        host_keys: vec![HOST_KEY.to_owned()],
    }
}

/// Bytes that differ from call to call, the same in every run.
fn counter() -> impl FnMut(&mut [u8]) {
    let mut next = 7u8;
    move |buffer: &mut [u8]| {
        for byte in buffer.iter_mut() {
            next = next.wrapping_mul(31).wrapping_add(11);
            *byte = next;
        }
    }
}

fn run(snapshot: &Value, operation: Operation) -> Result<(Value, Outcome), String> {
    apply(snapshot, &operation, &facts(), TODAY, &mut counter())
}

fn checked(snapshot: &Value) -> access::Snapshot {
    access::check(render(snapshot).as_bytes()).unwrap()
}

#[test]
fn every_change_raises_the_serial_and_passes_the_hosts_check() {
    let (changed, _) = run(
        &base(),
        Operation::TeamAdd {
            slug: "acme".to_owned(),
            owner: "alice".to_owned(),
            sessions: false,
            quota_bytes: 500 * 1_048_576,
        },
    )
    .unwrap();
    assert_eq!(
        changed["serial"], 13,
        "the host never applies a snapshot that is not newer"
    );
    assert_eq!(changed["teamCount"], 5);
    let snapshot = checked(&changed);
    assert_eq!(snapshot.teams["acme"].members["alice"], access::Rank::Owner);
    assert_eq!(snapshot.teams["acme"].projects, Some(vec![]));
}

#[test]
fn a_hand_written_snapshot_becomes_the_consoles_version() {
    let mut hand = empty();
    hand["version"] = 1.into();
    hand.as_object_mut().unwrap().remove("serial");
    hand.as_object_mut().unwrap().remove("bans");
    let (changed, _) = run(
        &hand,
        Operation::TeamAdd {
            slug: "acme".to_owned(),
            owner: "alice".to_owned(),
            sessions: true,
            quota_bytes: 1_048_576,
        },
    )
    .unwrap();
    assert_eq!(
        (changed["version"].clone(), changed["serial"].clone()),
        (2.into(), 1.into())
    );
    assert!(
        checked(&changed).teams["acme"].projects.is_none(),
        "a team with sessions keeps no list"
    );
}

#[test]
fn a_token_grant_is_taken_by_connect_and_the_token_opens_the_server() {
    let (changed, outcome) = run(
        &base(),
        Operation::TokenIssue {
            team: "vibebrains".to_owned(),
            member: "alice".to_owned(),
            agent: "codex".to_owned(),
            reader: false,
            history: false,
            projects: None,
            expires_at: None,
        },
    )
    .unwrap();
    let Outcome::Grant { grant, .. } = outcome else {
        panic!("a token is handed over as a grant");
    };
    let Ok(Claim::Token(token)) = read_answer(0, 200, &grant, &facts().address()) else {
        panic!("connect --grant takes what the console issues: {grant}");
    };
    assert_eq!(token.mcp_url, "https://memory.example.com/mcp");
    assert!(
        !render(&changed).contains(&token.token),
        "the snapshot keeps the digest, never the token"
    );
    let snapshot = checked(&changed);
    assert!(
        matches!(snapshot.admit(&token.token, NOW), Admission::Granted(found) if found.agent == "codex")
    );
}

#[test]
fn a_key_grant_is_taken_by_connect_and_lands_in_the_snapshot() {
    let (changed, outcome) = run(
        &base(),
        Operation::KeyAdd {
            member: "alice".to_owned(),
            machine: "desktop".to_owned(),
            teams: vec!["syncteam".to_owned()],
            public_key: MACHINE_KEY.to_owned(),
        },
    )
    .unwrap();
    let Outcome::Grant { grant, .. } = outcome else {
        panic!("a key is handed over as a grant");
    };
    let Ok(Claim::Key(key)) = read_answer(0, 200, &grant, &facts().address()) else {
        panic!("connect --grant takes what the console issues: {grant}");
    };
    assert_eq!(
        (key.mode.as_str(), key.remote.as_str()),
        ("sync", "teams/syncteam.git")
    );
    assert_eq!(key.host_keys, vec![HOST_KEY.to_owned()]);
    assert!(checked(&changed).key(&key.key_id).is_some());
}

#[test]
fn a_machine_key_needs_a_team_with_sessions() {
    let refused = run(
        &base(),
        Operation::KeyAdd {
            member: "alice".to_owned(),
            machine: "desktop".to_owned(),
            teams: vec!["vibebrains".to_owned()],
            public_key: MACHINE_KEY.to_owned(),
        },
    );
    assert!(
        refused
            .unwrap_err()
            .contains("sessions of vibebrains are off")
    );
}

#[test]
fn a_member_who_leaves_takes_their_tokens_and_keys_of_the_team() {
    let (with_key, _) = run(
        &base(),
        Operation::KeyAdd {
            member: "alice".to_owned(),
            machine: "desktop".to_owned(),
            teams: vec!["syncteam".to_owned()],
            public_key: MACHINE_KEY.to_owned(),
        },
    )
    .unwrap();
    let (changed, _) = run(
        &with_key,
        Operation::MemberRemove {
            team: "syncteam".to_owned(),
            handle: "alice".to_owned(),
        },
    )
    .unwrap();
    let snapshot = checked(&changed);
    assert!(!snapshot.teams["syncteam"].members.contains_key("alice"));
    assert!(
        snapshot
            .tokens
            .iter()
            .all(|token| !(token.team == "syncteam" && token.member == "alice"))
    );
    assert!(
        snapshot
            .keys
            .iter()
            .all(|key| !key.teams.contains(&"syncteam".to_owned()))
    );
}

#[test]
fn an_owner_neither_leaves_nor_changes_rank() {
    for operation in [
        Operation::MemberRemove {
            team: "vibebrains".to_owned(),
            handle: "borodatych".to_owned(),
        },
        Operation::MemberRole {
            team: "vibebrains".to_owned(),
            handle: "borodatych".to_owned(),
            role: Role::Admin,
        },
    ] {
        assert!(
            run(&base(), operation)
                .unwrap_err()
                .contains("owns vibebrains")
        );
    }
}

#[test]
fn an_admin_made_member_loses_the_search_of_past_sessions() {
    let (changed, _) = run(
        &base(),
        Operation::MemberRole {
            team: "syncteam".to_owned(),
            handle: "alice".to_owned(),
            role: Role::Member,
        },
    )
    .unwrap();
    let snapshot = checked(&changed);
    assert!(
        snapshot
            .tokens
            .iter()
            .filter(|token| token.team == "syncteam" && token.member == "alice")
            .all(|token| !token.history),
        "the host refuses history to a member; the console takes it away with the rank"
    );
}

#[test]
fn what_the_host_would_refuse_is_never_written() {
    let refused = run(
        &base(),
        Operation::TokenIssue {
            team: "vibebrains".to_owned(),
            member: "alice".to_owned(),
            agent: "codex".to_owned(),
            reader: false,
            history: true,
            projects: None,
            expires_at: None,
        },
    );
    assert!(refused.unwrap_err().starts_with("historyNotAllowed"));
}

#[test]
fn a_deleted_team_goes_with_its_tokens_and_keeps_its_name() {
    let (changed, _) = run(
        &base(),
        Operation::TeamRemove {
            slug: "syncteam".to_owned(),
        },
    )
    .unwrap();
    let snapshot = checked(&changed);
    assert_eq!(snapshot.teams["syncteam"].deleted.as_deref(), Some(TODAY));
    assert!(snapshot.tokens.iter().all(|token| token.team != "syncteam"));
    let again = run(
        &changed,
        Operation::TeamAdd {
            slug: "syncteam".to_owned(),
            owner: "alice".to_owned(),
            sessions: false,
            quota_bytes: 1_048_576,
        },
    );
    assert!(again.unwrap_err().contains("keep their name for good"));
}

#[test]
fn projects_are_listed_only_where_sessions_are_off() {
    let (changed, _) = run(
        &base(),
        Operation::ProjectAdd {
            team: "vibebrains".to_owned(),
            name: "Promed".to_owned(),
        },
    )
    .unwrap();
    assert!(
        checked(&changed).teams["vibebrains"]
            .projects
            .as_ref()
            .unwrap()
            .contains(&"Promed".to_owned())
    );
    let refused = run(
        &base(),
        Operation::ProjectAdd {
            team: "syncteam".to_owned(),
            name: "Promed".to_owned(),
        },
    );
    assert!(
        refused
            .unwrap_err()
            .contains("keeps its projects in its store")
    );
}

#[test]
fn a_revoked_token_no_longer_opens_anything() {
    let (changed, _) = run(
        &base(),
        Operation::TokenRevoke {
            id: "tk_7q2m9x4a".to_owned(),
        },
    )
    .unwrap();
    assert!(
        checked(&changed)
            .tokens
            .iter()
            .all(|token| token.id != "tk_7q2m9x4a")
    );
    assert!(
        run(
            &changed,
            Operation::TokenRevoke {
                id: "tk_7q2m9x4a".to_owned()
            }
        )
        .is_err()
    );
}
