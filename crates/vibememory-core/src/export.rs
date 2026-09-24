//! What may leave the machine.
//!
//! The store is a git repository with a remote, so everything the engine puts into it is on some
//! host and on every other machine within minutes. The config directory of Claude Code, however,
//! is not a folder of documents: it holds OAuth tokens, a peer token per live session, shell
//! snapshots, telemetry and a `.claude.json` that carries the pid of a running process. None of
//! that has a portable meaning, and all of it has a history of leaking.
//!
//! So the gate is deny-by-default: a path is exported only if a rule says it may be, and a second
//! list refuses paths outright even when they sit inside an allowed directory — because the CLI
//! adds files to its own directories without asking us, and the next release may put a credential
//! where today there is none.

/// Files at least this large are not committed. They are tool results — derived output that a
/// resume does not need — and git would carry every version of them forever.
pub const MAX_EXPORTED_BYTES: u64 = 45 * 1024 * 1024;

/// The directory of projects, where the sessions of each live.
const PROJECTS_DIR: &str = "projects";
/// A session's transcript: its id and this.
const TRANSCRIPT_SUFFIX: &str = ".jsonl";
/// Where the dashes of a session id stand: the CLI names sessions by UUID.
const SESSION_ID_DASHES: [usize; 4] = [8, 13, 18, 23];
/// Characters of a session id.
const SESSION_ID_LENGTH: usize = 36;

/// Whether `text` is a session id: a UUID in lowercase, as the CLI writes it.
fn is_session_id(text: &str) -> bool {
    text.len() == SESSION_ID_LENGTH
        && text.char_indices().all(|(at, character)| {
            if SESSION_ID_DASHES.contains(&at) {
                character == '-'
            } else {
                character.is_ascii_digit() || ('a'..='f').contains(&character)
            }
        })
}

/// Whether `path` — `/`-separated, from the root of the store or of the config directory — is a
/// session's own file: its transcript `projects/<name>/<session>.jsonl` or anything under its side
/// directory `projects/<name>/<session>/`. These are the agent's raw output: what a tool read and
/// what a person pasted lands there, the memory of a project does not.
#[must_use]
pub fn is_session_file(path: &str) -> bool {
    let mut segments = path.split('/');
    let (Some(PROJECTS_DIR), Some(_), Some(third)) =
        (segments.next(), segments.next(), segments.next())
    else {
        return false;
    };
    if segments.next().is_some() {
        is_session_id(third)
    } else {
        third
            .strip_suffix(TRANSCRIPT_SUFFIX)
            .is_some_and(is_session_id)
    }
}

/// How a path is matched. Rules are data rather than code so that the two lists read as lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    /// The first path segment equals this.
    TopLevel(&'static str),
    /// Any path segment equals this — the defence in depth: a `sessions` directory is refused
    /// wherever it appears, not only at the root.
    AnySegment(&'static str),
    /// The file name equals this.
    Name(&'static str),
    /// The file name starts with this.
    NamePrefix(&'static str),
    /// The file name ends with this.
    NameSuffix(&'static str),
}

impl Match {
    fn matches(self, path: &str) -> bool {
        let name = path.rsplit('/').next().unwrap_or_default();
        match self {
            Self::TopLevel(expected) => path.split('/').next() == Some(expected),
            Self::AnySegment(expected) => path.split('/').any(|segment| segment == expected),
            Self::Name(expected) => name == expected,
            Self::NamePrefix(prefix) => name.starts_with(prefix),
            Self::NameSuffix(suffix) => name.ends_with(suffix),
        }
    }
}

/// Why a path is refused, so `doctor` can say more than "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// It carries credentials or a live session's secrets.
    Secret,
    /// It means something only on this machine: pids, snapshots, caches, counters.
    MachineLocal,
    /// Nothing on the allow list covers it. This is the default answer, and it is deliberate:
    /// a file the engine has never heard of is not carried out by accident.
    NotListed,
    /// Derived output too large to keep every version of.
    TooLarge,
}

/// What the engine may do with a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Copy it into the store.
    Export,
    /// Leave it on the machine, with the rule that said so.
    Refuse {
        /// Why.
        refusal: Refusal,
        /// The rule, as text, for the log and for `doctor`.
        rule: &'static str,
    },
}

impl Decision {
    /// Whether the path may leave the machine.
    #[must_use]
    pub fn is_export(self) -> bool {
        matches!(self, Self::Export)
    }
}

/// Paths that never leave the machine, whatever the allow list says.
const REFUSED: &[(Match, Refusal, &str)] = &[
    // Credentials and session secrets.
    (
        Match::Name(".credentials.json"),
        Refusal::Secret,
        "the OAuth tokens of every MCP server",
    ),
    // Top level only, deliberately: the registry lives at the root of the config directory,
    // while `projects/<name>/memory/sessions/` holds a person's saved session hand-offs — real
    // data of theirs, found by the first dry run over a real archive. The peer tokens themselves
    // are `.key` files, and those are refused wherever they appear.
    (
        Match::TopLevel("sessions"),
        Refusal::Secret,
        "the registry of live sessions and their peer tokens",
    ),
    (Match::NameSuffix(".key"), Refusal::Secret, "a session key"),
    (
        Match::AnySegment("session-env"),
        Refusal::Secret,
        "the environment handed to hooks",
    ),
    (
        Match::Name("ant-device-registry.json"),
        Refusal::Secret,
        "the device key",
    ),
    (
        Match::Name("mcp-needs-auth-cache.json"),
        Refusal::Secret,
        "which servers hold credentials",
    ),
    (
        Match::AnySegment("local-agent-mode-sessions"),
        Refusal::Secret,
        "Cowork sessions with their prompts and e-mail",
    ),
    // Files whose content only means something here.
    (
        Match::TopLevel(".claude.json"),
        Refusal::MachineLocal,
        "the config with the pid of a live process",
    ),
    (
        Match::NamePrefix(".claude.json."),
        Refusal::MachineLocal,
        "a backup of that config",
    ),
    (
        Match::AnySegment("backups"),
        Refusal::MachineLocal,
        "backups of that config",
    ),
    (Match::AnySegment("cache"), Refusal::MachineLocal, "a cache"),
    (
        Match::AnySegment("debug"),
        Refusal::MachineLocal,
        "debug output",
    ),
    (
        Match::AnySegment("telemetry"),
        Refusal::MachineLocal,
        "telemetry queued for sending",
    ),
    (
        Match::AnySegment("shell-snapshots"),
        Refusal::MachineLocal,
        "a snapshot of this machine's shell",
    ),
    (
        Match::AnySegment("statsig"),
        Refusal::MachineLocal,
        "feature-flag state",
    ),
    (
        Match::AnySegment("file-history"),
        Refusal::MachineLocal,
        "the local file history",
    ),
    (Match::AnySegment("logs"), Refusal::MachineLocal, "logs"),
    (
        Match::AnySegment("ide"),
        Refusal::MachineLocal,
        "the running editor's handshake",
    ),
    (
        Match::AnySegment("plugins"),
        Refusal::MachineLocal,
        "installed plugins with their local paths",
    ),
    (
        Match::AnySegment("downloads"),
        Refusal::MachineLocal,
        "downloads",
    ),
    // Replicated another way: through the outbox, where one machine writes and the rest read.
    (
        Match::Name("history.jsonl"),
        Refusal::MachineLocal,
        "the prompt history, replicated through the outbox",
    ),
    (
        Match::AnySegment("todos"),
        Refusal::MachineLocal,
        "todo lists, replicated through the outbox",
    ),
    (
        Match::AnySegment("tasks"),
        Refusal::MachineLocal,
        "background tasks, replicated through the outbox",
    ),
    // Counters and verdicts that a second machine must not inherit.
    (
        Match::Name("policy-limits.json"),
        Refusal::MachineLocal,
        "usage limits of this account on this machine",
    ),
    (
        Match::Name("remote-settings.json"),
        Refusal::MachineLocal,
        "settings pushed to this machine",
    ),
    (
        Match::Name("stats-cache.json"),
        Refusal::MachineLocal,
        "a statistics cache",
    ),
    (
        Match::NamePrefix(".last-"),
        Refusal::MachineLocal,
        "a marker of when this machine last did something",
    ),
    (
        Match::Name(".DS_Store"),
        Refusal::MachineLocal,
        "a Finder leftover",
    ),
];

/// The only paths that leave the machine.
const ALLOWED: &[(Match, &str)] = &[
    (
        Match::TopLevel("projects"),
        "transcripts, their side files and the memory of a project",
    ),
    (Match::TopLevel("skills"), "skills, shared between machines"),
    (
        Match::Name("CLAUDE.md"),
        "the instructions that travel with the person",
    ),
    (
        Match::Name("settings.json"),
        "the settings that travel with the person",
    ),
];

/// Decides whether one path of the config directory may be copied into the store.
///
/// `path` is relative to the config directory, `/`-separated. `size` is the file's size; a
/// directory has none.
#[must_use]
pub fn decide(path: &str, size: Option<u64>) -> Decision {
    for (rule, refusal, reason) in REFUSED {
        if rule.matches(path) {
            return Decision::Refuse {
                refusal: *refusal,
                rule: reason,
            };
        }
    }
    let allowed = ALLOWED.iter().any(|(rule, _)| rule.matches(path));
    if !allowed {
        return Decision::Refuse {
            refusal: Refusal::NotListed,
            rule: "nothing on the allow list covers this path",
        };
    }
    if size.is_some_and(|size| size >= MAX_EXPORTED_BYTES) {
        return Decision::Refuse {
            refusal: Refusal::TooLarge,
            rule: "derived output of 45 MiB or more: a resume does not need it",
        };
    }
    Decision::Export
}
