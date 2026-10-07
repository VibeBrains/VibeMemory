//! Agents without hooks of their own: `session agent add` registers one with its log directory and
//! its wrapper, the tick looks at the directory every run and hands each log written since the last
//! look to the wrapper, which turns it into the format of `session put` and calls it.
//!
//! The tick is the one schedule the engine already keeps on every system; a second resident process
//! would be one more thing to install and one more thing to die without a witness. What decides is
//! in `vibememory_core::agent_watch`; here are the files and the wrapper's process.
//!
//! An agent the engine knows by a preset — `DeepSeek` Harness, `--preset dsh` — needs no wrapper:
//! the tick reads its log itself (`vibememory_core::dsh`) and places the session as `session put`
//! would.
//!
//! Everything lives on this machine: the directory and the wrapper are this machine's, and a team
//! store has no business knowing where an agent keeps its logs here.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use vibememory_core::naming::slug::is_slug;
use vibememory_core::{agent_watch, dsh};

use crate::config::Config;
use crate::install::Layout;

/// The registry, in the engine's directory: agent, log directory, wrapper.
pub const REGISTRY_FILE: &str = "agents.json";

/// The directory of what is known about each agent: `<agent>.put` when its last `session put`
/// went through, `<agent>.json` what the tick last saw and what the wrapper last said.
pub const STATE_DIR: &str = "agents";

/// How long one run of a wrapper may take before it is stopped: the tick runs every two minutes,
/// and a wrapper that hangs must not hold every other store's sync behind it.
pub const WRAPPER_TIMEOUT: Duration = Duration::from_mins(1);

/// How many logs of one agent a tick hands over at most: the rest wait for the next run, so a day
/// of logs after a week offline does not hold the tick for an hour.
pub const LOGS_PER_TICK: usize = 20;

/// How deep the walk goes below the log directory: deep enough for every layout seen so far (DSH
/// keeps a log three levels down), shallow enough not to wander a whole home directory.
const MAX_DEPTH: usize = 6;

/// How many last lines of a failed wrapper's output are kept: what it said last is what explains
/// the failure.
const FAILURE_LINES: usize = 5;

/// The environment variable the wrapper finds the engine's binary in: the tick runs under the
/// system's scheduler, where `PATH` is not the one of the person's terminal.
pub const BINARY_VAR: &str = "VIBEMEMORY_BIN";

/// The environment variable that names the agent the wrapper is run for.
pub const AGENT_VAR: &str = "VIBEMEMORY_AGENT";

/// An agent whose log the engine reads itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// `DeepSeek` Harness: `~/.dsh/sessions/<directory>/<session>/session.v4.jsonl.zstd`.
    Dsh,
}

impl Preset {
    /// Every preset the engine knows, in the order `doctor` lists them.
    pub const ALL: &[Self] = &[Self::Dsh];

    /// The name `--preset` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dsh => "dsh",
        }
    }

    /// The agent name this preset is registered under: what `session agent add` is told, what a
    /// token for it is taken under, and what `mcp-config` prints as the name to sign with.
    #[must_use]
    pub const fn agent(self) -> &'static str {
        match self {
            Self::Dsh => "dsh-desktop",
        }
    }

    /// What the agent is called in a sentence for a person.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dsh => "DeepSeek Harness",
        }
    }

    /// The preset of a name.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        (name == Self::Dsh.name()).then_some(Self::Dsh)
    }

    /// The agent's own directory under the home directory: its presence says the agent lives on
    /// this machine.
    #[must_use]
    pub fn home(self, home: &Path) -> PathBuf {
        match self {
            Self::Dsh => home.join(".dsh"),
        }
    }

    /// The file of instructions the agent reads in every project, named inside [`Preset::home`].
    ///
    /// The engine keeps it the same as the shared `CLAUDE.md`, so an agent other than Claude works
    /// by the same rules. DSH reads `$DSH_HOME/AGENTS.md` (read off its bundle on 2026-10-07).
    #[must_use]
    pub const fn instructions_name(self) -> &'static str {
        match self {
            Self::Dsh => "AGENTS.md",
        }
    }

    /// Where the agent keeps its logs when nothing else is said, under the home directory.
    #[must_use]
    pub fn default_dir(self, home: &Path) -> PathBuf {
        match self {
            Self::Dsh => self.home(home).join("sessions"),
        }
    }

    /// Whether a file under the log directory is a log: everything else beside it — a lock, a
    /// temporary file — is neither handed over nor taken as a sign of life.
    #[must_use]
    pub fn is_log(self, path: &Path) -> bool {
        match self {
            Self::Dsh => path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(vibememory_core::dsh::is_log_name),
        }
    }
}

/// Who turns a log into a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handler<'a> {
    /// The agent's own wrapper, run with the path of one log.
    Run(&'a Path),
    /// The engine itself.
    Preset(Preset),
}

/// One registered agent: with a wrapper or with a preset, never both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    /// The directory the agent writes its logs into, absolute.
    pub dir: String,
    /// The wrapper, absolute: run with the path of one log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The preset the engine reads the log by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
    /// When it was registered, seconds since the epoch: logs older than this are history.
    pub since: u64,
}

/// What a wrapper said when it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    /// When, seconds since the epoch.
    pub at: u64,
    /// The log it was run for.
    pub log: String,
    /// What happened: its exit code, or why it has none.
    pub outcome: String,
    /// The last lines of what it wrote.
    pub lines: Vec<String>,
}

/// What the tick knows of one agent between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Watch {
    /// The logs and their modification times at the last look.
    seen: BTreeMap<String, u64>,
    /// When the tick last looked.
    last_look: Option<u64>,
    /// The last failure of the wrapper; cleared by its next success.
    failure: Option<Failure>,
    /// Logs a preset could not hand over, each for a reason of its own, by path: tried again when
    /// they are written to, and said by `doctor` until then.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    skipped: BTreeMap<String, Failure>,
}

/// Why a log was not delivered.
enum Missed {
    /// Whatever stopped this log stops the agent's others too: the wrapper failing, a format this
    /// engine does not know. The log stays new, and the rest wait behind it.
    Agent(Failure),
    /// Only this log: its project directory is gone, it is damaged. The others go on, and this one
    /// is set aside until it changes — tried every run, it would hold the agent's history forever.
    Log(Failure),
}

/// Registers an agent: its logs from now on go to its wrapper or are read by its preset, one log at
/// a time. With `backfill` the logs already there are handed over too, a bounded number per tick.
///
/// # Errors
///
/// A sentence for the person: a name that is not a slug, a directory or a wrapper that is not
/// there, or a file that cannot be written.
pub fn register(
    layout: &Layout,
    agent: &str,
    dir: &Path,
    handler: Handler<'_>,
    backfill: bool,
) -> Result<Registration, String> {
    if !is_slug(agent) {
        return Err(format!(
            "agent {:?} is not a name: lowercase latin letters, digits and '-', not at either end, \
             at most 63",
            vibememory_core::terminal::printable(agent)
        ));
    }
    let dir = std::fs::canonicalize(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let (run, preset) = match handler {
        Handler::Run(run) => {
            let run = std::fs::canonicalize(run)
                .map_err(|error| format!("{}: {error}", run.display()))?;
            if !run.is_file() {
                return Err(format!("{} is not a file", run.display()));
            }
            (Some(run.to_string_lossy().into_owned()), None)
        }
        Handler::Preset(preset) => (None, Some(preset)),
    };
    let registration = Registration {
        dir: dir.to_string_lossy().into_owned(),
        run,
        preset,
        since: now(),
    };
    let mut registry = registry(layout);
    registry.insert(agent.to_owned(), registration.clone());
    write_json(&layout.engine_dir.join(REGISTRY_FILE), &registry)?;
    // Registering over a refusal is the later word, and the note about a decision taken back
    // would outlive it.
    allow(layout, agent);
    // The directory as it is now is the baseline: what was written before registration is history,
    // and handing all of it to the wrapper in the first tick would hold the tick for as long as the
    // history is long. Asked for the history, the baseline is empty, and the tick takes it in
    // bounded portions.
    let watch = Watch {
        seen: if backfill {
            BTreeMap::new()
        } else {
            logs(&dir, preset)
        },
        last_look: Some(registration.since),
        failure: None,
        skipped: BTreeMap::new(),
    };
    write_json(&state_path(layout, agent, "json"), &watch)?;
    Ok(registration)
}

/// Forgets an agent. Answers whether it was registered.
///
/// The removal is a decision, and it is written down as one: the logs stay where they are, so
/// without the note `doctor` would offer the registration again the moment it is removed — the
/// person would be argued with after every answer. `add` takes the note back.
///
/// # Errors
///
/// The registry cannot be written.
pub fn unregister(layout: &Layout, agent: &str) -> Result<bool, String> {
    let mut registry = registry(layout);
    if registry.remove(agent).is_none() {
        return Ok(false);
    }
    write_json(&layout.engine_dir.join(REGISTRY_FILE), &registry)?;
    // The watch state of an agent nobody watches is a note about nothing: it would outlive the
    // registration and be read as if it were current.
    let _ = std::fs::remove_file(state_path(layout, agent, "json"));
    let _ = std::fs::remove_file(state_path(layout, agent, "put"));
    let _ = decline(layout, agent);
    Ok(true)
}

/// An agent a person decided not to watch: its sessions stay on this machine, and the engine stops
/// offering the registration. Not the same state as "nobody has decided yet" — one is an answer,
/// the other is a question, and only one of them is worth repeating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declined {
    /// The name it would have been registered under.
    pub agent: String,
    /// The preset that would have read its logs.
    pub preset: Preset,
    /// The directory its logs are in.
    pub dir: String,
    /// The logs lying there now: the sessions the decision keeps on this machine.
    pub logs: usize,
    /// When the decision was made, seconds since the epoch.
    pub at: u64,
}

/// Writes down that this agent's sessions are to stay on this machine.
///
/// # Errors
///
/// The note cannot be written.
pub fn decline(layout: &Layout, agent: &str) -> Result<(), String> {
    let path = state_path(layout, agent, "declined");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(&path, format!("{}\n", now()))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Takes the decision back: what `add` does, because registering over a refusal is the later word.
fn allow(layout: &Layout, agent: &str) {
    let _ = std::fs::remove_file(state_path(layout, agent, "declined"));
}

/// The agents a person decided not to watch, with what waits behind the decision. Empty for an
/// agent that was never offered: a question nobody answered is not a decision.
#[must_use]
pub fn declined(layout: &Layout) -> Vec<Declined> {
    let Some(home) = crate::install::home_dir() else {
        return Vec::new();
    };
    declined_under(layout, &home)
}

/// The same with the home named, as [`waiting_under`] is to [`waiting`].
#[must_use]
pub fn declined_under(layout: &Layout, home: &Path) -> Vec<Declined> {
    Preset::ALL
        .iter()
        .copied()
        .filter_map(|preset| {
            let agent = preset.agent();
            let at = decision_at(layout, agent)?;
            let dir = preset.default_dir(home);
            let logs = logs(&dir, Some(preset));
            Some(Declined {
                agent: agent.to_owned(),
                preset,
                dir: dir.display().to_string(),
                logs: logs.len(),
                at,
            })
        })
        .collect()
}

impl Declined {
    /// The line `doctor` prints for a decision already made: the state is named, so that a person
    /// who forgot the answer is not left guessing why the sessions of an agent stay here.
    #[must_use]
    pub fn line(&self) -> String {
        let command = format!(
            "vibememory session agent add --agent {} --preset {} --backfill",
            vibememory_core::terminal::printable(&self.agent),
            self.preset.name()
        );
        let dir = vibememory_core::terminal::printable(&self.dir);
        let since = crate::clock::iso8601(i64::try_from(self.at).unwrap_or(i64::MAX));
        if self.logs == 0 {
            format!(
                "unwatched {}: sessions in {dir} stay on this machine by your decision of {since} \
                 \u{2014} {command} changes it",
                self.preset.title()
            )
        } else {
            format!(
                "unwatched {}: {} session(s) in {dir} stay on this machine by your decision of \
                 {since} \u{2014} {command} changes it",
                self.preset.title(),
                self.logs
            )
        }
    }
}

/// When the decision about this agent was written, seconds since the epoch.
fn decision_at(layout: &Layout, agent: &str) -> Option<u64> {
    let text = std::fs::read_to_string(state_path(layout, agent, "declined")).ok()?;
    text.trim().parse().ok()
}

/// How long a session may go without being told about a waiting agent: the sentence is worth
/// saying, and worth saying again tomorrow — but not at the start of every session of the day.
pub const NOTICE_AFTER: u64 = 24 * 60 * 60;

/// What to tell a session about an agent nobody watched, or `None` when there is nothing new to
/// say. The person is in the session and does not read the machine's report; the session is where
/// they can be told, once a day and again when the logs grow — a count that changed is news.
#[must_use]
pub fn notice_waiting(layout: &Layout, at: u64) -> Option<String> {
    let home = crate::install::home_dir()?;
    notice_waiting_under(layout, &home, at)
}

/// The same with the home named, as [`waiting_under`] is to [`waiting`].
#[must_use]
pub fn notice_waiting_under(layout: &Layout, home: &Path, at: u64) -> Option<String> {
    let waiting: Vec<Waiting> = waiting_under(layout, home)
        .into_iter()
        .filter(|waiting| waiting.logs > 0)
        .collect();
    let mut notices = Vec::new();
    for agent in &waiting {
        let said = notice_at(layout, &agent.agent);
        if said.is_some_and(|(when, logs)| {
            at.saturating_sub(when) < NOTICE_AFTER && logs == agent.logs
        }) {
            continue;
        }
        // A note that cannot be written would make the sentence repeat at every session start:
        // worse than silence until the disk is fixed.
        if remember_notice(layout, &agent.agent, at, agent.logs).is_err() {
            continue;
        }
        notices.push(format!(
            "VibeMemory: {title} keeps {logs} session(s) in {dir} that are not going to history, \
             because nobody watches this agent here \u{2014} `vibememory session agent add --agent \
             {agent} --preset {preset} --backfill` sends them, `vibememory session agent decline \
             --agent {agent}` leaves them on this machine for good.",
            title = agent.preset.title(),
            logs = agent.logs,
            dir = vibememory_core::terminal::printable(&agent.dir),
            agent = vibememory_core::terminal::printable(&agent.agent),
            preset = agent.preset.name(),
        ));
    }
    (!notices.is_empty()).then(|| notices.join(" "))
}

/// When the session was last told about this agent, and how many logs waited then.
fn notice_at(layout: &Layout, agent: &str) -> Option<(u64, usize)> {
    let text = std::fs::read_to_string(state_path(layout, agent, "noticed")).ok()?;
    let mut parts = text.split_whitespace();
    let when = parts.next()?.parse().ok()?;
    let logs = parts.next()?.parse().ok()?;
    Some((when, logs))
}

fn remember_notice(layout: &Layout, agent: &str, at: u64, logs: usize) -> Result<(), String> {
    let path = state_path(layout, agent, "noticed");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(&path, format!("{at} {logs}\n"))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Every registered agent, by name. A registry that cannot be read is an empty one: the tick goes on
/// with the stores, and `doctor` says what is wrong with the file.
#[must_use]
pub fn registry(layout: &Layout) -> BTreeMap<String, Registration> {
    read_registry(layout).unwrap_or_default()
}

/// What is wrong with the registry file, if it is there and cannot be read: every agent in it has
/// stopped being watched, and nothing but `doctor` would say so.
#[must_use]
pub fn registry_problem(layout: &Layout) -> Option<String> {
    read_registry(layout).err()
}

fn read_registry(layout: &Layout) -> Result<BTreeMap<String, Registration>, String> {
    let path = layout.engine_dir.join(REGISTRY_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Notes that a `session put` of `agent` went through: the one moment that proves the wrapper
/// works, whether or not the session had changed.
///
/// # Errors
///
/// The note cannot be written.
pub fn record_put(layout: &Layout, agent: &str) -> Result<(), String> {
    let path = state_path(layout, agent, "put");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{}: {error}", parent.display()))?;
    }
    std::fs::write(&path, format!("{}\n", now()))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// When the last `session put` of `agent` went through here, seconds since the epoch.
#[must_use]
pub fn last_put(layout: &Layout, agent: &str) -> Option<u64> {
    std::fs::read_to_string(state_path(layout, agent, "put"))
        .ok()
        .and_then(|text| text.trim().parse().ok())
}

/// What one tick did for one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// The agent.
    pub agent: String,
    /// Logs the wrapper took.
    pub handed: usize,
    /// Logs left for the next run.
    pub waiting: usize,
    /// The wrapper's failure this run, if it failed.
    pub failure: Option<Failure>,
    /// Logs set aside this run, each for a reason of its own.
    pub skipped: Vec<Failure>,
    /// The log directory is not there.
    pub missing: bool,
}

/// One look at every registered agent: each log written since the last look goes to its wrapper,
/// oldest first. A log the wrapper failed on stays new and is tried again next run, and the agent's
/// other logs wait behind it: they are the same wrapper, and it would fail on them for the same
/// reason.
#[must_use]
pub fn tick(layout: &Layout, config: &Config) -> Vec<Ran> {
    let binary = std::env::current_exe().ok();
    registry(layout)
        .into_iter()
        .map(|(agent, registration)| look(layout, config, &agent, &registration, binary.as_deref()))
        .collect()
}

/// One look at one agent.
fn look(
    layout: &Layout,
    config: &Config,
    agent: &str,
    registration: &Registration,
    binary: Option<&Path>,
) -> Ran {
    let dir = Path::new(&registration.dir);
    if !dir.is_dir() {
        return Ran {
            agent: agent.to_owned(),
            handed: 0,
            waiting: 0,
            failure: None,
            skipped: Vec::new(),
            missing: true,
        };
    }
    let mut watch = read_watch(layout, agent);
    let found = logs(dir, registration.preset);
    let fresh = agent_watch::changed(&watch.seen, &found);
    // Logs that are gone leave the list: it only ever holds what the directory holds.
    watch.seen.retain(|name, _| found.contains_key(name));
    watch.skipped.retain(|name, _| found.contains_key(name));
    let mut handed = 0;
    let mut failure = None;
    let mut skipped = Vec::new();
    for name in fresh.iter().take(LOGS_PER_TICK) {
        let delivered = match (registration.preset, &registration.run) {
            (Some(preset), _) => read_log(layout, config, agent, preset, name),
            (None, Some(run)) => run_wrapper(run, agent, name, binary).map_err(Missed::Agent),
            (None, None) => Err(Missed::Agent(Failure {
                at: now(),
                log: name.clone(),
                outcome: "the registration names neither a wrapper nor a preset".to_owned(),
                lines: Vec::new(),
            })),
        };
        let modified = found.get(name).copied().unwrap_or_default();
        match delivered {
            Ok(()) => {
                watch.seen.insert(name.clone(), modified);
                watch.skipped.remove(name);
                handed += 1;
            }
            Err(Missed::Log(missed)) => {
                watch.seen.insert(name.clone(), modified);
                watch.skipped.insert(name.clone(), missed.clone());
                skipped.push(missed);
            }
            Err(Missed::Agent(failed)) => {
                failure = Some(failed);
                break;
            }
        }
    }
    // This run's word is the last word: a failure stands until a run delivers, and a run with
    // nothing to deliver has nothing failing — a log that was failed on and is gone clears it.
    watch.failure.clone_from(&failure);
    watch.last_look = Some(now());
    // Losing the note costs one more run of the wrapper on the same log, which a wrapper of
    // `session put` survives: an unchanged file makes no commit.
    let _ = write_json(&state_path(layout, agent, "json"), &watch);
    Ran {
        agent: agent.to_owned(),
        waiting: fresh.len() - handed - skipped.len(),
        handed,
        failure,
        skipped,
        missing: false,
    }
}

/// Reads one log by its preset and places the session. A log with nothing in it yet is no failure:
/// it is taken as seen, and its next write makes it new again.
fn read_log(
    layout: &Layout,
    config: &Config,
    agent: &str,
    preset: Preset,
    log: &str,
) -> Result<(), Missed> {
    let failure = |outcome: String| Failure {
        at: now(),
        log: log.to_owned(),
        outcome,
        lines: Vec::new(),
    };
    let raw = std::fs::read(log)
        .map_err(|error| Missed::Agent(failure(format!("could not be read: {error}"))))?;
    let session = match preset {
        Preset::Dsh => match vibememory_core::dsh::convert(&raw) {
            Ok(session) => session,
            Err(dsh::Problem::Empty) => return Ok(()),
            Err(problem @ dsh::Problem::UnknownVersion(_)) => {
                return Err(Missed::Agent(failure(describe_dsh(&problem))));
            }
            Err(problem) => return Err(Missed::Log(failure(describe_dsh(&problem)))),
        },
    };
    crate::foreign_session::hand_over(
        layout,
        config,
        &crate::foreign_session::Handed {
            agent,
            session: &session.id,
            cwd: &session.cwd,
            origin: log,
            bytes: session.file.as_bytes(),
            ended: false,
        },
    )
    .map(|_| ())
    .map_err(|error| Missed::Log(failure(error)))
}

/// Why a log of DSH gives no session, in words.
fn describe_dsh(problem: &dsh::Problem) -> String {
    match problem {
        dsh::Problem::Empty => "holds nothing yet".to_owned(),
        dsh::Problem::NotText => "is not UTF-8 text once decompressed".to_owned(),
        dsh::Problem::NoHeader => {
            "does not begin with a session header holding its id and directory".to_owned()
        }
        dsh::Problem::UnknownVersion(version) => format!(
            "the DSH log format v{version} is unknown to this engine, which reads v{}: \
             vibememory update",
            dsh::LOG_VERSION
        ),
        dsh::Problem::Record { line } => format!("line {line} is not JSON: the log is damaged"),
    }
}

/// Runs the wrapper for one log, stopping it after [`WRAPPER_TIMEOUT`].
fn run_wrapper(run: &str, agent: &str, log: &str, binary: Option<&Path>) -> Result<(), Failure> {
    let failure = |outcome: String, lines: Vec<String>| Failure {
        at: now(),
        log: log.to_owned(),
        outcome,
        lines,
    };
    let mut command = Command::new(run);
    command
        .arg(log)
        .env(AGENT_VAR, agent)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(binary) = binary {
        command.env(BINARY_VAR, binary);
    }
    let mut child = command
        .spawn()
        .map_err(|error| failure(format!("did not start: {error}"), Vec::new()))?;
    // Read both pipes while the wrapper runs: one that fills its pipe would otherwise wait for a
    // reader forever, and look like a hang.
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= WRAPPER_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                return Err(failure(
                    format!("could not be waited for: {error}"),
                    Vec::new(),
                ));
            }
        }
    };
    let said = |reader: Option<std::thread::JoinHandle<Vec<u8>>>| {
        reader
            .and_then(|handle| handle.join().ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()
    };
    let (stdout, stderr) = (said(out), said(err));
    match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(failure(
            status.code().map_or_else(
                || "stopped by a signal".to_owned(),
                |code| format!("exit code {code}"),
            ),
            last_lines(if stderr.trim().is_empty() {
                &stdout
            } else {
                &stderr
            }),
        )),
        None => Err(failure(
            format!("stopped after {} seconds", WRAPPER_TIMEOUT.as_secs()),
            last_lines(if stderr.trim().is_empty() {
                &stdout
            } else {
                &stderr
            }),
        )),
    }
}

/// Reads a pipe to its end on a thread of its own.
fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        bytes
    })
}

/// The last non-empty lines of an output.
fn last_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    lines
        .iter()
        .skip(lines.len().saturating_sub(FAILURE_LINES))
        .map(|line| vibememory_core::terminal::printable(line))
        .collect()
}

impl Watched {
    /// Whether the agent's integration is broken: `doctor` fails on it.
    #[must_use]
    pub const fn is_wrong(&self) -> bool {
        self.missing || self.silent || self.failure.is_some()
    }
}

/// What `doctor` and `status` say about one registered agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watched {
    /// The agent.
    pub agent: String,
    /// Its registration.
    pub registration: Registration,
    /// When its last `session put` went through here.
    pub last_put: Option<u64>,
    /// When the tick last looked.
    pub last_look: Option<u64>,
    /// The wrapper's last failure, if its last run failed.
    pub failure: Option<Failure>,
    /// Logs a preset set aside, each for a reason of its own: said, but no fault of the
    /// integration — the others go on.
    pub skipped: Vec<Failure>,
    /// When the newest log in its directory was written.
    pub newest_log: Option<u64>,
    /// Logs keep coming and sessions do not.
    pub silent: bool,
    /// The log directory is not there.
    pub missing: bool,
}

/// Every registered agent, as `doctor` reports it.
#[must_use]
pub fn watched(layout: &Layout) -> Vec<Watched> {
    registry(layout)
        .into_iter()
        .map(|(agent, registration)| {
            let dir = Path::new(&registration.dir);
            let missing = !dir.is_dir();
            let newest_log = logs(dir, registration.preset).into_values().max();
            let last_put = last_put(layout, &agent);
            let watch = read_watch(layout, &agent);
            let silent = newest_log
                .is_some_and(|newest| agent_watch::is_silent(newest, last_put, registration.since));
            Watched {
                agent,
                last_put,
                last_look: watch.last_look,
                failure: watch.failure,
                skipped: watch.skipped.into_values().collect(),
                newest_log,
                silent,
                missing,
                registration,
            }
        })
        .collect()
}

/// The preset that reads this agent's logs, if the engine knows how to read them at all.
#[must_use]
pub fn preset_for(agent: &str) -> Option<Preset> {
    Preset::ALL
        .iter()
        .copied()
        .find(|preset| preset.agent() == agent)
}

/// An agent whose logs the engine knows how to read and which nobody registered: until a person
/// says so, its sessions go nowhere. Not a fault — handing sessions over is a decision, and the
/// registry is where the decision is written down — but silence about it looks exactly like an
/// agent that has nothing to hand over. That is how three days of sessions stayed out of history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    /// The name it would be registered under.
    pub agent: String,
    /// The preset that would read its logs.
    pub preset: Preset,
    /// The directory its logs are in.
    pub dir: String,
    /// The logs lying there: what `--backfill` would hand over.
    pub logs: usize,
    /// When the newest of them was written, seconds since the epoch.
    pub newest: Option<u64>,
}

impl Waiting {
    /// The line `doctor`, `status`, `install` and `update` print for it, in one wording: the same
    /// state must be recognized wherever a person meets it. Never a failure — handing sessions
    /// over is the owner's decision — so the line carries the count and the command instead.
    #[must_use]
    pub fn line(&self) -> String {
        let command = format!(
            "vibememory session agent add --agent {} --preset {} --backfill",
            vibememory_core::terminal::printable(&self.agent),
            self.preset.name()
        );
        let dir = vibememory_core::terminal::printable(&self.dir);
        if self.logs == 0 {
            format!(
                "waiting  {} keeps sessions in {dir} and they do not go to history \u{2014} nobody \
                 watched this agent: {command}",
                self.preset.title()
            )
        } else {
            format!(
                "waiting  {}: {} session(s) in {dir} are not in history \u{2014} nobody watched \
                 this agent: {command}",
                self.preset.title(),
                self.logs
            )
        }
    }
}

/// The known agents whose logs are on this machine and which no one registered. The registry is the
/// switch, and this is the report of a switch nobody has thrown.
#[must_use]
pub fn waiting(layout: &Layout) -> Vec<Waiting> {
    let Some(home) = crate::install::home_dir() else {
        return Vec::new();
    };
    waiting_under(layout, &home)
}

/// The same with the home named: a preset finds its log directory from it, and a machine that is
/// not this one — or a test — says which home it means.
#[must_use]
pub fn waiting_under(layout: &Layout, home: &Path) -> Vec<Waiting> {
    let watched = registry(layout);
    let declined: BTreeSet<String> = declined(layout).into_iter().map(|one| one.agent).collect();
    Preset::ALL
        .iter()
        .copied()
        .filter(|preset| {
            !watched
                .values()
                .any(|registration| registration.preset == Some(*preset))
                // A question already answered is not asked again: `decline` and `remove` write
                // the answer down, and `add` takes it back.
                && !declined.contains(preset.agent())
        })
        .filter_map(|preset| {
            let dir = preset.default_dir(home);
            dir.is_dir().then(|| {
                let logs = logs(&dir, Some(preset));
                Waiting {
                    agent: preset.agent().to_owned(),
                    preset,
                    dir: dir.display().to_string(),
                    logs: logs.len(),
                    newest: logs.values().copied().max(),
                }
            })
        })
        .collect()
}

/// The line for an agent whose sessions never reach a store and whose logs the engine cannot read:
/// every other line of `doctor` looks the same whether its sessions are coming or not, and a person
/// waiting for them would wait forever.
const NO_SESSIONS_NOTE: &str = "sessions: none in any store \u{2014} only memory comes from this \
                                client; the engine reads the logs of a watched agent or a known \
                                preset";

/// What this machine knows of which agents hand sessions over: whose sessions lie in the stores,
/// which agents are watched here, and whether the engine runs here at all. The engine reads the
/// logs of a watched agent and of a known preset, and of nobody else — every other client hands
/// over memory alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Readings {
    /// Agents whose sessions are in some store.
    delivered: BTreeSet<String>,
    /// Agents registered here: their logs the engine reads.
    watched: BTreeSet<String>,
    /// Whether the engine is installed here: without it the whole machine hands nothing over, and
    /// saying so under every agent would drown the one line that matters.
    engine: bool,
}

impl Readings {
    /// The state of this machine.
    #[must_use]
    pub fn of(layout: &Layout) -> Self {
        Self::with_delivered(layout, delivered_agents(layout))
    }

    /// The same when the caller already knows whose sessions are in the stores: `doctor` prints
    /// that list, and walking every store twice to say one thing is work for nothing.
    #[must_use]
    pub fn with_delivered(layout: &Layout, delivered: BTreeSet<String>) -> Self {
        Self {
            delivered,
            watched: registry(layout).into_keys().collect(),
            engine: crate::install::engine_configured(layout),
        }
    }

    /// The line for an agent whose sessions never reach a store, when it is one the engine cannot
    /// read. Not a fault — an agent that only writes memory is the ordinary way to connect one —
    /// but a state nobody names is a state nobody can fix.
    #[must_use]
    pub fn note(&self, agent: &str) -> Option<&'static str> {
        if !self.engine
            || self.delivered.contains(agent)
            || self.watched.contains(agent)
            // A known preset waits for its registration, and the `waiting` line says that with the
            // count and the command; the hooks agent hands its sessions over as transcripts.
            || preset_for(agent).is_some()
            || hooks_agent(agent)
        {
            return None;
        }
        Some(NO_SESSIONS_NOTE)
    }
}

/// The agents whose sessions are in some store of this machine — the personal one and every
/// connected team's. What `doctor` prints and what [`Readings`] decides the notes by.
#[must_use]
pub fn delivered_agents(layout: &Layout) -> BTreeSet<String> {
    let mut delivered = BTreeSet::new();
    for clone in std::iter::once(layout.store()).chain(
        crate::team_connect::connected_teams(layout)
            .into_iter()
            .map(|team| layout.team_store(&team)),
    ) {
        for (agent, _) in crate::foreign_session::delivered(&clone) {
            delivered.insert(agent);
        }
    }
    delivered
}

/// Whether the agent's sessions reach the store without being handed over: Claude Code writes the
/// transcripts the engine syncs, and Claude Desktop's sessions come through the outbox.
fn hooks_agent(agent: &str) -> bool {
    [
        crate::mcp_config::Client::ClaudeCode,
        crate::mcp_config::Client::ClaudeDesktop,
    ]
    .iter()
    .any(|client| client.agent() == agent)
}

/// The logs under a log directory and their modification times, by path: every file for an agent
/// with a wrapper, only what its preset calls a log otherwise. Links are not followed: a link out
/// of the directory would make the walk someone else's tree.
fn logs(dir: &Path, preset: Option<Preset>) -> BTreeMap<String, u64> {
    let mut found = BTreeMap::new();
    walk(dir, 0, preset, &mut found);
    found
}

fn walk(dir: &Path, depth: usize, preset: Option<Preset>, found: &mut BTreeMap<String, u64>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk(&path, depth + 1, preset, found);
        } else if kind.is_file() && preset.is_none_or(|preset| preset.is_log(&path)) {
            let modified = entry
                .metadata()
                .and_then(|data| data.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());
            found.insert(path.to_string_lossy().into_owned(), modified);
        }
    }
}

fn read_watch(layout: &Layout, agent: &str) -> Watch {
    std::fs::read_to_string(state_path(layout, agent, "json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn state_path(layout: &Layout, agent: &str, extension: &str) -> PathBuf {
    layout
        .engine_dir
        .join(STATE_DIR)
        .join(format!("{agent}.{extension}"))
}

/// Writes a JSON file whole or not at all: a half-written registry would lose every agent at once.
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, format!("{text}\n"))
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}

/// Now, seconds since the epoch: the clock every note of the tick is stamped with, and the one the
/// rules take as an argument so that they can be tested without a clock.
#[must_use]
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
