//! Everything the mirror probe decides without the network: which remote can be asked, what the
//! host's hook says, and what the two heads mean.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_cli::mirror::{
    Mirror, hook_script, host_of, read_hook_answer, repo_name, url_in_hook, verdict,
};

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
                # Mirrors every push. git push --mirror 'storeMirror:decoy/decoy.git'\n\
                set -euo pipefail\n\
                git push --mirror 'storeMirror:VibeBrains/VibeMemoryStore.git' >/dev/null 2>&1\n";
    assert_eq!(
        url_in_hook(hook).as_deref(),
        Some("storeMirror:VibeBrains/VibeMemoryStore.git")
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
        repo_name("storeMirror:VibeBrains/Store.git"),
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
    // No hook: a mirror is optional, so the answer carries no URL and nothing is at fault.
    let answer = read_hook_answer("HEAD 610d531\nNOHOOK\n");
    assert_eq!(answer.head, "610d531");
    assert_eq!(answer.url, None);

    let with_hook = "HEAD 610d531ac67d64fb12f95a9cead8422dffded887\n\
                     HOOK\n\
                     #!/usr/bin/env bash\n\
                     set -euo pipefail\n\
                     git push --mirror 'storeMirror:VibeBrains/VibeMemoryStore.git' >/dev/null 2>&1\n";
    let answer = read_hook_answer(with_hook);
    assert_eq!(answer.head, "610d531ac67d64fb12f95a9cead8422dffded887");
    assert_eq!(
        answer.url.as_deref(),
        Some("storeMirror:VibeBrains/VibeMemoryStore.git")
    );

    // An ssh banner ahead of the answer must not become a head.
    let noisy = read_hook_answer("Welcome to Ubuntu\nHEAD abc1234\nNOHOOK\n");
    assert_eq!(noisy.head, "abc1234");
}

#[test]
fn the_question_asked_of_the_host_names_the_branch_and_the_path() {
    let script = hook_script("vibememory/store.git", "main");
    assert!(script.contains("$HOME/vibememory/store.git/hooks/post-receive"));
    assert!(script.contains("rev-parse main"));
    // The host prints the hook; it must not try to read it.
    assert!(
        !script.contains("grep"),
        "parsing belongs to url_in_hook, not to the shell"
    );
}

#[test]
fn the_probe_asks_twice_and_only_when_there_is_something_to_ask() {
    use std::cell::RefCell;
    use vibememory_cli::mirror::probe;

    let asked: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let hook = "HEAD aaa1111\nHOOK\ngit push --mirror 'storeMirror:o/n.git'\n";
    let mirror = probe(Some("vibememory:store.git"), "main", |host, script| {
        asked.borrow_mut().push(host.to_owned());
        Ok(if script.contains("ls-remote") {
            "aaa1111\n".to_owned()
        } else {
            hook.to_owned()
        })
    });
    assert_eq!(
        asked.borrow().len(),
        2,
        "hook first, then the mirror's head"
    );
    assert!(matches!(mirror, Mirror::InSync { .. }));

    // No hook: the second question is never asked, because there is nothing to ask about.
    let asked = RefCell::new(0_usize);
    let mirror = probe(Some("vibememory:store.git"), "main", |_, _| {
        *asked.borrow_mut() += 1;
        Ok("HEAD aaa1111\nNOHOOK\n".to_owned())
    });
    assert_eq!(*asked.borrow(), 1);
    assert_eq!(mirror, Mirror::NotConfigured);

    // No remote at all: the host is never contacted.
    let mirror = probe(None, "main", |_, _| panic!("must not ask anyone"));
    assert_eq!(mirror, Mirror::NoHost);

    // The host unreachable is unknown, never "in sync", and never a fault of the mirror.
    let mirror = probe(Some("vibememory:store.git"), "main", |_, _| {
        Err("ssh: connect timed out".to_owned())
    });
    assert!(matches!(mirror, Mirror::Unknown { .. }));
    assert!(!mirror.is_fault());
}

#[test]
fn the_daily_check_comes_due_and_survives_a_clock_that_went_backwards() {
    use vibememory_cli::mirror::{CHECK_INTERVAL, check_due};

    assert!(check_due(None, 1_000), "never asked means ask now");
    assert!(!check_due(Some(1_000), 1_000 + CHECK_INTERVAL - 1));
    assert!(check_due(Some(1_000), 1_000 + CHECK_INTERVAL));
    // A clock that jumped back would otherwise silence the check for as long as the jump lasted.
    assert!(check_due(Some(5_000), 1_000));
}

#[test]
fn only_a_stale_backup_is_worth_waking_a_session_for() {
    use vibememory_cli::mirror::session_note;

    let stale = verdict("o/n", "aaa1111", "bbb2222");
    let note = session_note(&stale).expect("a stale backup is news");
    assert!(
        note.contains("o/n"),
        "the note names the repository: {note}"
    );
    assert!(
        !note.contains("mirror   "),
        "the doctor's column has no place in a sentence"
    );

    assert_eq!(session_note(&verdict("o/n", "aaa1111", "aaa1111")), None);
    assert_eq!(
        session_note(&Mirror::NotConfigured),
        None,
        "absence is a choice"
    );
    assert_eq!(
        session_note(&Mirror::Unknown {
            reason: "ssh".to_owned()
        }),
        None,
        "an unreachable host is not a broken backup"
    );
}
