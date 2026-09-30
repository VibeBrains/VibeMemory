//! What `doctor` knows about this machine's credentials: the tokens `connect` kept, whether only
//! their owner can reach them, and whether curl is there to get new ones.
//!
//! A section of its own, apart from the engine: a machine of a `memory` team has tokens and no
//! engine at all, and `doctor` there must still be able to say that everything is in order.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::connect::{FRAGMENT_SUFFIX, SIDECAR_EXTENSION, TOKENS_DIR, fragment_file};
use crate::install::Layout;

/// A token as its sidecar describes it, with what is wrong about its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptToken {
    /// The team.
    pub team: String,
    /// The agent.
    pub agent: String,
    /// Its public id.
    pub token_id: String,
    /// The cabinet it came from: where to go when it stops working.
    pub cabinet: String,
    /// The token file.
    pub file: PathBuf,
    /// What is wrong, if anything: a file missing, or reachable by others than the owner.
    pub problems: Vec<String>,
    /// When a memory server last started on this machine as this agent. `None` means no client
    /// ever called it by this name here: a token with nobody to use it.
    pub client_started: Option<String>,
}

/// Where the memory server notes each agent name it is started as: one file per name, holding when.
pub const CLIENTS_DIR: &str = "clients";

/// Notes that a memory server started as `agent`. The agent name is a slug, checked by the server
/// before it gets here, so it is a safe file name.
///
/// # Errors
///
/// The text of what went wrong.
pub fn record_client(engine_dir: &Path, agent: &str, stamp: &str) -> Result<(), String> {
    let dir = engine_dir.join(CLIENTS_DIR);
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let path = dir.join(agent);
    std::fs::write(&path, format!("{stamp}\n"))
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// When a memory server last started as `agent`, if ever.
#[must_use]
pub fn client_started(engine_dir: &Path, agent: &str) -> Option<String> {
    let text = std::fs::read_to_string(engine_dir.join(CLIENTS_DIR).join(agent)).ok()?;
    Some(text.trim().to_owned()).filter(|stamp| !stamp.is_empty())
}

/// The sidecar `connect` writes beside a token.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sidecar {
    cabinet: String,
    team: String,
    agent: String,
    token_id: String,
}

/// Every token kept on this machine, by the sidecars under `~/.vibememory/tokens/<team>/`.
#[must_use]
pub fn kept_tokens(layout: &Layout) -> Vec<KeptToken> {
    let root = layout.engine_dir.join(TOKENS_DIR);
    let Ok(teams) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    for team in teams.flatten().filter(|entry| entry.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(team.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let name = file.file_name().to_string_lossy().into_owned();
            let is_sidecar = path
                .extension()
                .is_some_and(|extension| extension == SIDECAR_EXTENSION)
                && !name.ends_with(FRAGMENT_SUFFIX);
            if is_sidecar {
                let mut token = inspect(&path);
                token.client_started = client_started(&layout.engine_dir, &token.agent);
                tokens.push(token);
            }
        }
    }
    tokens.sort_by(|left, right| (&left.team, &left.agent).cmp(&(&right.team, &right.agent)));
    tokens
}

/// One token by its sidecar: the files it needs, each reachable by the owner alone.
fn inspect(sidecar: &Path) -> KeptToken {
    let file = sidecar.with_extension("");
    let parsed = std::fs::read_to_string(sidecar)
        .map_err(|error| error.to_string())
        .and_then(|text| serde_json::from_str::<Sidecar>(&text).map_err(|error| error.to_string()));
    let mut kept = match parsed {
        Ok(sidecar) => KeptToken {
            team: sidecar.team,
            agent: sidecar.agent,
            token_id: sidecar.token_id,
            cabinet: sidecar.cabinet,
            file: file.clone(),
            problems: Vec::new(),
            client_started: None,
        },
        Err(error) => KeptToken {
            team: String::new(),
            agent: file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            token_id: String::new(),
            cabinet: String::new(),
            file: file.clone(),
            problems: vec![format!("its sidecar is unreadable: {error}")],
            client_started: None,
        },
    };
    for path in [sidecar.to_path_buf(), file.clone(), fragment_file(&file)] {
        if !path.exists() {
            kept.problems.push(format!("{} is missing", path.display()));
            continue;
        }
        if let Some(problem) = reachable_by_others(&path) {
            kept.problems.push(problem);
        }
    }
    kept
}

/// Why others than the owner can reach `path`, or `None` when they cannot.
#[cfg(unix)]
fn reachable_by_others(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| {
        format!(
            "{} is readable by others (mode {mode:o}): chmod 600 it",
            path.display()
        )
    })
}

/// Why others than the owner can reach `path`, by `icacls` against `whoami`.
#[cfg(not(unix))]
fn reachable_by_others(path: &Path) -> Option<String> {
    use vibememory_core::claim::{AccessList, access_list};
    let run = |program: &str, args: &[&std::ffi::OsStr]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let Some(user) = run("whoami", &[]) else {
        return Some("whoami did not answer: the rights could not be checked".to_owned());
    };
    let Some(output) = run("icacls", &[path.as_os_str()]) else {
        return Some(format!(
            "icacls could not read the rights of {}",
            path.display()
        ));
    };
    match access_list(&output, &path.to_string_lossy(), &user) {
        AccessList::OwnerOnly => None,
        AccessList::Others(others) => Some(format!(
            "{} is reachable by {}: run vibememory connect again",
            path.display(),
            others.join(", ")
        )),
        AccessList::Unreadable => Some(format!(
            "icacls named nobody for {}: the rights could not be checked",
            path.display()
        )),
    }
}
