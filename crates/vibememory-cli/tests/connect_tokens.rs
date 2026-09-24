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
use vibememory_cli::connect::{claude_code_registration, keep_token};
use vibememory_cli::credentials::kept_tokens;
use vibememory_cli::install::{Layout, Step, plan_binaries};
use vibememory_core::claim::{Claim, TokenGrant, read_answer};

const ANSWERS: &str = include_str!("../../../fixtures/claim/claimAnswers.json");

fn layout(temp: &TempDir) -> Layout {
    Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
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
    let Ok(Claim::Token(grant)) = read_answer(0, &case["body"].to_string(), asked) else {
        panic!("the fixture's token case must read")
    };
    grant
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

    let line = claude_code_registration(&grant, &kept.token);
    assert!(
        line.contains("$(cat "),
        "the line reads the token when it runs"
    );
    assert!(!line.contains(secret), "the line must not carry the token");
    assert!(line.contains("vibememory-vibebrains"));
    assert!(
        line.contains(&format!(" '{}' ", grant.mcp_url)),
        "the address is one quoted word: {line}"
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
fn a_machine_without_the_engine_plans_the_binaries_and_curl_only() {
    let temp = TempDir::new("connect-binaries");
    let steps: Vec<Step> = plan_binaries(&layout(&temp))
        .into_iter()
        .map(|action| action.step)
        .collect();
    assert_eq!(steps.first(), Some(&Step::Binary));
    assert_eq!(steps.last(), Some(&Step::Curl));
    assert!(
        steps
            .iter()
            .all(|step| matches!(step, Step::Binary | Step::McpBinary | Step::Curl))
    );
}
