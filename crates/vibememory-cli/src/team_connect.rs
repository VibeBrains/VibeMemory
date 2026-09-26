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
    /// The public key, `ssh-ed25519 …`, as it goes to the cabinet.
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
        let public = std::fs::read_to_string(dir.join(format!("{KEY_FILE}.pub")))
            .map_err(|error| error.to_string())?
            .trim()
            .to_owned();
        Ok(Self { dir, public })
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
                "the team's sessions are off: turn them on on the team's page, then connect again with a new code",
            ),
            Self::EngineMissing => formatter.write_str(
                "sessions travel with the engine, which is not installed here: run `vibememory install`, then connect again with a new code",
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
        let mut command = crate::git::command(
            &state_dir,
            &["-c", &format!("core.sshCommand={ssh}"), "clone", "--quiet"],
        );
        command.arg(record.git_url()).arg(&clone);
        match crate::git::run_capturing(command, CLONE_TIMEOUT).map_err(failed)? {
            Ok(_) => {}
            Err(stderr) => {
                return Err(failed(format!(
                    "the team's store could not be cloned: {}",
                    vibememory_core::terminal::printable(&stderr)
                )));
            }
        }
        set_clone(&clone, &ssh, &record.git_url()).map_err(failed)?;
        true
    };
    Ok(ConnectedStore {
        state_dir,
        clone,
        cloned,
    })
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
fn set_clone(clone: &Path, ssh: &str, url: &str) -> Result<(), String> {
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
    let path = layout.team_state_dir(team).join(RECORD_FILE);
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
    let path = layout.team_state_dir(team).join(KNOWN_HOSTS_FILE);
    crate::connect::write_private(&path, hosts.as_bytes())?;
    Ok(path)
}
