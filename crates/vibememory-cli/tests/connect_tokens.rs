//! What `connect` leaves on the machine and what `doctor` makes of it: the token in a file only its
//! owner reaches, a sidecar without it, a registration line that reads it at run time — and the
//! plan of a machine that has the binaries and no engine.

// The test writes files and reads their modes, so the purity gate is lifted here.
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

use support::TempDir;
use vibememory_cli::connect::{claude_code_registration, disconnect, headers_of, keep_token};
use vibememory_cli::credentials::{ClientStart, kept_tokens, record_client, started_clients};
use vibememory_cli::install::{Layout, Step, plan_binaries};
use vibememory_core::claim::{Claim, TokenGrant, read_answer};

const ANSWERS: &str = include_str!("../../../fixtures/claim/claimAnswers.json");

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    }
}

/// The grant of the fixture's `token` case, as `connect` reads it.
fn grant() -> TokenGrant {
    let file: serde_json::Value = serde_json::from_str(ANSWERS).unwrap();
    let case = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "token")
        .unwrap();
    let asked = file["asked"].as_str().unwrap();
    let Ok(Claim::Token(grant)) = read_answer(0, 200, &case["body"].to_string(), asked) else {
        panic!("the fixture's token case must read")
    };
    grant
}

/// A start of a memory server as the clients note reads it back.
fn started(stamp: &str, version: Option<&str>) -> ClientStart {
    ClientStart {
        stamp: stamp.to_owned(),
        version: version.map(str::to_owned),
    }
}

#[test]
fn the_token_is_kept_for_its_owner_alone_and_printed_nowhere() {
    let temp = TempDir::new("connect-keep");
    let layout = layout(&temp);
    let grant = grant();
    let kept = keep_token(&layout, &grant).unwrap();

    assert_eq!(fs::read_to_string(&kept.token).unwrap(), grant.token);
    let secret = grant.token.split('_').nth(2).unwrap();
    assert!(!fs::read_to_string(&kept.sidecar).unwrap().contains(secret));
    assert!(
        fs::read_to_string(&kept.fragment)
            .unwrap()
            .contains(&grant.token)
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&kept.token, &kept.sidecar, &kept.fragment] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let directory = kept.token.parent().unwrap();
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    let binary = layout.engine_dir.join("bin").join("vibememory");
    let line = claude_code_registration(&grant, &binary);
    assert!(!line.contains(secret), "the line must not carry the token");
    assert!(
        !line.contains("$(cat"),
        "nor read it into an argument: {line}"
    );
    assert!(
        line.starts_with("claude mcp add-json --scope user vibememory-vibebrains '"),
        "{line}"
    );
    let config = line
        .strip_prefix("claude mcp add-json --scope user vibememory-vibebrains '")
        .and_then(|rest| rest.strip_suffix('\''))
        .expect("one quoted word")
        // a quote inside the word is closed, escaped and reopened, as a POSIX shell reads it
        .replace("'\\''", "'");
    let config: serde_json::Value = serde_json::from_str(&config).expect("JSON");
    assert_eq!(config["url"], grant.mcp_url.as_str());
    assert_eq!(
        config["headersHelper"],
        format!("'{}' mcp-headers vibebrains claude-code", binary.display())
    );

    // What the helper prints when Claude Code connects: the header, read from the kept file.
    let headers = headers_of(&layout, "vibebrains", "claude-code").expect("headers");
    let headers: serde_json::Value = serde_json::from_str(&headers).expect("JSON");
    assert_eq!(headers["Authorization"], format!("Bearer {}", grant.token));
    assert!(
        headers_of(&layout, "../x", "claude-code").is_err(),
        "names only"
    );
}

#[test]
fn the_code_is_the_first_line_of_what_is_typed_or_piped() {
    use vibememory_cli::connect::read_code;
    assert_eq!(
        read_code("ABCD-EFGH-JKMN\nrest\n".as_bytes()),
        Ok("ABCD-EFGH-JKMN".to_owned())
    );
    assert_eq!(
        read_code("  ABCD-EFGH-JKMN\r\n".as_bytes()),
        Ok("ABCD-EFGH-JKMN".to_owned())
    );
    assert!(read_code("\n".as_bytes()).is_err());
    assert!(read_code("".as_bytes()).is_err());
}

#[test]
fn doctor_finds_the_token_and_says_when_others_can_read_it() {
    let temp = TempDir::new("connect-doctor");
    let layout = layout(&temp);
    let grant = grant();
    let kept = keep_token(&layout, &grant).unwrap();

    let tokens = kept_tokens(&layout);
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        (
            tokens[0].team.as_str(),
            tokens[0].agent.as_str(),
            tokens[0].token_id.as_str()
        ),
        ("vibebrains", "claude-code", "tk_7q2m9x4a")
    );
    assert_eq!(tokens[0].problems, Vec::<String>::new());
    assert_eq!(
        tokens[0].client_started, None,
        "a token no client has used yet says so"
    );
    record_client(
        &layout.engine_dir,
        "claude-code",
        "2026-09-30T09:00:00Z",
        "0.5.0",
    )
    .unwrap();
    record_client(
        &layout.engine_dir,
        "deepseek-typo",
        "2026-09-30T09:00:01Z",
        "0.4.3",
    )
    .unwrap();
    assert_eq!(
        kept_tokens(&layout)[0].client_started,
        Some(started("2026-09-30T09:00:00Z", Some("0.5.0"))),
        "by the token's own agent name, not by any client that ran"
    );
    assert_eq!(
        started_clients(&layout.engine_dir),
        vec![
            (
                "claude-code".to_owned(),
                started("2026-09-30T09:00:00Z", Some("0.5.0"))
            ),
            (
                "deepseek-typo".to_owned(),
                started("2026-09-30T09:00:01Z", Some("0.4.3"))
            ),
        ],
        "every client that started a server, with a token or without"
    );
    // A server that started before the engine it runs beside was updated keeps the old binary
    // until its client starts it again: that is what doctor warns about.
    let clients = started_clients(&layout.engine_dir);
    assert!(
        clients[1].1.behind("0.5.0"),
        "0.4.3 runs behind 0.5.0 on disk"
    );
    assert!(
        !clients[0].1.behind("0.5.0"),
        "the same version is not behind"
    );

    // A note from before the version field holds the time alone, and is read as such.
    fs::write(
        layout.engine_dir.join("clients").join("claude-code"),
        "2026-09-29T08:00:00Z\n",
    )
    .unwrap();
    let old = kept_tokens(&layout)[0].client_started.clone().unwrap();
    assert_eq!(old, started("2026-09-29T08:00:00Z", None));
    assert!(
        !old.behind("0.5.0"),
        "a note without a version proves nothing either way"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&kept.token, fs::Permissions::from_mode(0o644)).unwrap();
        let tokens = kept_tokens(&layout);
        assert_eq!(tokens[0].problems.len(), 1);
        assert!(tokens[0].problems[0].contains("readable by others"));
    }

    fs::remove_file(&kept.fragment).unwrap();
    assert!(
        kept_tokens(&layout)[0]
            .problems
            .iter()
            .any(|problem| problem.contains("is missing"))
    );
}

#[test]
fn a_machine_without_the_engine_plans_the_binaries_curl_and_path_only() {
    let temp = TempDir::new("connect-binaries");
    let steps: Vec<Step> = plan_binaries(&layout(&temp))
        .into_iter()
        .map(|action| action.step)
        .collect();
    assert_eq!(steps.first(), Some(&Step::Binary));
    // last: the programs on PATH, so the member types `vibememory connect` by name in a new terminal
    assert_eq!(steps.last(), Some(&Step::OnPath));
    assert!(steps.contains(&Step::Curl));
    assert!(steps.iter().all(|step| matches!(
        step,
        Step::Binary | Step::McpBinary | Step::Curl | Step::OnPath
    )));
}

#[test]
fn connect_takes_no_code_and_no_stray_word_from_the_command_line() {
    let cabinet = "https://app.vibememory.ru";
    let cases: [(&[&str], &str); 5] = [
        (
            &["--code", "ABCD-EFGH-JKMN", "--cabinet", cabinet],
            "not taken from the command line",
        ),
        (
            &["--code=ABCD-EFGH-JKMN", "--cabinet", cabinet],
            "not taken from the command line",
        ),
        (
            &["--cabinet", cabinet, "ABCD-EFGH-JKMN"],
            "is not an argument of connect",
        ),
        (&["--cabinet", "http://app.vibememory.ru"], "is not https"),
        (&[], "--cabinet is required"),
    ];
    for (args, said) in cases {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
            .arg("connect")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("the engine binary runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(said), "{args:?}: {stderr}");
    }
}

#[test]
fn disconnect_takes_the_teams_tokens_off_the_machine_and_names_where_to_revoke_them() {
    let temp = TempDir::new("disconnect");
    let layout = layout(&temp);
    let grant = grant();
    let kept = keep_token(&layout, &grant).unwrap();
    // A token of another team stays; one kept by hand in this team is left for the person.
    let other = TokenGrant {
        team: "otherteam".to_owned(),
        ..grant.clone()
    };
    let other_kept = keep_token(&layout, &other).unwrap();
    let by_hand = layout
        .engine_dir
        .join("tokens")
        .join(&grant.team)
        .join("by-hand");
    fs::write(&by_hand, "0123456789abcdef".repeat(4)).unwrap();

    let done = disconnect(&layout, &grant.team).unwrap();
    for path in [&kept.token, &kept.sidecar, &kept.fragment] {
        assert!(!path.exists(), "{} is gone", path.display());
        assert!(done.removed.contains(path));
    }
    assert_eq!(
        done.revoke,
        [(grant.token_id.clone(), grant.cabinet.clone())]
    );
    assert_eq!(done.left, std::slice::from_ref(&by_hand));
    assert!(other_kept.token.exists(), "another team's token stays");

    // Without the hand-kept file the directory goes too, and doctor finds nothing of the team.
    fs::remove_file(&by_hand).unwrap();
    let again = disconnect(&layout, &grant.team).unwrap();
    assert!(again.removed.is_empty() && again.left.is_empty());
    assert!(!layout.engine_dir.join("tokens").join(&grant.team).exists());
    assert!(
        kept_tokens(&layout)
            .iter()
            .all(|token| token.team != grant.team)
    );
    assert!(
        disconnect(&layout, "../escape").is_err(),
        "a name that is not a team"
    );
}
