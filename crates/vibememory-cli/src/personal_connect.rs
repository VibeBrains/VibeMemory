//! A new machine of the owner joins the personal store by a code, as a member's machine joins a
//! team: the key the claim made is kept in `<engine>/personal`, the store is cloned as the main
//! store `<engine>/store`, and `config.json` is written when the machine has none — after which the
//! full install makes it a machine of the engine like any other.
//!
//! Nothing of the owner's is overwritten: a main store already here is pointed at the new key, not
//! replaced, and a `config.json` already here keeps its machine id — its directory under
//! `machines/` in the store is that machine's history.

use std::path::{Path, PathBuf};

use vibememory_core::claim::KeyGrant;
use vibememory_core::team_store::{StoreRecord, known_hosts};

use crate::install::Layout;
use crate::team_connect::{
    KEY_FILE, KNOWN_HOSTS_FILE, KeyRefusal, PendingKey, RECORD_FILE, set_clone, ssh_command,
};

/// How long the first clone may take: the personal store carries every transcript of the owner.
const CLONE_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(30);

/// What connecting did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connected {
    /// The main store.
    pub store: PathBuf,
    /// Whether it was cloned now rather than found and pointed at the new key.
    pub cloned: bool,
    /// The machine id: the one `config.json` already had, or the one written now.
    pub machine_id: String,
    /// Whether `config.json` was written now.
    pub configured: bool,
}

/// The machine id a machine without `config.json` takes: the one asked for, or the machine's own
/// network name — which is how the first machines were named, so a machine connecting again finds
/// its own directory under `machines/`.
#[must_use]
pub fn machine_id(asked: Option<&str>) -> Option<String> {
    asked
        .map(str::to_owned)
        .or_else(sysinfo::System::host_name)
        .map(|name| name.trim().to_owned())
        .filter(|name| is_machine_id(name))
}

/// A machine id becomes a directory name in the store on every system: letters, digits, `-`, `_`
/// and `.`, not starting with a dot.
#[must_use]
pub fn is_machine_id(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Keeps the key of a personal store answer, makes or re-aims the main store and writes
/// `config.json` if there is none.
///
/// # Errors
///
/// [`KeyRefusal`]. The key stays registered in the cabinet whatever happens here; the caller names
/// it for revocation.
pub fn connect(
    layout: &Layout,
    grant: &KeyGrant,
    pending: PendingKey,
    asked_machine_id: Option<&str>,
) -> Result<Connected, KeyRefusal> {
    let failed = KeyRefusal::Failed;
    let Some(record) = StoreRecord::from_grant(grant) else {
        pending.discard();
        return Err(failed(
            "the answer is not for the personal store".to_owned(),
        ));
    };
    let Some(hosts) = known_hosts(&grant.ssh_host, &grant.host_keys) else {
        pending.discard();
        return Err(KeyRefusal::NoHostKeys);
    };
    let config_path = layout.engine_dir.join("config.json");
    let existing = std::fs::read_to_string(&config_path).ok();
    let configured_id = existing
        .as_deref()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
        .and_then(|config| config.get("machineId")?.as_str().map(str::to_owned));
    let Some(machine_id) = configured_id.or_else(|| machine_id(asked_machine_id)) else {
        pending.discard();
        return Err(failed(
            "no machine id: name this machine with --machine-id <name> (letters, digits, - _ .)"
                .to_owned(),
        ));
    };

    let state_dir = layout.personal_state_dir();
    std::fs::create_dir_all(&state_dir).map_err(|error| failed(error.to_string()))?;
    crate::connect::restrict_directory(&state_dir).map_err(failed)?;
    for name in [KEY_FILE.to_owned(), format!("{KEY_FILE}.pub")] {
        std::fs::rename(pending.path().join(&name), state_dir.join(&name))
            .map_err(|error| failed(format!("the key could not be kept: {error}")))?;
    }
    pending.discard();
    crate::connect::write_private(&state_dir.join(KNOWN_HOSTS_FILE), hosts.as_bytes())
        .map_err(failed)?;
    crate::connect::write_private(&state_dir.join(RECORD_FILE), record.to_json().as_bytes())
        .map_err(failed)?;

    let store = layout.store();
    let ssh = ssh_command(&state_dir);
    let cloned = if store.join(".git").exists() {
        set_clone(&store, &ssh, &record.git_url()).map_err(failed)?;
        false
    } else {
        clone_main(&store, &ssh, &record.git_url()).map_err(failed)?;
        true
    };

    let configured = existing.is_none();
    if configured {
        let text = serde_json::to_string_pretty(&serde_json::json!({ "machineId": machine_id }))
            .map_err(|error| failed(error.to_string()))?;
        crate::route::write_config(&config_path, &format!("{text}\n")).map_err(failed)?;
    }
    Ok(Connected {
        store,
        cloned,
        machine_id,
        configured,
    })
}

/// Clones the personal store as the main store, with its own ssh and the store's settings from the
/// first moment.
fn clone_main(store: &Path, ssh: &str, url: &str) -> Result<(), String> {
    let parent = store.parent().ok_or("the store has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut command = crate::git::command(
        parent,
        &["-c", &format!("core.sshCommand={ssh}"), "clone", "--quiet"],
    );
    for (key, value, _why) in crate::install::GIT_SETTINGS {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command.arg(url).arg(store);
    if let Err(stderr) = crate::git::run_capturing(command, CLONE_TIMEOUT)? {
        return Err(format!(
            "the personal store could not be cloned: {}",
            vibememory_core::terminal::printable(&stderr)
        ));
    }
    set_clone(store, ssh, url)
}
