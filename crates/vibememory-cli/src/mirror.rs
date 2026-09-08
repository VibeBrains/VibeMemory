//! The backup mirror of the store: a private GitHub repository the host pushes to after every
//! accepted push.
//!
//! The mirror belongs to the host, not to this machine — this machine has no key for it and
//! cannot read it directly. So the whole probe is one question asked of the host, and everything
//! that can be decided without the network is decided here, in functions that take text.

use std::time::Duration;

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
    (!url.is_empty()).then(|| url.to_owned())
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
    /// One line for `status` and `doctor`, in the same shape as the install steps.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::NoHost => "mirror   no ssh remote in config — nothing to mirror from".to_owned(),
            Self::NotConfigured => {
                "mirror   not configured — ./infra/mirrorSetup.sh --repo owner/name".to_owned()
            }
            Self::InSync { repo, head } => {
                format!("mirror   {repo} in sync at {}", short(head))
            }
            Self::Diverged { repo, host, mirror } => format!(
                "mirror   {repo} DIVERGED — host {}, mirror {}",
                short(host),
                short(mirror)
            ),
            Self::Unknown { reason } => format!("mirror   unknown — {reason}"),
        }
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

/// The one command asked of the host: the hook's text, the local head, and the mirror's head.
///
/// Kept as one round trip on purpose — three separate ssh sessions would cost three handshakes
/// and could observe three different moments.
#[must_use]
pub fn probe_script(path: &str, branch: &str) -> String {
    format!(
        "hook=$HOME/{path}/hooks/post-receive; \
         [ -f \"$hook\" ] || {{ echo NOHOOK; exit 0; }}; \
         url=$(grep -o \"push --mirror '[^']*'\" \"$hook\" | head -1 | cut -d\\' -f2); \
         [ -n \"$url\" ] || {{ echo NOHOOK; exit 0; }}; \
         echo \"URL $url\"; \
         echo \"HOST $(git -C $HOME/{path} rev-parse {branch} 2>/dev/null)\"; \
         echo \"MIRROR $(git ls-remote \"$url\" {branch} 2>/dev/null | cut -f1)\""
    )
}

/// Reads what [`probe_script`] printed.
#[must_use]
pub fn read_probe(output: &str) -> Mirror {
    if output.lines().any(|line| line.trim() == "NOHOOK") {
        return Mirror::NotConfigured;
    }
    let field = |name: &str| {
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix(name))
            .map(str::trim)
            .unwrap_or_default()
            .to_owned()
    };
    let url = field("URL ");
    if url.is_empty() {
        return Mirror::Unknown {
            reason: "хост не назвал репозиторий зеркала".to_owned(),
        };
    }
    verdict(&repo_name(&url), &field("HOST "), &field("MIRROR "))
}

/// The first seven characters of a hash, or the whole word when it is not one.
fn short(head: &str) -> &str {
    head.get(..7).unwrap_or(head)
}
