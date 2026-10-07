//! The machine key branch of `connect`: the key made for every claim and dropped when the code was
//! not for a machine, the refusals that set nothing up, and a machine connecting again — its clone
//! pointed at the new key and the team's own `known_hosts`, the user's ssh configuration untouched.
//! The keys of real ssh-keygen on both sides: the machine's goes out in the form the host takes, the
//! host's come back in the form the engine takes. A first clone over ssh needs the host and is
//! checked by the phase's live gate.

// The test writes files, runs git and ssh-keygen and reads modes, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fs;
use std::process::Command;

use support::TempDir;
use vibememory_cli::install::Layout;
use vibememory_cli::team_connect::{
    KEY_FILE, KNOWN_HOSTS_FILE, KeyRefusal, PendingKey, RECORD_FILE, keep_key, read_record,
    ssh_command,
};
use vibememory_core::claim::{Claim, KeyGrant, read_answer};

const ANSWERS: &str = include_str!("../../../fixtures/claim/claimAnswers.json");

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    }
}

/// The grant of the fixture's `key` case: team `syncteam`, sessions on.
fn grant() -> KeyGrant {
    let file: serde_json::Value = serde_json::from_str(ANSWERS).unwrap();
    let case = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "key")
        .unwrap();
    let body = case["body"]
        .as_str()
        .map_or_else(|| case["body"].to_string(), str::to_owned);
    let Ok(Claim::Key(grant)) = read_answer(0, 200, &body, file["asked"].as_str().unwrap()) else {
        panic!("the fixture's key case must read")
    };
    grant
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn a_key_is_made_for_the_claim_and_dropped_when_unused() {
    let temp = TempDir::new("connect-key-pending");
    let layout = layout(&temp);
    let pending = PendingKey::make(&layout).unwrap();
    // the key a real ssh-keygen made passes the very rule the host checks the snapshot with
    assert!(
        vibememory_core::ssh_key::is_ed25519_line(&pending.public),
        "{}",
        pending.public
    );
    pending.discard();
    // nothing of it stays: the stores directory holds no pending key and no team
    let left: Vec<_> = fs::read_dir(layout.engine_dir.join("stores"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn refusals_set_nothing_up() {
    let temp = TempDir::new("connect-key-refusals");
    let layout = layout(&temp);
    let mut off = grant();
    off.mode = "memory".to_owned();
    let mut hostless = grant();
    hostless.host_keys.clear();
    for (grant, installed, want) in [
        (off, true, KeyRefusal::SessionsOff),
        (grant(), false, KeyRefusal::EngineMissing),
        (hostless, true, KeyRefusal::NoHostKeys),
    ] {
        let pending = PendingKey::make(&layout).unwrap();
        assert_eq!(keep_key(&layout, &grant, pending, installed), Err(want));
        assert!(!layout.team_state_dir(&grant.team).exists());
    }
}

#[test]
fn a_machine_connecting_again_keeps_its_clone_on_the_new_key() {
    let temp = TempDir::new("connect-key-again");
    let layout = layout(&temp);
    let grant = grant();
    let clone = layout.team_store(&grant.team);
    fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "--quiet"]);
    git(
        &clone,
        &["remote", "add", "origin", "old@host:teams/syncteam.git"],
    );
    let home_ssh = temp.dir("home/.ssh");
    fs::write(home_ssh.join("config"), "Host *\n").unwrap();

    let pending = PendingKey::make(&layout).unwrap();
    let public = pending.public.clone();
    let store = keep_key(&layout, &grant, pending, true).unwrap();

    assert!(!store.cloned);
    assert_eq!(store.clone, clone);
    let state = layout.team_state_dir(&grant.team);
    // the file keeps ssh-keygen's comment; the cabinet got the same key without it
    let kept = fs::read_to_string(state.join(format!("{KEY_FILE}.pub"))).unwrap();
    assert_eq!(
        vibememory_core::ssh_key::wire_line(&kept).as_deref(),
        Some(public.as_str())
    );
    let hosts = fs::read_to_string(state.join(KNOWN_HOSTS_FILE)).unwrap();
    assert_eq!(
        hosts,
        format!("{} {}\n", grant.ssh_host, grant.host_keys[0])
    );
    let record = read_record(&layout, &grant.team).unwrap();
    assert_eq!(record.store_name, grant.store_name);
    assert!(state.join(RECORD_FILE).exists());
    assert_eq!(
        git(&clone, &["remote", "get-url", "origin"]),
        "vmgit@vibememory.ru:teams/syncteam.git"
    );
    assert_eq!(
        git(&clone, &["config", "core.sshCommand"]),
        ssh_command(&state)
    );
    assert!(ssh_command(&state).starts_with("ssh -F none -i "));
    // the person's own ssh is not read and not written
    assert_eq!(
        fs::read_to_string(home_ssh.join("config")).unwrap(),
        "Host *\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for name in [KEY_FILE, KNOWN_HOSTS_FILE, RECORD_FILE] {
            let mode = fs::metadata(state.join(name)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
        let mode = fs::metadata(&state).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}

#[test]
fn doctor_names_what_is_wrong_with_a_team_store() {
    let temp = TempDir::new("connect-key-doctor");
    let layout = layout(&temp);
    let grant = grant();
    let clone = layout.team_store(&grant.team);
    fs::create_dir_all(&clone).unwrap();
    git(&clone, &["init", "--quiet"]);
    git(
        &clone,
        &["remote", "add", "origin", "old@host:teams/syncteam.git"],
    );
    let pending = PendingKey::make(&layout).unwrap();
    keep_key(&layout, &grant, pending, true).unwrap();

    let facts = vibememory_cli::team_connect::team_facts(&layout);
    assert_eq!(facts.len(), 1);
    assert!(facts[0].problems.is_empty(), "{:?}", facts[0].problems);
    assert_eq!(facts[0].pause, None);

    let state = layout.team_state_dir(&grant.team);
    fs::remove_file(state.join(KNOWN_HOSTS_FILE)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(state.join(KEY_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    }
    vibememory_cli::guard::TickState {
        pause: Some(vibememory_cli::guard::StorePause {
            code: "readOnly".to_owned(),
            lines: Vec::new(),
            since: "2026-09-05T10:00:00Z".to_owned(),
            recheck_at: "2026-09-05T11:00:00Z".to_owned(),
        }),
        ..vibememory_cli::guard::TickState::default()
    }
    .write(&state)
    .unwrap();

    let facts = vibememory_cli::team_connect::team_facts(&layout);
    let problems = facts[0].problems.join("\n");
    assert!(
        problems.contains("connect --refresh syncteam"),
        "{problems}"
    );
    #[cfg(unix)]
    assert!(problems.contains("readable by others"), "{problems}");
    assert_eq!(
        facts[0].pause.as_ref().map(|pause| pause.code.as_str()),
        Some("readOnly")
    );
}

#[test]
fn the_host_keys_a_real_host_announces_are_taken_by_the_engine() {
    let temp = TempDir::new("connect-host-keys");
    let dir = temp.dir("etc/ssh");
    // an sshd host carries a key of each type, each with its comment
    let files: Vec<String> = ["ed25519", "ecdsa", "rsa"]
        .iter()
        .map(|kind| {
            let key = dir.join(format!("ssh_host_{kind}_key"));
            let status = Command::new("ssh-keygen")
                .args(["-q", "-t", kind, "-N", "", "-C", "root@host", "-f"])
                .arg(&key)
                .status()
                .unwrap();
            assert!(status.success(), "ssh-keygen -t {kind}");
            fs::read_to_string(key.with_extension("pub")).unwrap()
        })
        .collect();
    let lines = vibememory_core::ssh_key::host_key_lines(files.iter().map(String::as_str));
    assert_eq!(lines.len(), 1, "{lines:?}");

    // the cabinet passes them on in the claim answer, and the engine reads that answer
    let file: serde_json::Value = serde_json::from_str(ANSWERS).unwrap();
    let case = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "key")
        .unwrap();
    let mut body: serde_json::Value = case["body"].as_str().map_or_else(
        || case["body"].clone(),
        |text| serde_json::from_str(text).unwrap(),
    );
    body["hostKeys"] = serde_json::json!(lines);
    let answer = read_answer(0, 200, &body.to_string(), file["asked"].as_str().unwrap());
    assert!(matches!(answer, Ok(Claim::Key(_))), "{answer:?}");
}

/// The fixture's answer for a machine of the owner's personal store.
fn personal_grant() -> KeyGrant {
    let file: serde_json::Value = serde_json::from_str(ANSWERS).unwrap();
    let case = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "personalKey")
        .unwrap();
    let Ok(Claim::Key(grant)) = read_answer(
        0,
        200,
        &case["body"].to_string(),
        file["asked"].as_str().unwrap(),
    ) else {
        panic!("the fixture's personal case must read")
    };
    grant
}

#[test]
fn a_new_machine_of_the_owner_takes_the_personal_store_as_its_main_store() {
    let temp = TempDir::new("connect-personal");
    let layout = layout(&temp);
    // the main store as a machine that was here before has it: kept and pointed at the new key
    let store = layout.store();
    fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "--quiet"]);
    git(&store, &["remote", "add", "origin", "old@host:store.git"]);
    let grant = personal_grant();

    let pending = PendingKey::make(&layout).unwrap();
    let connected =
        vibememory_cli::personal_connect::connect(&layout, &grant, pending, Some("GPD-WIN-MAX2"))
            .unwrap();
    assert!(!connected.cloned && connected.configured);
    assert_eq!(connected.machine_id, "GPD-WIN-MAX2");
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(layout.engine_dir.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(config["machineId"], "GPD-WIN-MAX2");
    let state = layout.personal_state_dir();
    assert!(state.join(KEY_FILE).is_file() && state.join(KNOWN_HOSTS_FILE).is_file());
    assert_eq!(
        git(&store, &["config", "core.sshCommand"]),
        ssh_command(&state)
    );
    assert_eq!(
        git(&store, &["remote", "get-url", "origin"]),
        "vmgit@vibememory.ru:teams/personal.git"
    );
    // the personal store is no team: its record is found, and the teams stay as they were
    assert_eq!(
        read_record(&layout, "personal").unwrap().key_id,
        grant.key_id
    );
    assert!(vibememory_cli::team_connect::connected_teams(&layout).is_empty());

    // connecting again keeps the machine id the configuration has, whatever is asked
    let pending = PendingKey::make(&layout).unwrap();
    let again =
        vibememory_cli::personal_connect::connect(&layout, &grant, pending, Some("other")).unwrap();
    assert_eq!(again.machine_id, "GPD-WIN-MAX2");
    assert!(!again.configured);
}

#[test]
fn a_machine_id_is_a_name_every_system_takes_as_a_directory() {
    use vibememory_cli::personal_connect::{is_machine_id, machine_id};
    assert!(
        is_machine_id("GPD-WIN-MAX2") && is_machine_id("mac-main") && is_machine_id("box.local")
    );
    assert!(
        !is_machine_id("")
            && !is_machine_id(".hidden")
            && !is_machine_id("a/b")
            && !is_machine_id("a b")
    );
    assert_eq!(machine_id(Some(" gpd ")).as_deref(), Some("gpd"));
    assert_eq!(machine_id(Some("a/b")), None);
}

#[test]
fn a_clone_that_stopped_at_its_checkout_is_laid_out_on_a_machine_the_engine_never_ran_on() {
    let temp = TempDir::new("connect-personal-partial");
    let layout = layout(&temp);
    let store = layout.store();
    fs::create_dir_all(&store).unwrap();
    git(&store, &["init", "--quiet"]);
    git(&store, &["config", "user.email", "t@example.invalid"]);
    git(&store, &["config", "user.name", "t"]);
    fs::create_dir_all(store.join("projects/Acme")).unwrap();
    fs::write(store.join("projects/Acme/s.jsonl"), "{}\n").unwrap();
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "--quiet", "-m", "history"]);
    git(&store, &["remote", "add", "origin", "old@host:store.git"]);
    // what a checkout refused on Windows leaves: the history, not the files
    fs::remove_file(store.join("projects/Acme/s.jsonl")).unwrap();

    let pending = PendingKey::make(&layout).unwrap();
    vibememory_cli::personal_connect::connect(&layout, &personal_grant(), pending, Some("gpd"))
        .unwrap();
    assert!(
        store.join("projects/Acme/s.jsonl").is_file(),
        "the history's files are laid out"
    );
}
