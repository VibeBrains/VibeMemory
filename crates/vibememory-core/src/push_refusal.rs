//! What a team's host said when it refused a push, read from git's stderr, and what the machine
//! does about it. The host's `pre-receive` writes `vibememory: <code>` and then one line per path
//! or detail; git hands them to the client prefixed with `remote: `.

/// A refusal of the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The host's code: `quota`, `readOnly`, `pathDenied`, …
    pub code: String,
    /// The lines after it: the paths it refused, or what it says about the refusal.
    pub lines: Vec<String>,
}

/// What a refusal asks of the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// The team's standing in the cabinet: out of room, unpaid, the member or the machine removed.
    /// Pushes stop, reads go on, and the host is asked again after a while — a payment or a
    /// bigger plan ends the pause without anyone touching the machine.
    Cabinet,
    /// The clone holds what the team's store may not: a path outside its tree, a branch, a token.
    /// Pushes stop until the clone is made anew (`vibememory store reclone`).
    Reclone,
    /// The host itself is short of something for now: the next tick simply tries again.
    Transient,
    /// The team's sessions are switched off: its store is memory only now, written by the memory
    /// server alone and exported to its owner and admins. The machine leaves the store.
    SessionsOff,
}

/// The prefix git puts before every line the host writes.
const REMOTE: &str = "remote:";

/// The prefix the host puts before its code.
const MARK: &str = "vibememory:";

/// The host's refusal in git's stderr, if it is one.
#[must_use]
pub fn parse(stderr: &str) -> Option<Refusal> {
    // `remote:` opens git's line, or stands inside a line whose start is somebody's own words
    let mut lines = stderr.lines().map(|line| {
        line.split_once(REMOTE)
            .map_or((false, line.trim()), |(_, rest)| (true, rest.trim()))
    });
    let code = lines.by_ref().find_map(|(_, line)| {
        line.strip_prefix(MARK)
            .map(str::trim)
            .filter(|code| !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_owned)
    })?;
    let lines = lines
        .take_while(|(remote, _)| *remote)
        .map(|(_, line)| line.to_owned())
        .filter(|line| !line.is_empty())
        .collect();
    Some(Refusal { code, lines })
}

/// What to do about a code. An unknown code — a newer host — is treated as the cabinet's: the
/// safe answer is to stop pushing and ask again later, never to push harder.
#[must_use]
pub fn remedy(code: &str) -> Remedy {
    match code {
        "pathDenied" | "configDenied" | "refDenied" | "tokenInPush" | "contentTooLarge" => {
            Remedy::Reclone
        }
        "diskReserve" | "hostFailure" | "snapshotUnusable" => Remedy::Transient,
        "pushDenied" | "exportDenied" => Remedy::SessionsOff,
        _ => Remedy::Cabinet,
    }
}
