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

use std::collections::BTreeMap;
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
    /// The name `--preset` takes.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Dsh => "dsh",
        }
    }

    /// The preset of a name.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        (name == Self::Dsh.name()).then_some(Self::Dsh)
    }

    /// Where the agent keeps its logs when nothing else is said, under the home directory.
    #[must_use]
    pub fn default_dir(self, home: &Path) -> PathBuf {
        match self {
            Self::Dsh => home.join(".dsh").join("sessions"),
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
/// # Errors
///
/// The registry cannot be written.
pub fn unregister(layout: &Layout, agent: &str) -> Result<bool, String> {
    let mut registry = registry(layout);
    if registry.remove(agent).is_none() {
        return Ok(false);
    }
    write_json(&layout.engine_dir.join(REGISTRY_FILE), &registry)?;
    let _ = std::fs::remove_file(state_path(layout, agent, "json"));
    Ok(true)
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

/// Now, seconds since the epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}
