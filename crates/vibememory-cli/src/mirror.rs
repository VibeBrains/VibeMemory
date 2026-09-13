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

/// The store's path as the host's shell must see it.
///
/// A remote is written three ways in the wild and all three are legal: `alias:store.git`
/// (relative to the home directory, what `hostBootstrap.sh` makes), `alias:~/store.git` (the form
/// the manual and CLAUDE.md use) and `alias:/srv/store.git` (absolute). Gluing `$HOME/` in front
/// of all of them turned the second into `$HOME/~/store.git`, and the probe of a store set up by
/// the book failed silently.
#[must_use]
pub fn shell_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("$HOME/{rest}")
    } else {
        format!("$HOME/{path}")
    }
}

/// What is asked of the host first: the hook's text and the head it holds.
///
/// The hook is printed, not parsed here: the rule for reading it lives in [`url_in_hook`], under
/// a gate. A `grep` in this string would be a second implementation of the same format — the one
/// that actually runs, and the only one nothing tests.
#[must_use]
pub fn hook_script(path: &str, branch: &str) -> String {
    let store = shell_path(path);
    format!(
        "hook={store}/hooks/post-receive; \
         echo \"HEAD $(git -C {store} rev-parse {branch} 2>/dev/null)\"; \
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

/// How long to wait before asking again after the host or the provider did not answer. Without
/// it an unreachable provider means two ssh calls every two minutes, all day.
pub const RETRY_AFTER: i64 = 60 * 60;

/// Whether the periodic check is due, given when it last ran.
///
/// A clock that jumped backwards (machines disagree, and one of them is always wrong) makes the
/// check due rather than never due again: an early question costs one ssh, a skipped one costs
/// the backup.
#[must_use]
pub const fn check_due(last_checked: Option<i64>, now: i64) -> bool {
    due_after(last_checked, now, CHECK_INTERVAL)
}

/// Whether to ask the host at all right now.
///
/// Both clocks have to agree: the daily question must be due, and an attempt that got no answer
/// must have had its hour. A host that never answers leaves `last_checked` empty forever, so
/// without the second clock the daily rule would say "due" on every tick — two ssh calls every
/// two minutes, all day.
#[must_use]
pub const fn should_probe(last_checked: Option<i64>, last_attempt: Option<i64>, now: i64) -> bool {
    check_due(last_checked, now) && retry_due(last_attempt, now)
}

/// Whether a failed attempt may be retried yet.
#[must_use]
pub const fn retry_due(last_attempt: Option<i64>, now: i64) -> bool {
    due_after(last_attempt, now, RETRY_AFTER)
}

/// The shared rule: never asked means ask; a clock that went backwards means ask; otherwise wait
/// out the interval.
const fn due_after(last: Option<i64>, now: i64, interval: i64) -> bool {
    match last {
        None => true,
        Some(last) => now < last || now - last >= interval,
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

/// The column the disk line is printed in, beside the mirror's.
const DISK_COLUMN: &str = "disk     ";

/// Below this much free space on the store's host, a session is told and `doctor` fails.
pub const LOW_DISK_KIB: u64 = 1024 * 1024;

/// …or below this share of the whole disk, whichever comes first.
pub const LOW_DISK_PERCENT: u64 = 10;

/// Free space on the disk that holds the store, as the host's `df` reports it.
///
/// Asked because a full host refuses every push while each machine goes on working: on 2026-09-12
/// the 10 GiB disk filled up and nothing said so for a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    /// Free, in KiB.
    pub available_kib: u64,
    /// The whole disk, in KiB.
    pub total_kib: u64,
}

impl Disk {
    /// Whether this is worth waking someone for.
    #[must_use]
    pub const fn is_low(&self) -> bool {
        self.available_kib < LOW_DISK_KIB
            || self.available_kib.saturating_mul(100)
                < self.total_kib.saturating_mul(LOW_DISK_PERCENT)
    }

    /// `2.5 GiB free of 9.8 GiB`.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} free of {}",
            gib(self.available_kib),
            gib(self.total_kib)
        )
    }

    /// One line for `doctor`, in the column shape of the other lines.
    #[must_use]
    pub fn describe(&self) -> String {
        let low = if self.is_low() { " \u{2014} LOW" } else { "" };
        format!("{DISK_COLUMN}host {}{low}", self.summary())
    }
}

/// KiB as GiB with one decimal, without a float.
fn gib(kib: u64) -> String {
    let tenths = kib.saturating_mul(10) / (1024 * 1024);
    format!("{}.{} GiB", tenths / 10, tenths % 10)
}

/// What is asked of the host: the `df` line of the disk that holds the store.
#[must_use]
pub fn disk_script(path: &str) -> String {
    format!("df -Pk {} | tail -1", shell_path(path))
}

/// Reads the `df -Pk` line: filesystem, size, used, available, capacity, mount point.
#[must_use]
pub fn read_disk(output: &str) -> Option<Disk> {
    let line = output.lines().rev().find(|line| !line.trim().is_empty())?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    Some(Disk {
        total_kib: fields.get(1)?.parse().ok()?,
        available_kib: fields.get(3)?.parse().ok()?,
    })
}

/// Asks the host about its disk; `None` when there is no host to ask.
pub fn probe_disk<F>(remote: Option<&str>, mut run: F) -> Option<Result<Disk, String>>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let host = remote.and_then(host_of)?;
    Some(run(host.ssh, &disk_script(host.path)).and_then(|text| {
        read_disk(&text).ok_or_else(|| format!("unreadable df answer: {}", text.trim()))
    }))
}

/// What a session is told when the host is filling up — before pushes start failing, not after.
#[must_use]
pub fn disk_note(disk: &Disk) -> Option<String> {
    disk.is_low().then(|| {
        format!(
            "VibeMemory: the store's host is running out of disk \u{2014} {}. Pushes stop when it \
             fills up: repack the store there or give the host more space.",
            disk.summary()
        )
    })
}
