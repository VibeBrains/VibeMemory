//! The machine key branch of `connect`: a fresh ed25519 key goes to the cabinet with the claim code,
//! and when the code was for a machine of a team whose sessions are on, the key is kept in the
//! team's state directory with the host's keys the cabinet vouched for, and the team's store is
//! cloned there.
//!
//! The clone carries its own ssh command (`core.sshCommand`): this key, this team's `known_hosts`,
//! nothing of the user's ssh configuration (`-F none`). The engine never edits `~/.ssh/config`, so
//! connecting or leaving a team leaves the person's own ssh exactly as it was.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use vibememory_core::claim::KeyGrant;
use vibememory_core::team_store::{StoreRecord, known_hosts};

use crate::install::{Layout, shell_word};

/// The private key's file name in a state directory; the public key sits beside it with `.pub`.
pub const KEY_FILE: &str = "key";

/// The team's own list of host keys, in its state directory.
pub const KNOWN_HOSTS_FILE: &str = "known_hosts";

/// What the machine knows about the team, in its state directory.
pub const RECORD_FILE: &str = "store.json";

/// Where the key of a host without the cabinet waits for its grant, under `<engine>/stores/`.
const GRANT_PENDING_DIR: &str = ".pending-grant";

/// The comment of every key the engine makes: what it is for, and nothing that names the person.
const KEY_COMMENT: &str = "vibememory";

/// How long a first clone may take: a store with sessions can be large.
const CLONE_TIMEOUT: Duration = Duration::from_mins(10);

/// How long a small git or ssh-keygen step may take.
const STEP_TIMEOUT: Duration = Duration::from_mins(1);

/// A key made for a claim, not yet anybody's: it lies in a pending directory until the answer says
/// whether it is a machine key, and goes away when it is not.
#[derive(Debug)]
pub struct PendingKey {
    dir: PathBuf,
    /// The public key in the host's form, `ssh-ed25519 <base64>` without a comment, as it goes to the cabinet.
    pub public: String,
}

impl PendingKey {
    /// Makes a key in `<engine>/stores/.pending-<pid>`, reachable by the owner alone.
    ///
    /// # Errors
    ///
    /// When `ssh-keygen` is missing or fails: then the claim goes without a key, and only a code
    /// for a machine is refused.
    pub fn make(layout: &Layout) -> Result<Self, String> {
        let dir = layout
            .engine_dir
            .join("stores")
            .join(format!(".pending-{}", std::process::id()));
        Self::generate(dir)
    }

    /// The key of a host without the cabinet, which `connect --key-request` makes and
    /// `connect --grant` takes: it waits in `<engine>/stores/.pending-grant` while the host's owner
    /// registers its public half, so asking again shows the same key instead of a new one.
    ///
    /// # Errors
    ///
    /// As [`PendingKey::make`].
    pub fn for_grant(layout: &Layout) -> Result<Self, String> {
        let dir = layout.engine_dir.join("stores").join(GRANT_PENDING_DIR);
        if let Some(public) = std::fs::read_to_string(dir.join(format!("{KEY_FILE}.pub")))
            .ok()
            .and_then(|file| vibememory_core::ssh_key::wire_line(&file))
            && dir.join(KEY_FILE).is_file()
        {
            return Ok(Self { dir, public });
        }
        Self::generate(dir)
    }

    /// The key that `connect --key-request` left, if there is one.
    #[must_use]
    pub fn waiting_for_grant(layout: &Layout) -> Option<Self> {
        let dir = layout.engine_dir.join("stores").join(GRANT_PENDING_DIR);
        let public = std::fs::read_to_string(dir.join(format!("{KEY_FILE}.pub")))
            .ok()
            .and_then(|file| vibememory_core::ssh_key::wire_line(&file))?;
        dir.join(KEY_FILE).is_file().then_some(Self { dir, public })
    }

    fn generate(dir: PathBuf) -> Result<Self, String> {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        crate::connect::restrict_directory(&dir)?;
        let key = dir.join(KEY_FILE);
        let status = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", KEY_COMMENT, "-f"])
            .arg(&key)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("ssh-keygen could not be started: {error}"))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("ssh-keygen could not make a key".to_owned());
        }
        let file = std::fs::read_to_string(dir.join(format!("{KEY_FILE}.pub")))
            .map_err(|error| error.to_string())?;
        // the host takes the type and the key only: the comment would make it an `authorized_keys` option
        let Some(public) = vibememory_core::ssh_key::wire_line(&file) else {
            let _ = std::fs::remove_dir_all(&dir);
            return Err("ssh-keygen made a key the host would not take".to_owned());
        };
        Ok(Self { dir, public })
    }

    /// The directory the key lies in until it is kept.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Drops the key: the code was for a token, or the claim failed.
    pub fn discard(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Where a connected team lies on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectedStore {
    /// The team's state directory.
    pub state_dir: PathBuf,
    /// Its clone.
    pub clone: PathBuf,
    /// Whether the clone was made now rather than found from an earlier connect.
    pub cloned: bool,
}

/// Why a key answer set nothing up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRefusal {
    /// The team's sessions are off: its store is not cloned, and the key has nothing to push.
    SessionsOff,
    /// The engine is not installed: a team store is kept by the tick, which is not there.
    EngineMissing,
    /// The cabinet named no host key, so the host could not be checked on the first connection.
    NoHostKeys,
    /// A step failed.
    Failed(String),
}

impl std::fmt::Display for KeyRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SessionsOff => formatter.write_str(
                "the team's sessions are off: turn them on on the team's page, then connect again with a new code or grant",
            ),
            Self::EngineMissing => formatter.write_str(
                "sessions travel with the engine, which is not installed here: run `vibememory install`, then connect again with a new code or grant",
            ),
            Self::NoHostKeys => formatter.write_str(
                "the cabinet named no key of its host, so the host could not be checked: try again in a few minutes with a new code",
            ),
            Self::Failed(reason) => formatter.write_str(reason),
        }
    }
}

/// Keeps the key of a machine key answer and makes the team's clone: the record, the key and the
/// host keys go into `<engine>/stores/<team>/`, then the store is cloned beside them — or, for a
/// machine connecting again, the clone it has is pointed at the new key.
///
/// # Errors
///
/// [`KeyRefusal`]. The key stays registered in the cabinet whatever happens here; the caller
/// names it for revocation.
pub fn keep_key(
    layout: &Layout,
    grant: &KeyGrant,
    pending: PendingKey,
    engine_installed: bool,
) -> Result<ConnectedStore, KeyRefusal> {
    let Some(record) = StoreRecord::from_grant(grant) else {
        pending.discard();
        return Err(KeyRefusal::SessionsOff);
    };
    if !engine_installed {
        pending.discard();
        return Err(KeyRefusal::EngineMissing);
    }
    let Some(hosts) = known_hosts(&grant.ssh_host, &grant.host_keys) else {
        pending.discard();
        return Err(KeyRefusal::NoHostKeys);
    };
    let failed = |error: String| KeyRefusal::Failed(error);
    let state_dir = layout.team_state_dir(&record.team);
    std::fs::create_dir_all(&state_dir).map_err(|error| failed(error.to_string()))?;
    crate::connect::restrict_directory(&state_dir).map_err(failed)?;
    for name in [KEY_FILE.to_owned(), format!("{KEY_FILE}.pub")] {
        std::fs::rename(pending.dir.join(&name), state_dir.join(&name))
            .map_err(|error| failed(format!("the key could not be kept: {error}")))?;
    }
    pending.discard();
    crate::connect::write_private(&state_dir.join(KNOWN_HOSTS_FILE), hosts.as_bytes())
        .map_err(failed)?;
    crate::connect::write_private(&state_dir.join(RECORD_FILE), record.to_json().as_bytes())
        .map_err(failed)?;
    let clone = layout.team_store(&record.team);
    let ssh = ssh_command(&state_dir);
    let cloned = if clone.join(".git").exists() {
        set_clone(&clone, &ssh, &record.git_url()).map_err(failed)?;
        false
    } else {
        announce_size(&state_dir, &record);
        clone_store(&state_dir, &record, &record.git_url()).map_err(failed)?;
        true
    };
    Ok(ConnectedStore {
        state_dir,
        clone,
        cloned,
    })
}

/// Clones a team's store — from `source`, its host's address in every real use — into its state
/// directory with its own ssh and the store's settings from
/// the first moment: git writes its defaults at init, and `core.autocrlf` must hold before the
/// first file is checked out.
pub(crate) fn clone_store(
    state_dir: &Path,
    record: &StoreRecord,
    source: &str,
) -> Result<PathBuf, String> {
    let clone = state_dir.join("store");
    let ssh = ssh_command(state_dir);
    // git's own progress goes to the person: a store with sessions takes minutes, and a silent
    // terminal reads as a hang
    let mut command = crate::git::command(
        state_dir,
        &[
            "-c",
            &format!("core.sshCommand={ssh}"),
            "clone",
            "--progress",
        ],
    );
    for (key, value, _why) in crate::install::GIT_SETTINGS {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command.arg(source).arg(&clone);
    command.stderr(std::process::Stdio::inherit());
    if crate::git::run_capturing(command, CLONE_TIMEOUT)?.is_err() {
        return Err("the team's store could not be cloned: git said why above".to_owned());
    }
    set_clone(&clone, &ssh, &record.git_url())?;
    Ok(clone)
}

/// Says how big a store is before it is cloned, from the host's own `status` over the new key: the
/// clone then shows git's progress against a number the person already knows. Silent when the
/// host does not say — the clone goes ahead either way.
pub fn announce_size(state_dir: &Path, record: &StoreRecord) {
    let output = Command::new("ssh")
        .args(["-F", "none", "-i"])
        .arg(state_dir.join(KEY_FILE))
        .args([
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
        ])
        .arg(format!(
            "UserKnownHostsFile={}",
            state_dir.join(KNOWN_HOSTS_FILE).display()
        ))
        .arg(format!("{}@{}", record.ssh_user, record.ssh_host))
        .arg("status")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let size = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|status| {
            status
                .get("teams")?
                .get(&record.team)?
                .get("sizeBytes")?
                .as_u64()
        });
    if let Some(bytes) = size {
        println!(
            "cloning:   {} on the host — git shows its progress below",
            readable_size(bytes)
        );
    }
}

/// Bytes as a person reads them: MiB below a GiB, GiB with one decimal above.
#[must_use]
pub fn readable_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes < GIB {
        format!("{} MiB", bytes.div_ceil(MIB))
    } else {
        let tenths = (bytes * 10).div_ceil(GIB);
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    }
}

/// The clone's own ssh: this team's key and host keys only, the user's configuration not read,
/// and never a question — the tick has nobody to answer it. Git runs the line through `sh` on every
/// system, Windows too, so the paths are single-quoted words.
#[must_use]
pub fn ssh_command(state_dir: &Path) -> String {
    format!(
        "ssh -F none -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile={}",
        shell_word(&state_dir.join(KEY_FILE)),
        shell_word(&state_dir.join(KNOWN_HOSTS_FILE)),
    )
}

/// Points a clone at its key and address.
pub(crate) fn set_clone(clone: &Path, ssh: &str, url: &str) -> Result<(), String> {
    let steps: [&[&str]; 2] = [
        &["config", "core.sshCommand", ssh],
        &["remote", "set-url", "origin", url],
    ];
    for args in steps {
        match crate::git::run_capturing(crate::git::command(clone, args), STEP_TIMEOUT)? {
            Ok(_) => {}
            Err(stderr) => {
                return Err(format!(
                    "git {} failed: {}",
                    args.first().copied().unwrap_or_default(),
                    vibememory_core::terminal::printable(&stderr)
                ));
            }
        }
    }
    Ok(())
}

/// What the machine knows about a team, read from its state directory.
///
/// # Errors
///
/// A record that is missing or not one this engine reads.
pub fn read_record(layout: &Layout, team: &str) -> Result<StoreRecord, String> {
    let path = state_dir_of(layout, team).join(RECORD_FILE);
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    StoreRecord::parse(&text).map_err(|error| error.to_string())
}

/// The cabinet's public list of its host's keys.
const HOST_KEYS_PATH: &str = "/api/agent/hostkeys";

/// How long the cabinet may take to list the host's keys, in seconds.
const HOST_KEYS_TIMEOUT_SECONDS: &str = "30";

/// `connect --refresh <team>`: the team's `known_hosts` rewritten from the cabinet the store record
/// names, after the host changed its key. Only that file changes; the key and the clone stay.
///
/// # Errors
///
/// A team not connected here, a cabinet not reached, or an answer
/// [`vibememory_core::team_store::refreshed_known_hosts`] refuses — the old file then stays.
pub fn refresh(layout: &Layout, team: &str) -> Result<PathBuf, String> {
    if !vibememory_core::naming::is_slug(team) {
        return Err(format!("{team:?} is not a team"));
    }
    let record = read_record(layout, team)?;
    let cabinet = vibememory_core::claim::cabinet_address(&record.cabinet)?;
    let protocol = if cabinet.starts_with("https://") {
        "=https"
    } else {
        "=http"
    };
    // the same curl as the claim: no .curlrc, the cabinet's scheme only
    let output = Command::new("curl")
        .args([
            "-q",
            "--proto",
            protocol,
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            HOST_KEYS_TIMEOUT_SECONDS,
        ])
        .arg(format!("{cabinet}{HOST_KEYS_PATH}"))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "the cabinet was not reached: {}",
            vibememory_core::terminal::printable(String::from_utf8_lossy(&output.stderr).trim())
        ));
    }
    let hosts = vibememory_core::team_store::refreshed_known_hosts(
        &String::from_utf8_lossy(&output.stdout),
        &record,
    )?;
    let path = state_dir_of(layout, team).join(KNOWN_HOSTS_FILE);
    crate::connect::write_private(&path, hosts.as_bytes())?;
    Ok(path)
}

/// Where a connection's key and record lie: a team's state directory, or `<engine>/personal` for
/// the personal store — which is `personal` by its record, and never among the teams.
fn state_dir_of(layout: &Layout, team: &str) -> PathBuf {
    let personal = layout.personal_state_dir();
    let is_personal = std::fs::read_to_string(personal.join(RECORD_FILE))
        .ok()
        .and_then(|text| StoreRecord::parse(&text).ok())
        .is_some_and(|record| record.team == team);
    if is_personal {
        personal
    } else {
        layout.team_state_dir(team)
    }
}

/// The teams connected on this machine: every state directory under `<engine>/stores` that holds a
/// store record, by id. A pending key directory (`.pending-*`) and an archived one are not teams.
#[must_use]
pub fn connected_teams(layout: &Layout) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(layout.engine_dir.join("stores")) else {
        return Vec::new();
    };
    let mut teams: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| vibememory_core::naming::is_slug(name))
        .filter(|name| layout.team_state_dir(name).join(RECORD_FILE).is_file())
        .collect();
    teams.sort();
    teams
}

/// What `doctor` says about one team store beyond the plan's steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamFacts {
    /// The team.
    pub team: String,
    /// The cabinet the record names, for the advice; empty when the record does not read.
    pub cabinet: String,
    /// What is wrong on this machine: a missing clone, key or host list, a key others may read.
    pub problems: Vec<String>,
    /// The pause the host put the store under, if any.
    pub pause: Option<crate::guard::StorePause>,
    /// Failing runs in a row of this store's tick.
    pub failures: u32,
}

/// The facts of every connected team.
#[must_use]
pub fn team_facts(layout: &Layout) -> Vec<TeamFacts> {
    connected_teams(layout)
        .into_iter()
        .map(|team| {
            let state_dir = layout.team_state_dir(&team);
            let mut problems = Vec::new();
            let cabinet = match read_record(layout, &team) {
                Ok(record) => record.cabinet,
                Err(error) => {
                    problems.push(format!("store record: {error}"));
                    String::new()
                }
            };
            if !layout.team_store(&team).join(".git").is_dir() {
                problems.push("the clone is missing: connect again with a new code".to_owned());
            }
            let key = state_dir.join(KEY_FILE);
            if key.is_file() {
                if let Some(problem) = key_rights_problem(&key) {
                    problems.push(problem);
                }
            } else {
                problems
                    .push("the machine key is missing: connect again with a new code".to_owned());
            }
            if !state_dir.join(KNOWN_HOSTS_FILE).is_file() {
                problems.push(format!(
                    "the host's keys are missing: `vibememory connect --refresh {team}`"
                ));
            }
            let state = crate::guard::TickState::read(&state_dir);
            TeamFacts {
                team,
                cabinet,
                problems,
                pause: state.pause,
                failures: state.consecutive_failures,
            }
        })
        .collect()
}

/// A private key others may read is a key ssh itself refuses, and a key anybody could copy.
#[cfg(unix)]
fn key_rights_problem(key: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(key).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then(|| format!("the machine key is readable by others (mode {mode:o})"))
}

/// On Windows the rights are an access list `connect` narrowed; ssh checks it itself.
#[cfg(not(unix))]
fn key_rights_problem(_key: &Path) -> Option<String> {
    None
}
