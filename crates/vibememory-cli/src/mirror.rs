//! The backup mirror of the store: a private GitHub repository the host pushes to after every
//! accepted push.
//!
//! The mirror belongs to the host, not to this machine — this machine has no key for it and
//! cannot read it directly. So the whole probe is one question asked of the host, and everything
//! that can be decided without the network is decided here, in functions that take text.

use std::time::Duration;

/// The column `doctor` prints the mirror in, aligned with the install steps beside it.
const COLUMN: &str = "mirror   ";

/// Where the store's host lives, as written in `config.json`'s `remote`.
#[derive(Debug, PartialEq, Eq)]
pub struct Host<'a> {
    /// The ssh alias or `user@server`.
    pub ssh: &'a str,
    /// Path of the bare repository, as the host's shell sees it.
    pub path: &'a str,
}

/// Splits `alias:path/to/store.git` into its two halves.
///
/// Returns `None` for anything else — an https remote, a bare path, an empty string. A mirror
/// lives behind ssh on a host we can log into; nothing else can be asked the question.
#[must_use]
pub fn host_of(remote: &str) -> Option<Host<'_>> {
    let remote = remote.trim();
    if remote.is_empty() || remote.contains("://") {
        return None;
    }
    let (ssh, path) = remote.split_once(':')?;
    if ssh.is_empty() || path.is_empty() || path.contains(' ') {
        return None;
    }
    Some(Host { ssh, path })
}

/// Pulls the mirror's URL out of a `post-receive` hook written by `hostBootstrap.sh`.
///
/// The hook is the only place the URL is recorded, and it is per-host by design: every owner
/// mirrors into their own private repository, so nothing about it can be assumed here.
#[must_use]
pub fn url_in_hook(hook: &str) -> Option<String> {
    let line = hook
        .lines()
        .find(|line| line.contains("git push --mirror") && !line.trim_start().starts_with('#'))?;
    let rest = line.split("--mirror").nth(1)?.trim();
    let url = rest
        .strip_prefix('\'')
        .and_then(|rest| rest.split('\'').next())
        .or_else(|| {
            rest.strip_prefix('"')
                .and_then(|rest| rest.split('"').next())
        })
        .or_else(|| rest.split_whitespace().next())?;
    // The URL is handed back to the host inside a shell command, so anything that could end the
    // quoting or start something else is not a URL as far as this function is concerned. A hook
    // written by hand with a space in the path fails loudly here instead of quietly running.
    let safe = !url.is_empty()
        && !url.contains(['\'', '"', '`', '$', ';', '&', '|', '\n', '\r', ' ', '\t']);
    safe.then(|| url.to_owned())
}

/// What the probe found.
#[derive(Debug, PartialEq, Eq)]
pub enum Mirror {
    /// No `remote` in the config, or one we cannot log into: nothing to ask.
    NoHost,
    /// The host has no mirror hook. A mirror is optional, so this is a statement, not a fault.
    NotConfigured,
    /// The mirror holds what the host holds.
    InSync {
        /// `owner/name` of the mirror, for a line a human reads.
        repo: String,
        /// The head both sides agree on.
        head: String,
    },
    /// The mirror is behind, ahead, or empty. Named, because a backup nobody checked is not one.
    Diverged {
        /// `owner/name` of the mirror.
        repo: String,
        /// What the host holds.
        host: String,
        /// What the mirror holds, or `пусто` when it holds nothing.
        mirror: String,
    },
    /// The question could not be asked: host down, key gone, GitHub unreachable.
    Unknown {
        /// Why the question could not be answered.
        reason: String,
    },
}

impl Mirror {
    /// What is wrong or right, as a sentence — without the column `doctor` prints it in.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::NoHost => "no ssh remote in config \u{2014} nothing to mirror from".to_owned(),
            Self::NotConfigured => {
                "not configured \u{2014} ./infra/mirrorSetup.sh --repo owner/name".to_owned()
            }
            Self::InSync { repo, head } => format!("{repo} in sync at {}", short(head)),
            Self::Diverged { repo, host, mirror } => format!(
                "{repo} DIVERGED \u{2014} host {}, mirror {}",
                short(host),
                short(mirror)
            ),
            Self::Unknown { reason } => format!("unknown \u{2014} {reason}"),
        }
    }

    /// One line for `status` and `doctor`, in the same shape as the install steps.
    #[must_use]
    pub fn describe(&self) -> String {
        format!("{COLUMN}{}", self.summary())
    }

    /// Whether `doctor` should fail on this. A missing mirror is a choice; a mirror that stopped
    /// following the host is a broken backup, and silence about it is how backups are lost.
    #[must_use]
    pub const fn is_fault(&self) -> bool {
        matches!(self, Self::Diverged { .. })
    }
}

/// Decides the verdict from the two heads, as text, exactly as git prints them.
#[must_use]
pub fn verdict(repo: &str, host_head: &str, mirror_head: &str) -> Mirror {
    let host = host_head.trim();
    let mirror = mirror_head.trim();
    if host.is_empty() {
        return Mirror::Unknown {
            reason: "хост не назвал голову ветки".to_owned(),
        };
    }
    if host == mirror {
        Mirror::InSync {
            repo: repo.to_owned(),
            head: host.to_owned(),
        }
    } else {
        Mirror::Diverged {
            repo: repo.to_owned(),
            host: host.to_owned(),
            mirror: if mirror.is_empty() {
                "пусто".to_owned()
            } else {
                mirror.to_owned()
            },
        }
    }
}

/// Turns `githubMirror:owner/name.git` into `owner/name`, for a line a human can read.
#[must_use]
pub fn repo_name(url: &str) -> String {
    let after_host = url.rsplit_once(':').map_or(url, |(_, rest)| rest);
    after_host.trim_end_matches(".git").to_owned()
}

/// How long the host is given to answer. The probe runs in `doctor`, which a human is watching,
/// and it talks to two networks in turn — the host, then GitHub from the host.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// What is asked of the host first: the hook's text and the head it holds.
///
/// The hook is printed, not parsed here: the rule for reading it lives in [`url_in_hook`], under
/// a gate. A `grep` in this string would be a second implementation of the same format — the one
/// that actually runs, and the only one nothing tests.
#[must_use]
pub fn hook_script(path: &str, branch: &str) -> String {
    format!(
        "hook=$HOME/{path}/hooks/post-receive; \
         echo \"HEAD $(git -C $HOME/{path} rev-parse {branch} 2>/dev/null)\"; \
         [ -f \"$hook\" ] && {{ echo HOOK; cat \"$hook\"; }} || echo NOHOOK"
    )
}

/// What is asked of the host second, once [`url_in_hook`] has named the mirror: the head the
/// mirror holds. Only the host has the key, so only the host can ask.
#[must_use]
pub fn remote_head_script(url: &str, branch: &str) -> String {
    format!("git ls-remote '{url}' {branch} 2>/dev/null | cut -f1")
}

/// The head and the mirror's URL, as read from what [`hook_script`] printed.
#[must_use]
pub fn read_hook_answer(output: &str) -> HookAnswer {
    let head = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("HEAD "))
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    let hook = output.split_once("HOOK\n").map(|(_, rest)| rest);
    HookAnswer {
        head,
        url: hook.and_then(url_in_hook),
    }
}

/// What the host said about itself.
#[derive(Debug, PartialEq, Eq)]
pub struct HookAnswer {
    /// The head of the store's branch on the host.
    pub head: String,
    /// The mirror's URL, when a mirror hook is installed.
    pub url: Option<String>,
}

/// The first seven characters of a hash, or the whole word when it is not one.
fn short(head: &str) -> &str {
    head.get(..7).unwrap_or(head)
}

/// How often the tick asks about the mirror. A backup that rots is found by asking, and asking
/// on every tick would mean two ssh round trips every few minutes for a question whose answer
/// changes at most as often as a push.
pub const CHECK_INTERVAL: i64 = 24 * 60 * 60;

/// Whether the periodic check is due, given when it last ran.
///
/// A clock that jumped backwards (machines disagree, and one of them is always wrong) makes the
/// check due rather than never due again: an early question costs one ssh, a skipped one costs
/// the backup.
#[must_use]
pub const fn check_due(last_checked: Option<i64>, now: i64) -> bool {
    match last_checked {
        None => true,
        Some(last) => now < last || now - last >= CHECK_INTERVAL,
    }
}

/// Asks the host about its mirror, using `run` to reach it.
///
/// The two questions are separate because only the host holds the key to the mirror: the first
/// asks what the hook says, the second asks the mirror itself, through the host.
pub fn probe<F>(remote: Option<&str>, branch: &str, mut run: F) -> Mirror
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let Some(host) = remote.and_then(host_of) else {
        return Mirror::NoHost;
    };
    let answer = match run(host.ssh, &hook_script(host.path, branch)) {
        Ok(text) => read_hook_answer(&text),
        Err(reason) => return Mirror::Unknown { reason },
    };
    let Some(url) = answer.url else {
        return Mirror::NotConfigured;
    };
    match run(host.ssh, &remote_head_script(&url, branch)) {
        Ok(head) => verdict(&repo_name(&url), &answer.head, &head),
        Err(reason) => Mirror::Unknown { reason },
    }
}

/// What a session is told when the backup stopped following the host. Said once a day at most,
/// and only about a fault: a mirror that is merely absent is a choice, not news.
///
/// English, like every other note the engine hands a session — one voice, whatever the reader's
/// own language is.
#[must_use]
pub fn session_note(mirror: &Mirror) -> Option<String> {
    mirror.is_fault().then(|| {
        format!(
            "VibeMemory: the backup mirror fell behind the host \u{2014} {}.",
            mirror.summary()
        )
    })
}
