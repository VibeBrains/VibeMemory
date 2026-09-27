//! Asking the operating system about processes: whether one is there, which one it is, and which
//! process of the agent a hook runs under.
//!
//! A process id alone is not an identity: the system hands a freed id to the next process. The id
//! together with the moment the process started is, so a session is marked by both.

use serde::{Deserialize, Serialize};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// How many parents a hook looks through for the agent's process: the hook runs under a shell or
/// two, never under a long chain.
const ANCESTORS: usize = 8;

/// Names of the agent's executable, without `.exe`.
const AGENT_NAMES: &[&str] = &["claude"];

/// Interpreters the agent may run under when it is installed as a package: the process is `node`
/// or `bun`, and the agent is named in its arguments.
const AGENT_HOSTS: &[&str] = &["node", "bun"];

/// A process as it is known for good: its id and the moment it started, in the operating system's
/// own words. Compared as text; nothing reads a date out of it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMark {
    /// The process id.
    pub pid: u32,
    /// When the process started, as the system reports it.
    pub started: String,
}

impl ProcessMark {
    /// Whether this very process is still running: the id is there and started at the same moment.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        started(self.pid).is_some_and(|started| started == self.started)
    }
}

/// Whether a process with this id exists.
#[must_use]
pub fn is_running(pid: u32) -> bool {
    started(pid).is_some()
}

/// The agent's process this hook runs under: the nearest ancestor that is the agent's executable,
/// or an interpreter running it. `None` when there is none, as for a hook run by hand.
#[must_use]
pub fn agent_process() -> Option<ProcessMark> {
    let system = snapshot(ProcessesToUpdate::All);
    let mut pid = system
        .process(Pid::from_u32(std::process::id()))?
        .parent()?;
    for _ in 0..ANCESTORS {
        let process = system.process(pid)?;
        if is_agent(process) {
            return Some(ProcessMark {
                pid: pid.as_u32(),
                started: process.start_time().to_string(),
            });
        }
        pid = process.parent()?;
    }
    None
}

/// The mark of a running process, `None` when there is no process with this id.
#[must_use]
pub fn mark_of(pid: u32) -> Option<ProcessMark> {
    Some(ProcessMark {
        pid,
        started: started(pid)?,
    })
}

/// When the process started, in seconds since the epoch, as text.
fn started(pid: u32) -> Option<String> {
    let pid = Pid::from_u32(pid);
    snapshot(ProcessesToUpdate::Some(&[pid]))
        .process(pid)
        .map(|process| process.start_time().to_string())
}

/// The processes asked for, with their parent, name, arguments and start time.
fn snapshot(which: ProcessesToUpdate<'_>) -> System {
    let mut system = System::new();
    system.refresh_processes_specifics(
        which,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    system
}

/// The executable's name without `.exe`, lower case.
fn base_name(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .map_or_else(|| lower.clone(), str::to_owned)
}

fn is_agent(process: &sysinfo::Process) -> bool {
    let name = base_name(&process.name().to_string_lossy());
    if AGENT_NAMES.contains(&name.as_str()) {
        return true;
    }
    AGENT_HOSTS.contains(&name.as_str())
        && process.cmd().iter().skip(1).any(|argument| {
            let argument = argument.to_string_lossy();
            AGENT_NAMES.iter().any(|agent| argument.contains(agent))
        })
}
