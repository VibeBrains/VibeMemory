//! `vibememory update`: the newest release from the host, installed over this one.
//!
//! The same three files the one-line installers read (`vibememory_core::release`), the same checks:
//! the archive's SHA-256 against the published sum, nothing installed when they differ. The new
//! program installs itself — `install` of the version being installed, not of the one running —
//! so a release that changed how it installs is installed its own way.
//!
//! The tick asks once a day whether a newer version is out and remembers the answer; it never
//! installs anything by itself. `SessionStart` and `doctor` say the version is out.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use vibememory_core::release;

use crate::install::Layout;

/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The cabinet whose host publishes the releases, when the machine knows of no other.
pub const DEFAULT_CABINET: &str = "https://app.vibememory.ru";

/// Where under a cabinet the releases are served.
const RELEASES_PATH: &str = "/dl/";

/// Names another release directory, as it does for the one-line installers: a test against a local
/// copy, or a host of one's own.
pub const RELEASES_VAR: &str = "VIBEMEMORY_DL";

/// Set for the program `connect` hands over to after updating itself: that one does not update
/// again, so a host whose `latest` names a version its archive is not can never start a loop.
pub const NO_SELF_UPDATE_VAR: &str = "VIBEMEMORY_NO_SELF_UPDATE";

/// How often the tick asks whether a newer version is out.
pub const CHECK_EVERY_SECONDS: u64 = 24 * 60 * 60;

/// How long reading `latest` may take: the tick waits for it.
const LATEST_TIMEOUT_SECONDS: &str = "15";

/// How long downloading an archive may take.
const DOWNLOAD_TIMEOUT_SECONDS: &str = "300";

/// What the tick last learned, kept beside the engine's other state.
const STATE_FILE: &str = "update.json";

/// The release directory: the one [`RELEASES_VAR`] names, else the one of the cabinet this machine
/// was connected from, else the default cabinet's.
#[must_use]
pub fn releases_base(layout: &Layout) -> String {
    if let Ok(base) = std::env::var(RELEASES_VAR)
        && !base.trim().is_empty()
    {
        return base;
    }
    base_of_cabinet(&known_cabinet(layout).unwrap_or_else(|| DEFAULT_CABINET.to_owned()))
}

/// The release directory of a cabinet.
#[must_use]
pub fn base_of_cabinet(cabinet: &str) -> String {
    format!("{}{RELEASES_PATH}", cabinet.trim_end_matches('/'))
}

/// The cabinet this machine was connected from: the personal store's record first, then a team's,
/// then a kept token's.
fn known_cabinet(layout: &Layout) -> Option<String> {
    let personal = std::fs::read_to_string(
        layout
            .personal_state_dir()
            .join(crate::team_connect::RECORD_FILE),
    )
    .ok()
    .and_then(|text| vibememory_core::team_store::StoreRecord::parse(&text).ok())
    .map(|record| record.cabinet);
    personal
        .or_else(|| {
            crate::team_connect::connected_teams(layout)
                .iter()
                .find_map(|team| crate::team_connect::read_record(layout, team).ok())
                .map(|record| record.cabinet)
        })
        .or_else(|| {
            crate::credentials::kept_tokens(layout)
                .into_iter()
                .map(|token| token.cabinet)
                .find(|cabinet| !cabinet.is_empty())
        })
}

/// Fetches a URL with curl into `output`, or into memory when `output` is `None`. curl reads no
/// `.curlrc` and speaks only the scheme of the base — https, or file for a local copy — so no
/// configuration of the machine and no redirect sends the download anywhere else.
fn fetch(url: &str, output: Option<&Path>, timeout: &str) -> Result<Vec<u8>, String> {
    let protocol = if url.starts_with("https://") {
        "=https"
    } else if url.starts_with("file://") {
        "=file"
    } else {
        return Err(format!(
            "{url}: releases are read over https only (or from file:// for a local copy)"
        ));
    };
    let mut command = Command::new("curl");
    command.args([
        "-q",
        "--proto",
        protocol,
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        timeout,
    ]);
    if let Some(path) = output {
        command.arg("--output").arg(path);
    }
    let result = command
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("curl could not be started: {error}"))?;
    if result.status.success() {
        Ok(result.stdout)
    } else {
        Err(format!(
            "{url}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

/// The newest published version.
///
/// # Errors
///
/// The host could not be read, or answered something that is not a version.
pub fn latest(base: &str) -> Result<String, String> {
    let answer = fetch(&format!("{base}latest"), None, LATEST_TIMEOUT_SECONDS)?;
    release::parse_version(&String::from_utf8_lossy(&answer))
        .ok_or_else(|| format!("{base}latest: the host answered no version"))
}

/// What `update` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Updated {
    /// Nothing newer is published; the version running is the one named.
    Current(String),
    /// The version named was installed.
    Installed(String),
}

/// Installs the newest release when it is newer than this build.
///
/// # Errors
///
/// A sentence for a person: the host unreadable, no release for this system, a sum that does not
/// match, an archive that does not unpack, or the new program's `install` failing.
pub fn update(layout: &Layout, base: &str) -> Result<Updated, String> {
    let version = latest(base)?;
    remember(layout, &version, epoch_seconds());
    if !release::is_newer(&version, CURRENT) {
        return Ok(Updated::Current(CURRENT.to_owned()));
    }
    let target =
        release::target(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
            format!(
                "no release is built for {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
    let work = layout
        .engine_dir
        .join(format!("update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("unpacked"))
        .map_err(|error| format!("{}: {error}", work.display()))?;
    let installed = download_and_install(layout, base, &version, target, &work);
    let _ = std::fs::remove_dir_all(&work);
    installed.map(|()| Updated::Installed(version))
}

/// The archive into `work`, checked, unpacked, and its program's `install` run.
fn download_and_install(
    layout: &Layout,
    base: &str,
    version: &str,
    target: &str,
    work: &Path,
) -> Result<(), String> {
    let archive = release::archive_name(version, target);
    let url = format!("{base}{version}/{archive}");
    let local = work.join(&archive);
    fetch(&url, Some(&local), DOWNLOAD_TIMEOUT_SECONDS)?;
    let published = fetch(&format!("{url}.sha256"), None, LATEST_TIMEOUT_SECONDS)?;
    let expected = release::published_sum(&String::from_utf8_lossy(&published))
        .ok_or_else(|| format!("{url}.sha256 holds no sum"))?;
    let bytes = std::fs::read(&local).map_err(|error| format!("{}: {error}", local.display()))?;
    if crate::sha256::hex(&bytes) != expected {
        return Err(format!(
            "the SHA-256 of {archive} does not match the published one: nothing is installed"
        ));
    }
    let unpacked = work.join("unpacked");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&local)
        .arg("-C")
        .arg(&unpacked)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("tar could not be started: {error}"))?;
    if !status.success() {
        return Err(format!("{archive} could not be unpacked"));
    }
    let program = unpacked.join(format!("vibememory{}", std::env::consts::EXE_SUFFIX));
    let status = Command::new(&program)
        .arg("install")
        .env(crate::install::ENGINE_DIR_VAR, &layout.engine_dir)
        .env(crate::install::CONFIG_DIR_VAR, &layout.config_dir)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("{}: {error}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the install of {version} did not finish; what it said is above"
        ))
    }
}

/// What the tick last learned about releases.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct State {
    /// When the host was last asked, seconds since the epoch.
    checked_at: u64,
    /// The newest version it named then; empty when it could not be read.
    latest: String,
}

fn state_path(layout: &Layout) -> PathBuf {
    layout.engine_dir.join(STATE_FILE)
}

fn read_state(layout: &Layout) -> State {
    std::fs::read_to_string(state_path(layout))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Notes what the host said and when. Losing the note costs one more question tomorrow.
fn remember(layout: &Layout, latest: &str, now: u64) {
    let state = State {
        checked_at: now,
        latest: latest.to_owned(),
    };
    if let Ok(text) = serde_json::to_string(&state) {
        let _ = std::fs::write(state_path(layout), text);
    }
}

/// Seconds since the epoch.
fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Asks the host for `latest` when the last question is a day old, for the tick. A host that
/// cannot be read is asked again the next day, not every two minutes: the answer only feeds a
/// sentence, and nothing waits on it.
///
/// Returns the newer version when one is out.
#[must_use]
pub fn check_if_due(layout: &Layout, base: &str, now: u64) -> Option<String> {
    let state = read_state(layout);
    if now.saturating_sub(state.checked_at) >= CHECK_EVERY_SECONDS {
        let answer = latest(base).unwrap_or_default();
        // an unreadable host keeps what was known, and waits a day like a readable one
        let known = if answer.is_empty() {
            state.latest
        } else {
            answer
        };
        remember(layout, &known, now);
    }
    newer_known(layout)
}

/// The newer version the tick last heard of, if the running one is older. Reads a file only: for
/// `SessionStart` and `doctor`, which must not wait on the network.
#[must_use]
pub fn newer_known(layout: &Layout) -> Option<String> {
    let latest = read_state(layout).latest;
    (!latest.is_empty() && release::is_newer(&latest, CURRENT)).then_some(latest)
}
