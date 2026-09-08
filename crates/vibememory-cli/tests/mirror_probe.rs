//! Everything the mirror probe decides without the network: which remote can be asked, what the
//! host's hook says, and what the two heads mean.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_cli::mirror::{Mirror, host_of, read_probe, repo_name, url_in_hook, verdict};

#[test]
fn only_an_ssh_remote_can_be_asked() {
    let host = host_of("vibememory:vibememory/store.git").expect("ssh remote");
    assert_eq!(host.ssh, "vibememory");
    assert_eq!(host.path, "vibememory/store.git");

    let host = host_of("vm@vibememory.ru:~/store.git").expect("user@server remote");
    assert_eq!(host.ssh, "vm@vibememory.ru");

    // Nothing we can log into: no probe, and no pretending the mirror is fine.
    for remote in [
        "",
        "   ",
        "https://github.com/owner/name.git",
        "ssh://host/path.git",
        "/Volumes/backup/store.git",
        "alias:",
        ":path.git",
        "alias:path with space.git",
    ] {
        assert!(host_of(remote).is_none(), "must refuse {remote:?}");
    }
}

#[test]
fn the_hook_names_the_repository_and_the_comment_does_not() {
    let hook = "#!/usr/bin/env bash\n\
                # Mirrors every push. git push --mirror 'githubMirror:decoy/decoy.git'\n\
                set -euo pipefail\n\
                git push --mirror 'githubMirror:VibeBrains/VibeMemoryStore.git' >/dev/null 2>&1\n";
    assert_eq!(
        url_in_hook(hook).as_deref(),
        Some("githubMirror:VibeBrains/VibeMemoryStore.git")
    );
    assert_eq!(
        url_in_hook("#!/usr/bin/env bash\nexit 0\n"),
        None,
        "a hook without a mirror push names no repository"
    );
}

#[test]
fn a_repository_name_survives_any_host_alias() {
    assert_eq!(
        repo_name("githubMirror:VibeBrains/Store.git"),
        "VibeBrains/Store"
    );
    assert_eq!(repo_name("git@gitlab.com:team/store.git"), "team/store");
    // Not GitHub at all: a self-hosted Gitea over ssh. The line must still read sensibly.
    assert_eq!(repo_name("gitea:backups/store"), "backups/store");
}

#[test]
fn heads_decide_the_verdict() {
    assert_eq!(
        verdict("o/n", "abc1234\n", "abc1234"),
        Mirror::InSync {
            repo: "o/n".to_owned(),
            head: "abc1234".to_owned()
        }
    );
    // An empty mirror is a diverged mirror, said out loud: this is the state right after the
    // repository is created and before the first upload.
    assert_eq!(
        verdict("o/n", "abc1234", ""),
        Mirror::Diverged {
            repo: "o/n".to_owned(),
            host: "abc1234".to_owned(),
            mirror: "\u{43f}\u{443}\u{441}\u{442}\u{43e}".to_owned()
        }
    );
    assert!(verdict("o/n", "abc", "def").is_fault());
    assert!(!verdict("o/n", "abc", "abc").is_fault());
    // No head from the host is not a fault of the mirror, and must not be reported as one.
    assert!(matches!(verdict("o/n", "", "abc"), Mirror::Unknown { .. }));
}

#[test]
fn the_hosts_answer_is_read_back() {
    assert_eq!(read_probe("NOHOOK\n"), Mirror::NotConfigured);
    assert!(!Mirror::NotConfigured.is_fault(), "a mirror is optional");

    let answer = "URL githubMirror:VibeBrains/VibeMemoryStore.git\n\
                  HOST 610d531ac67d64fb12f95a9cead8422dffded887\n\
                  MIRROR 610d531ac67d64fb12f95a9cead8422dffded887\n";
    match read_probe(answer) {
        Mirror::InSync { repo, .. } => assert_eq!(repo, "VibeBrains/VibeMemoryStore"),
        other => panic!("expected in sync, got {other:?}"),
    }

    let behind = "URL githubMirror:VibeBrains/VibeMemoryStore.git\n\
                  HOST 610d531ac67d64fb12f95a9cead8422dffded887\n\
                  MIRROR 4ab7be0152df289f9367b6275675e3dd3deedd00\n";
    assert!(
        read_probe(behind).is_fault(),
        "a stale backup must be named"
    );
    assert!(read_probe(behind).describe().contains("DIVERGED"));

    // Garbage, or an ssh that printed a banner: unknown, never "in sync".
    assert!(matches!(
        read_probe("Welcome to Ubuntu\n"),
        Mirror::Unknown { .. }
    ));
}
