//! The host's commands: what a machine key's forced command runs, what a push's `pre-receive`
//! runs, the application of a snapshot and the host's report.
//!
//! Every decision is made by a pure module — [`shell`], [`receive`], [`apply`], [`status`] — and
//! tested there against its fixture. This module only gathers what they need from the disk and
//! does what they decided.

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde::Serialize;
use serde::de::DeserializeOwned;
use vibememory_core::terminal::printable;

use crate::access::{self, Snapshot};
use crate::apply::{self, Step};
use crate::disk::dir_size;
use crate::git_memories::{self, GitMemories};
use crate::host::TeamMemories;
use crate::http::{self, Grant};
use crate::layout::{self, TeamDir, store_dir, team_dir};
use crate::protocol;
use crate::push_scan::{self, Scanned};
use crate::receive::{self, Objects, Push, Sizes, Update};
use crate::shell::{self, ShellAction};
use crate::status::{
    self, Applied, Backup, Disk, Facts, HostReport, Problem, ProjectFacts, Refused, RepoFacts,
};
use crate::tools::{Limits, Writes};

/// The variable through which the forced command tells `pre-receive` whose key pushes.
pub const KEY_VARIABLE: &str = "VIBEMEMORY_KEY";
/// The file every team store's `main` holds from its first commit.
const GITATTRIBUTES_PATH: &str = ".gitattributes";
/// Who the host's own commits in a team store are signed by.
const APPLY_WRITER: &str = "host";
/// Tags of the journal lines, one per command, so `journalctl -t` finds each.
const SHELL_TAG: &str = "vibememory-shell";
const HOOK_TAG: &str = "vibememory-pre-receive";
const APPLY_TAG: &str = "vibememory-apply";
const STATUS_TAG: &str = "vibememory-status";
/// Where syslog listens; journald takes its lines.
#[cfg(unix)]
const SYSLOG_SOCKET: &str = "/dev/log";
/// `user.notice`: facility 1, severity 5.
#[cfg(unix)]
const SYSLOG_PRIORITY: u8 = 13;
/// How much of a client's command a journal line quotes: enough to see what was asked, not a
/// megabyte of it.
const QUOTED_COMMAND_CHARS: usize = 200;
/// Exit code of a refusal, as the spec gives it.
const REFUSED: u8 = 1;
/// The problem written when the snapshot itself is refused.
const SNAPSHOT_REJECTED: &str = "snapshotRejected";
/// The problem written when the snapshot is not newer than the one the host applied last.
const SERIAL_BEHIND: &str = "serialBehind";
/// The problem written when an operation on the host fails.
const APPLY_FAILED: &str = "applyFailed";
/// The problem written when the adopted store is not where the snapshot says.
const REPO_MISSING: &str = "repoMissing";
/// Mode of `vmgit`'s `authorized_keys` and its directory: sshd refuses looser ones.
const PRIVATE_FILE: u32 = 0o600;
const PRIVATE_DIR: u32 = 0o700;
/// Mode of the host's reports: its own account writes, the cabinet's group reads.
const SHARED_FILE: u32 = 0o640;
/// How many times one run applies a snapshot that keeps changing under it.
const APPLY_PASSES: usize = 3;

/// Where the host keeps the snapshot and the team stores.
#[derive(Debug, Clone)]
pub struct HostPaths {
    /// The access snapshot; `applied.json`, `host.json` and `backup.json` live beside it.
    pub access: PathBuf,
    /// The team stores.
    pub teams: PathBuf,
}

/// Sends a line to syslog, which journald takes; whether it could.
fn to_syslog(line: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixDatagram;
        let message = format!("<{SYSLOG_PRIORITY}>{line}");
        UnixDatagram::unbound()
            .and_then(|socket| socket.send_to(message.as_bytes(), SYSLOG_SOCKET))
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = line;
        false
    }
}

/// A line for the host's journal from a command whose stderr is a unit's or the operator's:
/// syslog, and stderr where there is none.
pub fn journal(line: &str) {
    if !to_syslog(line) {
        eprintln!("{line}");
    }
}

/// A line for the host's journal from a command whose stderr belongs to the client at the other
/// end of ssh: syslog or nowhere. The client reads the first line of stderr as the code of a
/// refusal, and a journal line there would take its place.
fn journal_quietly(line: &str) {
    let _ = to_syslog(line);
}

/// Text from outside, made safe for one journal line: control characters escaped, length capped.
fn quoted(text: &str) -> String {
    let head: String = text.chars().take(QUOTED_COMMAND_CHARS).collect();
    let cut = if text.chars().count() > QUOTED_COMMAND_CHARS {
        "…"
    } else {
        ""
    };
    format!("\"{}{cut}\"", printable(&head))
}

/// A refusal as the spec gives it: the code on the first line of stderr, what exactly on the next
/// lines, exit code 1.
fn refuse(code: &str, lines: &[&str]) -> ExitCode {
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "vibememory: {code}");
    for line in lines {
        let _ = writeln!(err, "{}", printable(line));
    }
    ExitCode::from(REFUSED)
}

/// The bytes of the snapshot at `access`, and the snapshot they hold, checked; the outer error is
/// a file that cannot be read at all.
fn read_checked(access: &Path) -> Result<(Vec<u8>, Result<Snapshot, String>), String> {
    let bytes = std::fs::read(access).map_err(|error| format!("{}: {error}", access.display()))?;
    let checked =
        access::check(&bytes).map_err(|refusal| format!("{}: {}", refusal.code, refusal.detail));
    Ok((bytes, checked))
}

/// The copy of the snapshot the host applied last, checked; `None` when there is none, or it does
/// not read — then the file alone decides, and `note` says why.
fn read_applied_snapshot(access: &Path, note: fn(&str)) -> Option<Snapshot> {
    let copy = layout::applied_snapshot_file(access);
    match read_checked(&copy) {
        Ok((_, Ok(snapshot))) => Some(snapshot),
        Err(_) if copy.symlink_metadata().is_err() => None,
        Ok((_, Err(why))) | Err(why) => {
            note(&format!(
                "the copy of the applied snapshot is passed over: {why}"
            ));
            None
        }
    }
}

/// The snapshot in force at `access`: the file, checked, unless it is not newer than the snapshot
/// the host applied last (`access::in_force`). A file that is gone or broken shuts the door whatever
/// the copy says: it may have been a revocation.
///
/// The application's own record, `applied.json`, is asked too, as the application asks it
/// (`newer_than_applied`): when the copy could not be written, the record alone knows that a newer
/// snapshot was applied, and a late older file must not open what that one closed. With no copy of
/// that snapshot to go by, the door stays shut until the cabinet publishes again.
///
/// # Errors
///
/// The file cannot be read or breaks a rule, or what is in force is older than what was applied.
pub fn read_in_force(access: &Path, note: fn(&str)) -> Result<Snapshot, String> {
    let (bytes, checked) = read_checked(access)?;
    let file = checked?;
    let copy = read_applied_snapshot(access, note);
    let file_in_force = copy
        .as_ref()
        .is_none_or(|copy| !access::behind(&file, copy));
    let file_hash = vibememory_cli::sha256::hex(&bytes);
    let in_force = access::in_force(file, copy);
    if let Ok(Some(applied)) = read_json::<Applied>(&layout::applied_file(access)) {
        let hash = file_in_force.then_some(file_hash.as_str());
        if older_than_applied(in_force.serial, hash, &applied) {
            return Err(format!(
                "serial {} is in force, but serial {} was applied and has no copy here",
                in_force.serial, applied.serial
            ));
        }
    }
    Ok(in_force)
}

/// Whether a snapshot of `serial` — with `hash`, when its bytes are known — is older than the one
/// `applied` records: a lower serial, or the same serial of the cabinet with other bytes.
fn older_than_applied(serial: u64, hash: Option<&str>, applied: &Applied) -> bool {
    serial < applied.serial
        || (serial > 0
            && serial == applied.serial
            && hash.is_some_and(|hash| hash != applied.snapshot_hash))
}

/// A JSON file of the host, or `None` when there is none yet. A file that is there and does not
/// read is an error: a report built past it would say something false.
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Temporary names tried before a write gives up: each is fresh, so only a directory someone keeps
/// filling on purpose runs out of them.
const TEMPORARY_ATTEMPTS: u32 = 8;

/// Writes a file whole or not at all: a temporary file beside it, then a rename over it. A reader
/// sees the old file or the new one, never half of one.
///
/// The temporary file is always a new one (`create_new`): the directory is shared with the
/// cabinet's account, and opening a name someone placed there — a file, or a link to one of this
/// account's own files — would write through it.
fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut last = None;
    for attempt in 0..TEMPORARY_ATTEMPTS {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let temporary = parent.join(format!(
            ".{name}.{}.{nanos}.{attempt}.tmp",
            std::process::id()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        #[cfg(not(unix))]
        let _ = mode;
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(error);
                continue;
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let written = file
            .write_all(bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&temporary, path));
        return written.map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("{}: {error}", path.display())
        });
    }
    Err(format!(
        "{}: no fresh temporary name after {TEMPORARY_ATTEMPTS} tries ({})",
        path.display(),
        last.map_or_else(String::new, |error| error.to_string())
    ))
}

/// Writes a report of the host, pretty, with a final newline.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let mut text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    text.push('\n');
    write_atomic(path, text.as_bytes(), SHARED_FILE)
}

/// The names of the directories in `dir`.
fn directory_names(dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

/// Free and total space of the partition `path` is on, as `df` gives them.
fn disk(path: &Path) -> Result<Disk, String> {
    let output = Command::new("df")
        .arg("-Pk")
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("df could not be started: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let found = vibememory_cli::mirror::read_disk(&text)
        .ok_or_else(|| format!("unreadable df answer: {}", text.trim()))?;
    Ok(Disk {
        free_bytes: found.available_kib.saturating_mul(1024),
        total_bytes: found.total_kib.saturating_mul(1024),
    })
}

// ------------------------------------------------------------------------------------------ shell

/// `shell <key>`: runs what the key's client asked for in `original`, if the snapshot lets it.
#[must_use]
pub fn shell(key: &str, original: &str, paths: &HostPaths) -> ExitCode {
    let snapshot = match read_in_force(&paths.access, journal_quietly) {
        Ok(snapshot) => Some(snapshot),
        Err(why) => {
            journal_quietly(&format!(
                "{SHELL_TAG}: the access snapshot cannot be used: {why}"
            ));
            None
        }
    };
    let teams = paths.teams.to_string_lossy();
    let now = vibememory_cli::clock::now();
    let action = match shell::decide(original, key, snapshot.as_ref(), &teams, &now) {
        Ok(action) => action,
        Err(refusal) => {
            journal_quietly(&format!(
                "{SHELL_TAG}: key {key} refused ({}): {}; asked {}",
                refusal.code,
                refusal.detail,
                quoted(original)
            ));
            return refuse(refusal.code, &[&refusal.detail]);
        }
    };
    // An action is only ever decided for a key of a usable snapshot.
    let Some((snapshot, identity)) = snapshot
        .as_ref()
        .and_then(|snapshot| Some((snapshot, snapshot.key(key)?)))
    else {
        return refuse("unknownKey", &["the key is not in the snapshot"]);
    };
    match action {
        ShellAction::Upload { team } => {
            journal_quietly(&format!(
                "{SHELL_TAG}: key {key} ({}) fetches {team}",
                identity.store_name
            ));
            run_git("upload-pack", &paths.teams.join(store_dir(&team)), None)
        }
        ShellAction::Receive { team } => {
            journal_quietly(&format!(
                "{SHELL_TAG}: key {key} ({}) pushes to {team}",
                identity.store_name
            ));
            run_git(
                "receive-pack",
                &paths.teams.join(store_dir(&team)),
                Some(key),
            )
        }
        ShellAction::Mcp { team, agent } => serve_team(&identity.id, &team, &agent, paths),
        ShellAction::Status => answer_status(snapshot, key, paths),
    }
}

/// Hands the connection to git: `upload-pack` or `receive-pack` of the store, by its path on the
/// host and never by the path the client sent.
fn run_git(command: &str, repo: &Path, key: Option<&str>) -> ExitCode {
    let mut git = Command::new("git");
    git.arg(command).arg(repo);
    if let Some(key) = key {
        git.env(KEY_VARIABLE, key);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = git.exec();
        refuse(
            "hostFailure",
            &[&format!("git {command} could not be started: {error}")],
        )
    }
    #[cfg(not(unix))]
    {
        match git.status() {
            Ok(status) if status.success() => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(error) => refuse(
                "hostFailure",
                &[&format!("git {command} could not be started: {error}")],
            ),
        }
    }
}

/// What one machine key may do in one team, taken from `snapshot` at `now`; why not, when it may
/// no longer — the key revoked, its member banned or out of the team, the team deleted.
fn team_rights(
    snapshot: &Snapshot,
    key_id: &str,
    slug: &str,
    agent: &str,
    now: &str,
    paths: &HostPaths,
) -> Result<(Grant, TeamMemories), String> {
    let key = snapshot
        .key(key_id)
        .filter(|key| !snapshot.barred(&key.member, now))
        .ok_or_else(|| format!("key {key_id} is not in the snapshot any more"))?;
    let (team, rank) = snapshot
        .teams
        .get(slug)
        .filter(|team| team.deleted.is_none() && key.teams.iter().any(|team| team == slug))
        .and_then(|team| Some((team, *team.members.get(&key.member)?)))
        .ok_or_else(|| format!("key {key_id} does not open {slug} any more"))?;
    let memories = TeamMemories::for_session(
        team.repository(slug, &paths.teams),
        key.store_name.clone(),
        team.projects.clone(),
    );
    let grant = Grant {
        token: key.id.clone(),
        team: slug.to_owned(),
        member: key.member.clone(),
        agent: agent.to_owned(),
        writes: if team.writable {
            Writes::Allowed
        } else {
            Writes::ReadOnlyTeam
        },
        history: rank.runs_the_team(),
        scope: None,
        limits: Limits {
            max_records: team.limits.max_records,
            max_record_bytes: team.limits.max_record_bytes,
            quota_bytes: team.limits.quota_bytes,
        },
        cabinet: None,
    };
    Ok((grant, memories))
}

/// The memory server of a team over the session's stdin and stdout: the member is the key's, the
/// agent is what the client said, versions are signed with the key's `storeName`.
///
/// A client keeps such a session open for days, so what the key may do is taken anew from the
/// snapshot in force before every request: a ban, a revocation, a removal, a demotion or the end of
/// the team's term reaches an open session with its next request, not with its next connection.
fn serve_team(key_id: &str, slug: &str, agent: &str, paths: &HostPaths) -> ExitCode {
    journal_quietly(&format!(
        "{SHELL_TAG}: key {key_id} opens the memory of {slug} for {agent}"
    ));
    let mut rights = || {
        let snapshot = read_in_force(&paths.access, journal_quietly)?;
        let now = vibememory_cli::clock::now();
        team_rights(&snapshot, key_id, slug, agent, &now, paths).inspect_err(|why| {
            journal_quietly(&format!("{SHELL_TAG}: session of key {key_id} ends: {why}"));
        })
    };
    let stdin = std::io::stdin();
    let served = protocol::serve_checked_lines(
        stdin.lock(),
        std::io::stdout(),
        &mut rights,
        &|(grant, memories): &(Grant, TeamMemories), request| {
            let response = protocol::handle(request, &grant.caller(), memories);
            if let Some(line) =
                http::journal_line(&http::call_note(request, response.as_ref(), grant), "ssh")
            {
                journal_quietly(&line);
            }
            response
        },
        &mut |_, _| {},
    );
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("vibememory-mcp: {problem}");
            ExitCode::FAILURE
        }
    }
}

/// `status`: the host's report as the key may see it.
fn answer_status(snapshot: &Snapshot, key: &str, paths: &HostPaths) -> ExitCode {
    let report_file = layout::report_file(&paths.access);
    let report = match read_json::<HostReport>(&report_file) {
        Ok(Some(report)) => report,
        Ok(None) => {
            return refuse(
                "statusUnavailable",
                &["the host has not made its report yet; it does so every hour"],
            );
        }
        Err(why) => {
            journal_quietly(&format!(
                "{SHELL_TAG}: the host report cannot be read: {why}"
            ));
            return refuse("statusUnavailable", &["the host's report cannot be read"]);
        }
    };
    let Some(answer) = status::answer(snapshot, &report, key) else {
        return refuse("unknownKey", &["the key is not in the snapshot"]);
    };
    match serde_json::to_string_pretty(&answer) {
        Ok(text) => {
            let mut out = std::io::stdout().lock();
            if writeln!(out, "{text}").and_then(|()| out.flush()).is_err() {
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(error) => refuse("hostFailure", &[&error.to_string()]),
    }
}

// ------------------------------------------------------------------------------------ pre-receive

/// `pre-receive`: decides a push to the team store the hook runs in. Git runs the hook in the
/// repository's directory, with the push's objects in quarantine until the hook says yes.
#[must_use]
pub fn pre_receive(paths: &HostPaths, limits: PushLimits) -> ExitCode {
    let key = std::env::var(KEY_VARIABLE).ok();
    let repo = match std::env::current_dir().and_then(std::fs::canonicalize) {
        Ok(repo) => repo,
        Err(error) => return refuse("hostFailure", &[&format!("no repository: {error}")]),
    };
    let name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let team = match team_dir(&name) {
        Some(TeamDir::Store(slug)) => slug,
        _ => name,
    };
    let mut input = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut input) {
        return refuse("hostFailure", &[&format!("the update lines: {error}")]);
    }
    let updates = match receive::parse_updates(&input) {
        Ok(updates) => updates,
        Err(why) => return refuse("hostFailure", &[&why]),
    };
    let changed = match receive::main_update(&updates).map(|update| changed_paths(&repo, update)) {
        None => Vec::new(),
        Some(Ok(changed)) => changed,
        Some(Err(why)) => return refuse("hostFailure", &[&why]),
    };
    let quarantine = std::env::var_os("GIT_QUARANTINE_PATH")
        .map(PathBuf::from)
        .and_then(|path| std::fs::canonicalize(path).ok());
    // A disk that cannot be measured is treated as full: the reserve is kept either way.
    let free_bytes = disk(&repo).map_or(0, |disk| disk.free_bytes);
    let sizes = Sizes {
        repo_bytes: dir_size(&repo, quarantine.as_deref()),
        incoming_bytes: quarantine.as_deref().map_or(0, |dir| dir_size(dir, None)),
        free_bytes,
        reserve_bytes: limits.reserve_bytes,
    };
    let snapshot = read_in_force(&paths.access, journal_quietly).ok();
    let now = vibememory_cli::clock::now();
    let mut push = Push {
        key: key.as_deref(),
        team: &team,
        updates: &updates,
        changed: &changed,
        sizes,
        now: &now,
        objects: Objects::Unread,
    };
    let who = who_of(key.as_deref());
    // Every other rule first: the objects are read only for a push that could otherwise land
    if let Err(refusal) = receive::decide(&push, snapshot.as_ref()) {
        return refuse_push(who, &team, &refusal);
    }
    let scanned = match receive::main_update(&updates) {
        None => Scanned::Found(Vec::new()),
        Some(update) => match push_scan::scan(&repo, &update.new, limits.max_content_bytes) {
            Ok(scanned) => scanned,
            Err(why) => return refuse("hostFailure", &[&why]),
        },
    };
    push.objects = match &scanned {
        Scanned::Found(found) => Objects::Read(found),
        Scanned::TooLarge { bytes } => Objects::TooLarge {
            bytes: *bytes,
            limit: limits.max_content_bytes,
        },
    };
    match receive::decide(&push, snapshot.as_ref()) {
        Ok(()) => {
            journal_quietly(&format!(
                "{HOOK_TAG}: key {who} pushes to {team}: {} paths, {} bytes",
                changed.len(),
                sizes.incoming_bytes
            ));
            ExitCode::SUCCESS
        }
        Err(refusal) => refuse_push(who, &team, &refusal),
    }
}

/// What bounds a push besides the snapshot's rules.
#[derive(Debug, Clone, Copy)]
pub struct PushLimits {
    /// Free space a push may not take from the partition every team shares.
    pub reserve_bytes: u64,
    /// What the push's objects may hold, uncompressed, for the hook to read them for tokens.
    pub max_content_bytes: u64,
}

/// The key a push came with, as the journal names it.
fn who_of(key: Option<&str>) -> &str {
    key.unwrap_or("-")
}

/// Journals a refused push and answers it: the code, then the paths or what is wrong.
fn refuse_push(who: &str, team: &str, refusal: &receive::PushRefusal) -> ExitCode {
    journal_quietly(&format!(
        "{HOOK_TAG}: key {who} refused in {team} ({}): {}",
        refusal.code, refusal.detail
    ));
    if refusal.paths.is_empty() {
        refuse(refusal.code, &[&refusal.detail])
    } else {
        let paths: Vec<&str> = refusal.paths.iter().map(String::as_str).collect();
        refuse(refusal.code, &paths)
    }
}

/// The paths a push changes on `main`: the difference of the trees before and after, not every
/// commit — what the push leaves behind is what members check out.
fn changed_paths(repo: &Path, update: &Update) -> Result<Vec<String>, String> {
    let old = if receive::is_zero(&update.old) {
        // A push that creates `main` changes everything against nothing: git's empty tree, of the
        // repository's own hash.
        let empty = git_memories::run(
            repo,
            &["hash-object", "-t", "tree", "--stdin"],
            Some(b""),
            &[],
        )?;
        String::from_utf8_lossy(&empty).trim().to_owned()
    } else {
        update.old.clone()
    };
    let listed = git_memories::run(
        repo,
        &[
            "diff-tree",
            "-r",
            "--name-only",
            "--no-renames",
            "-z",
            &old,
            &update.new,
        ],
        None,
        &[],
    )?;
    Ok(listed
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

// ----------------------------------------------------------------------------------- access-apply

/// Where `access-apply` writes besides the teams: `vmgit`'s keys, and the script that sets a
/// store up.
#[derive(Debug, Clone)]
pub struct ApplyPaths {
    /// `vmgit`'s `authorized_keys`.
    pub authorized_keys: PathBuf,
    /// `storeInit.sh`.
    pub store_init: PathBuf,
}

/// `access-apply`: brings the host in line with the snapshot — team stores, `vmgit`'s keys — and
/// writes `applied.json` and `host.json`.
///
/// A snapshot published while a run is on would be merged by systemd into that run and wait for
/// the next publication, so a run that finds the file changed under it applies it again.
///
/// `catch_up` is the timer's run: it does nothing when the file is the snapshot applied last and
/// that application failed at nothing. An application that failed is otherwise retried only by the
/// next change of the file, which may be days away. The two runs are separate units, so the whole
/// application holds its lock: the second waits and then finds the host caught up.
#[must_use]
pub fn access_apply(paths: &HostPaths, apply_paths: &ApplyPaths, catch_up: bool) -> ExitCode {
    let _lock = match hold_lock(&layout::apply_lock_file(&paths.teams)) {
        Ok(lock) => lock,
        Err(why) => {
            journal(&format!(
                "{APPLY_TAG}: the application cannot take its lock: {why}"
            ));
            return ExitCode::FAILURE;
        }
    };
    if catch_up && caught_up(paths) {
        return ExitCode::SUCCESS;
    }
    let mut outcome = ExitCode::FAILURE;
    for _ in 0..APPLY_PASSES {
        let (code, applied) = apply_once(paths, apply_paths);
        outcome = code;
        if std::fs::read(&paths.access).ok() == applied {
            break;
        }
        journal(&format!(
            "{APPLY_TAG}: the snapshot changed while it was applied; applying it again"
        ));
    }
    outcome
}

/// Takes the exclusive lock on the file at `path`, waiting for its holder; it is held until the
/// returned file is dropped.
fn hold_lock(path: &Path) -> Result<std::fs::File, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    lock.lock()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(lock)
}

/// Whether the host already did what it can with the file as it is now: applied it and failed at
/// nothing, or refused these very bytes — or the missing file — for the snapshot itself, which a
/// second run would refuse the same way. A run that could not do its work is tried again.
fn caught_up(paths: &HostPaths) -> bool {
    let hash = std::fs::read(&paths.access)
        .ok()
        .map(|bytes| vibememory_cli::sha256::hex(&bytes));
    let Ok(Some(applied)) = read_json::<Applied>(&layout::applied_file(&paths.access)) else {
        return false;
    };
    let applied_whole = hash.as_deref() == Some(applied.snapshot_hash.as_str())
        && applied.last_refused.is_none()
        && applied
            .problems
            .iter()
            .all(|problem| problem.code != APPLY_FAILED);
    let refused_already = applied
        .last_refused
        .as_ref()
        .is_some_and(|refused| refused.snapshot_hash == hash && refused.code != APPLY_FAILED);
    applied_whole || refused_already
}

/// One application of the snapshot as it is now; returns the bytes it applied or refused.
///
/// A snapshot not newer than the one applied last is refused: a publication that lost its turn,
/// or a file put back by hand, is an older decision and does not take the host back. Once the team
/// stores are listed, the copy of what is applied is written before anything else, so a late older
/// file is out of force from here on; a run that cannot even list them applies nothing and says
/// so, and the timer's next run tries again.
fn apply_once(paths: &HostPaths, apply_paths: &ApplyPaths) -> (ExitCode, Option<Vec<u8>>) {
    let applied_file = layout::applied_file(&paths.access);
    let (bytes, snapshot) = match read_checked(&paths.access) {
        Ok((bytes, Ok(snapshot))) => (bytes, snapshot),
        Ok((bytes, Err(why))) => {
            return refused(paths, &applied_file, SNAPSHOT_REJECTED, why, Some(bytes));
        }
        Err(why) => return refused(paths, &applied_file, SNAPSHOT_REJECTED, why, None),
    };
    if let Err(why) = newer_than_applied(&paths.access, &snapshot, &bytes) {
        return refused(paths, &applied_file, SERIAL_BEHIND, why, Some(bytes));
    }
    let dirs = match directory_names(&paths.teams) {
        Ok(dirs) => dirs,
        Err(why) => {
            let why = format!("the team stores cannot be listed: {why}");
            return refused(paths, &applied_file, APPLY_FAILED, why, Some(bytes));
        }
    };
    let mut problems = Vec::new();
    let mut failed = !keep_applied_copy(&paths.access, &bytes, &mut problems);
    let plan = apply::plan(&snapshot, &dirs);
    for problem in &plan.problems {
        journal(&format!(
            "{APPLY_TAG}: {} {}: {}",
            problem.code,
            problem.team.as_deref().unwrap_or("-"),
            problem.detail
        ));
    }
    problems.extend(plan.problems);
    failed |= run_steps(&plan.steps, &snapshot, paths, apply_paths, &mut problems);
    for (slug, team) in snapshot.teams.iter().filter(|(_, team)| team.adopted) {
        let repo = team.repository(slug, &paths.teams);
        if !repo.is_dir() {
            problems.push(Problem {
                code: REPO_MISSING.to_owned(),
                team: Some(slug.clone()),
                detail: format!("{} is not a repository", repo.display()),
            });
        }
    }
    if let Err(why) = write_keys(&apply_paths.authorized_keys, &snapshot) {
        failed = true;
        journal(&format!("{APPLY_TAG}: authorized_keys: FAILED: {why}"));
        problems.push(Problem {
            code: APPLY_FAILED.to_owned(),
            team: None,
            detail: why,
        });
    }
    let applied = Applied {
        version: status::VERSION,
        serial: snapshot.serial,
        snapshot_hash: vibememory_cli::sha256::hex(&bytes),
        applied_at: vibememory_cli::clock::now(),
        team_count: snapshot.team_count,
        problems,
        last_refused: None,
    };
    if let Err(why) = write_json(&applied_file, &applied) {
        failed = true;
        journal(&format!("{APPLY_TAG}: applied.json: FAILED: {why}"));
    } else {
        journal(&format!(
            "{APPLY_TAG}: snapshot {} of serial {} applied: {} teams, {} machine keys, {} problems",
            applied.snapshot_hash,
            applied.serial,
            snapshot.teams.len(),
            snapshot.keys.len(),
            applied.problems.len()
        ));
    }
    report_after(paths);
    let code = if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };
    (code, Some(bytes))
}

/// Whether `snapshot` (`bytes`) is newer than the one the host applied last; why not, when it is
/// not. The copy says so first; `applied.json` too, for a run whose copy could not be written.
fn newer_than_applied(access: &Path, snapshot: &Snapshot, bytes: &[u8]) -> Result<(), String> {
    if let Ok(Some(applied)) = read_json::<Applied>(&layout::applied_file(access))
        && older_than_applied(
            snapshot.serial,
            Some(&vibememory_cli::sha256::hex(bytes)),
            &applied,
        )
    {
        return Err(format!(
            "serial {} is not newer than serial {} applied",
            snapshot.serial, applied.serial
        ));
    }
    let Some(last) = read_applied_snapshot(access, journal) else {
        return Ok(());
    };
    if !access::behind(snapshot, &last) {
        return Ok(());
    }
    Err(if snapshot.serial == last.serial {
        format!(
            "serial {} is the one applied, with other contents",
            snapshot.serial
        )
    } else {
        format!(
            "serial {} is older than serial {} applied",
            snapshot.serial, last.serial
        )
    })
}

/// Writes the copy of the snapshot being applied; whether it could. A copy that could not be
/// written is an `applyFailed` problem and nothing more: a newer decision is never held back by the
/// guard's bookkeeping.
fn keep_applied_copy(access: &Path, bytes: &[u8], problems: &mut Vec<Problem>) -> bool {
    let Err(why) = write_atomic(&layout::applied_snapshot_file(access), bytes, SHARED_FILE) else {
        return true;
    };
    journal(&format!(
        "{APPLY_TAG}: applied-snapshot.json: FAILED: {why}"
    ));
    problems.push(Problem {
        code: APPLY_FAILED.to_owned(),
        team: None,
        detail: why,
    });
    false
}

/// A snapshot that is not applied: nothing on the host changes, and the reports say so under
/// `code`.
fn refused(
    paths: &HostPaths,
    applied_file: &Path,
    code: &str,
    why: String,
    read: Option<Vec<u8>>,
) -> (ExitCode, Option<Vec<u8>>) {
    journal(&format!(
        "{APPLY_TAG}: the snapshot is refused ({code}), nothing is applied: {why}"
    ));
    let refusal = Refused {
        snapshot_hash: read.as_deref().map(vibememory_cli::sha256::hex),
        code: code.to_owned(),
    };
    keep_previous_with_rejection(applied_file, refusal, why);
    report_after(paths);
    (ExitCode::FAILURE, read)
}

/// Does what the plan says, one team after another: a failure of one does not stop the others.
/// Each failure becomes an `applyFailed` problem; returns whether there was one.
fn run_steps(
    steps: &[Step],
    snapshot: &Snapshot,
    paths: &HostPaths,
    apply_paths: &ApplyPaths,
    problems: &mut Vec<Problem>,
) -> bool {
    let mut failed = false;
    for step in steps {
        let (slug, outcome) = match step {
            Step::Create { slug } | Step::Keep { slug } => {
                let repo = paths.teams.join(store_dir(slug));
                let settled = settle_store(&repo, slug, &apply_paths.store_init);
                // a team whose sessions are off keeps its memory only
                let sessions_off = snapshot
                    .teams
                    .get(slug)
                    .is_some_and(|team| team.mode == Some(crate::access::Mode::Memory));
                let outcome = match settled {
                    Ok(done) if sessions_off => {
                        purge_sessions(&repo, slug).map(|purged| match (done, purged) {
                            (Some(done), Some(purged)) => Some(format!("{done}; {purged}")),
                            (done, purged) => done.or(purged),
                        })
                    }
                    other => other,
                };
                (slug, outcome)
            }
            Step::Retire { slug, to } => (slug, retire(&paths.teams, slug, to)),
        };
        match outcome {
            Ok(Some(done)) => journal(&format!("{APPLY_TAG}: {slug}: {done}")),
            Ok(None) => {}
            Err(why) => {
                failed = true;
                journal(&format!("{APPLY_TAG}: {slug}: FAILED: {why}"));
                problems.push(Problem {
                    code: APPLY_FAILED.to_owned(),
                    team: Some(slug.clone()),
                    detail: why,
                });
            }
        }
    }
    failed
}

/// A refused snapshot changes nothing on the host, and the last application says so: its values
/// stay, and the refusal — `snapshotRejected`, `serialBehind` or an `applyFailed` that stopped the
/// run, the latest one only — stands first among its problems. Before the first application there
/// is nothing to keep, and nothing is written.
fn keep_previous_with_rejection(applied_file: &Path, refusal: Refused, why: String) {
    let previous = match read_json::<Applied>(applied_file) {
        Ok(Some(previous)) => previous,
        Ok(None) => return,
        Err(error) => {
            journal(&format!("{APPLY_TAG}: applied.json: {error}"));
            return;
        }
    };
    let mut applied = previous;
    // The earlier refusal gives way to this one; what failed for one team stays until an
    // application of that team does it
    applied.problems.retain(|problem| {
        problem.code != SNAPSHOT_REJECTED
            && problem.code != SERIAL_BEHIND
            && !(problem.code == refusal.code && problem.team.is_none())
    });
    applied.problems.insert(
        0,
        Problem {
            code: refusal.code.clone(),
            team: None,
            detail: why,
        },
    );
    applied.last_refused = Some(refusal);
    if let Err(error) = write_json(applied_file, &applied) {
        journal(&format!("{APPLY_TAG}: applied.json: FAILED: {error}"));
    }
}

/// Settings and hook of a team store from `storeInit.sh` — which also creates a missing one —
/// then `.gitattributes` on `main` brought to the engine's text: the first commit of a new store,
/// a new commit on top when the engine's text changed. Says what changed, if anything.
fn settle_store(repo: &Path, slug: &str, store_init: &Path) -> Result<Option<String>, String> {
    let existed = repo.is_dir();
    let output = Command::new(store_init)
        .arg(repo)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{} could not be started: {error}", store_init.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: {}",
            store_init.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let committed = GitMemories::new(repo.to_path_buf(), APPLY_WRITER.to_owned()).settle_file(
        GITATTRIBUTES_PATH,
        vibememory_cli::install::GITATTRIBUTES.as_bytes(),
        &format!("vibememory: {GITATTRIBUTES_PATH} of team {slug}"),
    )?;
    Ok(match (existed, committed) {
        (false, _) => Some("store created with its first commit".to_owned()),
        (true, true) => Some(format!("{GITATTRIBUTES_PATH} brought to the engine's text")),
        (true, false) => None,
    })
}

/// Takes a team's sessions off its store once they are switched off: `main` rewritten to its memory
/// and the host's own files, the generation raised, the old history expired and packed away — git
/// gives the room back only then. Says what it did, if anything.
fn purge_sessions(repo: &Path, slug: &str) -> Result<Option<String>, String> {
    let git = GitMemories::new(repo.to_path_buf(), APPLY_WRITER.to_owned());
    let Some(generation) = git.rewrite_keeping(
        crate::purge::kept_without_sessions,
        &format!("vibememory: sessions of team {slug} switched off"),
    )?
    else {
        return Ok(None);
    };
    git_memories::run(
        repo,
        &["reflog", "expire", "--expire=now", "--all"],
        None,
        &[],
    )?;
    git_memories::run(repo, &["gc", "--prune=now", "--quiet"], None, &[])?;
    Ok(Some(format!(
        "sessions switched off: history rewritten to memory, generation {generation}"
    )))
}

/// Renames a deleted team's store; nothing of it is removed.
fn retire(teams: &Path, slug: &str, to: &str) -> Result<Option<String>, String> {
    std::fs::rename(teams.join(store_dir(slug)), teams.join(to))
        .map_err(|error| format!("rename to {to}: {error}"))?;
    Ok(Some(format!("deleted, store renamed to {to}")))
}

/// `vmgit`'s `authorized_keys`, written whole: sshd reads it on every connection.
fn write_keys(path: &Path, snapshot: &Snapshot) -> Result<(), String> {
    if let Some(dir) = path.parent()
        && !dir.is_dir()
    {
        std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(PRIVATE_DIR))
                .map_err(|error| format!("{}: {error}", dir.display()))?;
        }
    }
    #[cfg(not(unix))]
    let _ = PRIVATE_DIR;
    write_atomic(
        path,
        apply::authorized_keys(snapshot, layout::BINARY).as_bytes(),
        PRIVATE_FILE,
    )
}

// ----------------------------------------------------------------------------------------- status

/// `status`: writes `host.json`.
#[must_use]
pub fn status(paths: &HostPaths) -> ExitCode {
    match write_report(paths) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            journal(&format!("{STATUS_TAG}: host.json is not written: {why}"));
            ExitCode::FAILURE
        }
    }
}

/// The report after an application: its failure does not undo what was applied.
fn report_after(paths: &HostPaths) {
    if let Err(why) = write_report(paths) {
        journal(&format!("{STATUS_TAG}: host.json is not written: {why}"));
    }
}

/// Builds `host.json` from the snapshot, `applied.json` and what is on disk, and writes it.
///
/// The application and the hourly report are separate units and may run at once; each holds the
/// lock from reading `applied.json` to writing the report, so a slow report never lands after a
/// fresh one with an older application in it.
fn write_report(paths: &HostPaths) -> Result<(), String> {
    let _lock = hold_lock(&layout::report_lock_file(&paths.teams))?;
    let snapshot = match read_in_force(&paths.access, journal) {
        Ok(snapshot) => Some(snapshot),
        Err(why) => {
            journal(&format!(
                "{STATUS_TAG}: the snapshot cannot be used, teams and orphans are not told apart: {why}"
            ));
            None
        }
    };
    let applied = read_json::<Applied>(&layout::applied_file(&paths.access))?;
    let facts = gather(paths, snapshot.as_ref())?;
    let report = status::host_report(snapshot.as_ref(), applied, facts);
    write_json(&layout::report_file(&paths.access), &report)
}

/// What the report is made of.
fn gather(paths: &HostPaths, snapshot: Option<&Snapshot>) -> Result<Facts, String> {
    let mut repos = BTreeMap::new();
    let activity = layout::activity_dir(&paths.teams);
    for name in directory_names(&paths.teams)? {
        let mut facts = repo_facts(&paths.teams.join(&name));
        if let Some(layout::TeamDir::Store(slug)) = layout::team_dir(&name) {
            facts.last_access_at = last_access(&activity, &slug);
        }
        repos.insert(name, facts);
    }
    let mut adopted = BTreeMap::new();
    for (slug, team) in snapshot
        .iter()
        .flat_map(|snapshot| snapshot.teams.iter())
        .filter(|(_, team)| team.adopted)
    {
        let repo = team.repository(slug, &paths.teams);
        if repo.is_dir() {
            let mut facts = repo_facts(&repo);
            facts.last_access_at = last_access(&activity, slug);
            adopted.insert(slug.clone(), facts);
        }
    }
    let backup = match read_json::<Backup>(&layout::backup_file(&paths.access)) {
        Ok(backup) => backup,
        Err(why) => {
            // Reported as no backup, which the cabinet raises an alarm for anyway.
            journal(&format!("{STATUS_TAG}: backup.json: {why}"));
            None
        }
    };
    Ok(Facts {
        generated_at: vibememory_cli::clock::now(),
        repos,
        adopted,
        host_keys: host_keys(),
        disk: disk(&paths.teams)?,
        services: service_states(),
        backup,
    })
}

/// How long an archive waits on the host for the cabinet to hand it out.
const EXPORT_KEEP: std::time::Duration = std::time::Duration::from_hours(30 * 24);
/// The tag of the export's lines in the journal.
const EXPORT_TAG: &str = "vibememory-export";

/// A request the cabinet leaves in the exports directory: `<id>.request` naming the team.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportRequest {
    slug: String,
}

/// Whether a request's id is one the cabinet makes: a uuid, so a name can never climb out of the
/// directory or collide with an archive of another request.
fn is_request_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

/// Builds the archives the cabinet asked for and prunes the old ones.
///
/// Every `<id>.request` without its `<id>.zip` gets one: the memory of the named live team, written
/// to a part file and renamed, so the cabinet never reads half an archive. The request stays — it is
/// the cabinet's file, and the directory's sticky bit keeps each side to its own; the cabinet takes
/// it away once the archive is there. An archive older than [`EXPORT_KEEP`] is removed.
#[must_use]
pub fn export(paths: &HostPaths) -> ExitCode {
    let dir = layout::exports_dir(&paths.teams);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) => {
            journal(&format!("{EXPORT_TAG}: {}: {error}", dir.display()));
            return ExitCode::FAILURE;
        }
    };
    let mut failed = false;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        // the archives are named by this command alone, in lower case
        if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension == "zip")
        {
            let old = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > EXPORT_KEEP);
            if old && std::fs::remove_file(entry.path()).is_ok() {
                journal(&format!(
                    "{EXPORT_TAG}: {name} kept its time and is removed"
                ));
            }
            continue;
        }
        let Some(id) = name.strip_suffix(".request") else {
            continue;
        };
        if !is_request_id(id) || dir.join(format!("{id}.zip")).exists() {
            continue;
        }
        match build_archive(paths, &dir, id, &entry.path()) {
            Ok(slug) => journal(&format!("{EXPORT_TAG}: archive {id} of {slug} is ready")),
            Err(why) => {
                journal(&format!("{EXPORT_TAG}: archive {id}: {why}"));
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// One archive: the team's journals and their markdown, in `<id>.zip`, readable by the cabinet.
fn build_archive(
    paths: &HostPaths,
    dir: &Path,
    id: &str,
    request: &Path,
) -> Result<String, String> {
    let bytes = std::fs::read(request).map_err(|error| error.to_string())?;
    let ExportRequest { slug } =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if !crate::access::is_name(&slug) {
        return Err(format!("{slug:?} is no team's name"));
    }
    let repo = paths.teams.join(layout::store_dir(&slug));
    if !repo.is_dir() {
        return Err(format!("team {slug} has no live store"));
    }
    let git = GitMemories::new(repo, APPLY_WRITER.to_owned());
    let mut journals = Vec::new();
    for project in crate::memories::Memories::projects(&git)? {
        let journal = git.journal_bytes(&project)?;
        journals.push((project, journal));
    }
    let files = crate::export::archive_files(&slug, &vibememory_cli::clock::now(), &journals);
    let part = dir.join(format!("{id}.zip.part"));
    let file = std::fs::File::create(&part).map_err(|error| error.to_string())?;
    crate::export::write_zip(file, &files)?;
    // the cabinet reads it through the shared group
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o640))
            .map_err(|error| error.to_string())?;
    }
    std::fs::rename(&part, dir.join(format!("{id}.zip"))).map_err(|error| error.to_string())?;
    Ok(slug)
}

/// When the memory server last touched the team's activity file.
fn last_access(activity: &Path, slug: &str) -> Option<String> {
    let modified = std::fs::metadata(activity.join(slug))
        .ok()?
        .modified()
        .ok()?;
    let seconds = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(seconds)
        .ok()
        .map(vibememory_cli::clock::iso8601)
}

/// Size, projects, last commit and the `machines/` hint of one repository.
fn repo_facts(repo: &Path) -> RepoFacts {
    let git = GitMemories::new(repo.to_path_buf(), APPLY_WRITER.to_owned());
    let size_bytes = dir_size(repo, None);
    let Ok(Some(main)) = git.main_commit() else {
        return RepoFacts {
            size_bytes,
            projects: Vec::new(),
            last_commit_at: None,
            machines: false,
            project_facts: BTreeMap::new(),
            last_access_at: None,
        };
    };
    let text = |args: &[&str]| {
        git_memories::run(repo, args, None, &[])
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
    };
    let projects = crate::memories::Memories::projects(&git).unwrap_or_default();
    let last_commit_at = text(&["log", "-1", "--format=%ct", &main])
        .ok()
        .and_then(|seconds| seconds.parse::<i64>().ok())
        .map(vibememory_cli::clock::iso8601);
    let machines = text(&["ls-tree", "-d", "--name-only", &main, "machines"])
        .is_ok_and(|listed| !listed.is_empty());
    let project_facts = project_facts(&text, &main, &projects);
    RepoFacts {
        size_bytes,
        projects,
        last_commit_at,
        machines,
        project_facts,
        last_access_at: None,
    }
}

/// Size, last commit and its author of each project of `main`. The size is one listing of the
/// tree, not a walk of history; the last commit is one `log -1` per project, hourly.
fn project_facts(
    text: &dyn Fn(&[&str]) -> Result<String, String>,
    main: &str,
    projects: &[String],
) -> BTreeMap<String, ProjectFacts> {
    let mut sizes: BTreeMap<&str, u64> = BTreeMap::new();
    let mut memory: BTreeMap<&str, u64> = BTreeMap::new();
    if let Ok(listed) = text(&["ls-tree", "-r", "-l", main, "--", "projects/"]) {
        // `<mode> <type> <object> <size>\t<path>`; a submodule has `-` for a size and adds nothing
        for line in listed.lines() {
            let Some((meta, path)) = line.split_once('\t') else {
                continue;
            };
            let size = meta
                .split_whitespace()
                .nth(3)
                .and_then(|size| size.parse::<u64>().ok());
            let Some((project, inside)) = path
                .strip_prefix("projects/")
                .and_then(|rest| rest.split_once('/'))
            else {
                continue;
            };
            if let Some(size) = size
                && let Some(known) = projects.iter().find(|known| known.as_str() == project)
            {
                *sizes.entry(known.as_str()).or_default() += size;
                if is_memory_part(inside) {
                    *memory.entry(known.as_str()).or_default() += size;
                }
            }
        }
    }
    projects
        .iter()
        .map(|project| {
            let path = format!("projects/{project}/");
            let last = text(&["log", "-1", "--format=%ct%x09%an", main, "--", &path]).ok();
            let (seconds, author) = last
                .as_deref()
                .and_then(|line| line.split_once('\t'))
                .map_or((None, None), |(seconds, author)| {
                    (seconds.parse::<i64>().ok(), Some(author.to_owned()))
                });
            let facts = ProjectFacts {
                size_bytes: sizes.get(project.as_str()).copied().unwrap_or_default(),
                memory_bytes: Some(memory.get(project.as_str()).copied().unwrap_or_default()),
                last_commit_at: seconds.map(vibememory_cli::clock::iso8601),
                last_author: author.filter(|author| !author.is_empty()),
            };
            (project.clone(), facts)
        })
        .collect()
}

/// Whether a path inside a project is its memory: the journal and the `memory/` directory the
/// memory server writes and the agents read. Everything else there is a session — a transcript and
/// the directory of its subagents and tool results.
fn is_memory_part(inside: &str) -> bool {
    inside == MEMORY_JOURNAL || inside.starts_with(MEMORY_DIR_PREFIX)
}

/// The memory journal of a project, relative to the project.
const MEMORY_JOURNAL: &str = "memory.jsonl";

/// The memory directory of a project, relative to the project, with its separator.
const MEMORY_DIR_PREFIX: &str = "memory/";

/// The host's public keys: type and base64 of each `ssh_host_*_key.pub`, without the comment.
fn host_keys() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(layout::HOST_KEYS_DIR) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("ssh_host_") && name.ends_with("_key.pub")
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| {
            let mut fields = text.split_whitespace();
            Some(format!("{} {}", fields.next()?, fields.next()?))
        })
        .collect();
    keys.sort();
    keys
}

/// `systemctl is-active` of each service the report names.
fn service_states() -> BTreeMap<String, String> {
    layout::SERVICES
        .iter()
        .map(|name| {
            let state = Command::new("systemctl")
                .args(["is-active", name])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .filter(|state| !state.is_empty())
                .unwrap_or_else(|| "unknown".to_owned());
            ((*name).to_owned(), state)
        })
        .collect()
}
