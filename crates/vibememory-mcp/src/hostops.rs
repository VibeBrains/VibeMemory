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
use crate::git_memories::{self, GitMemories};
use crate::host::TeamMemories;
use crate::http::{self, Grant};
use crate::layout::{self, TeamDir, store_dir, team_dir};
use crate::protocol;
use crate::receive::{self, Push, Sizes, Update};
use crate::shell::{self, ShellAction};
use crate::status::{self, Applied, Backup, Disk, Facts, HostReport, Problem, RepoFacts};
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

/// The snapshot at `access`, checked.
fn read_snapshot(access: &Path) -> Result<Snapshot, String> {
    read_checked(access).and_then(|(_, checked)| checked)
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
/// # Errors
///
/// The file cannot be read or breaks a rule.
pub fn read_in_force(access: &Path, note: fn(&str)) -> Result<Snapshot, String> {
    let file = read_snapshot(access)?;
    Ok(access::in_force(file, read_applied_snapshot(access, note)))
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

/// Bytes of every file under `root`, symbolic links not followed, `skip` left out.
fn dir_size(root: &Path, skip: Option<&Path>) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if skip.is_some_and(|skip| path == skip) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
            } else {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
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
pub fn pre_receive(paths: &HostPaths, reserve_bytes: u64) -> ExitCode {
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
        reserve_bytes,
    };
    let with_tokens = match receive::main_update(&updates) {
        Some(update) => match paths_with_tokens(&repo, &update.new, &changed) {
            Ok(found) => found,
            Err(why) => return refuse("hostFailure", &[&why]),
        },
        None => Vec::new(),
    };
    let snapshot = read_in_force(&paths.access, journal_quietly).ok();
    let now = vibememory_cli::clock::now();
    let push = Push {
        key: key.as_deref(),
        team: &team,
        updates: &updates,
        changed: &changed,
        sizes,
        now: &now,
        with_tokens: &with_tokens,
    };
    let who = key.as_deref().unwrap_or("-");
    match receive::decide(&push, snapshot.as_ref()) {
        Ok(()) => {
            journal_quietly(&format!(
                "{HOOK_TAG}: key {who} pushes to {team}: {} paths, {} bytes",
                changed.len(),
                sizes.incoming_bytes
            ));
            ExitCode::SUCCESS
        }
        Err(refusal) => {
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
    }
}

/// Of `changed`, the paths whose content at `commit` holds an agent token of a cabinet, read in one
/// `git cat-file --batch`; a path the push deletes has no content and holds nothing.
fn paths_with_tokens(repo: &Path, commit: &str, changed: &[String]) -> Result<Vec<String>, String> {
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let mut asked = String::new();
    for path in changed {
        asked.push_str(commit);
        asked.push(':');
        asked.push_str(path);
        asked.push('\n');
    }
    let output = git_memories::run(repo, &["cat-file", "--batch"], Some(asked.as_bytes()), &[])?;
    let mut found = Vec::new();
    let mut rest = output.as_slice();
    for path in changed {
        let end = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| "git cat-file answered short".to_owned())?;
        let header = String::from_utf8_lossy(rest.get(..end).unwrap_or_default()).into_owned();
        rest = rest.get(end + 1..).unwrap_or_default();
        if header.ends_with(" missing") {
            continue;
        }
        let size: usize = header
            .rsplit(' ')
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(|| format!("git cat-file answered {header:?}"))?;
        let content = rest
            .get(..size)
            .ok_or_else(|| "git cat-file answered short".to_owned())?;
        if vibememory_core::token::holds_token(content) {
            found.push(path.clone());
        }
        // the content, then the newline that ends it
        rest = rest.get(size + 1..).unwrap_or_default();
    }
    Ok(found)
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
    let _lock = match hold_lock(&layout::apply_lock_file(&paths.access)) {
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

/// Whether the last application was of the file as it is now and did everything it had to.
fn caught_up(paths: &HostPaths) -> bool {
    let Ok(bytes) = std::fs::read(&paths.access) else {
        return false;
    };
    matches!(
        read_json::<Applied>(&layout::applied_file(&paths.access)),
        Ok(Some(applied)) if applied.snapshot_hash == vibememory_cli::sha256::hex(&bytes)
            && applied.problems.iter().all(|problem| problem.code != APPLY_FAILED)
    )
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
    failed |= run_steps(&plan.steps, paths, apply_paths, &mut problems);
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
        && (snapshot.serial < applied.serial
            || (snapshot.serial > 0
                && snapshot.serial == applied.serial
                && vibememory_cli::sha256::hex(bytes) != applied.snapshot_hash))
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
    keep_previous_with_rejection(applied_file, code, why);
    report_after(paths);
    (ExitCode::FAILURE, read)
}

/// Does what the plan says, one team after another: a failure of one does not stop the others.
/// Each failure becomes an `applyFailed` problem; returns whether there was one.
fn run_steps(
    steps: &[Step],
    paths: &HostPaths,
    apply_paths: &ApplyPaths,
    problems: &mut Vec<Problem>,
) -> bool {
    let mut failed = false;
    for step in steps {
        let (slug, outcome) = match step {
            Step::Create { slug } | Step::Keep { slug } => (
                slug,
                settle_store(
                    &paths.teams.join(store_dir(slug)),
                    slug,
                    &apply_paths.store_init,
                ),
            ),
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
fn keep_previous_with_rejection(applied_file: &Path, code: &str, why: String) {
    let previous = match read_json::<Applied>(applied_file) {
        Ok(Some(previous)) => previous,
        Ok(None) => return,
        Err(error) => {
            journal(&format!("{APPLY_TAG}: applied.json: {error}"));
            return;
        }
    };
    let mut applied = previous;
    applied.problems.retain(|problem| {
        problem.code != SNAPSHOT_REJECTED && problem.code != SERIAL_BEHIND && problem.code != code
    });
    applied.problems.insert(
        0,
        Problem {
            code: code.to_owned(),
            team: None,
            detail: why,
        },
    );
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
    let _lock = hold_lock(&layout::report_lock_file(&paths.access))?;
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
    for name in directory_names(&paths.teams)? {
        let facts = repo_facts(&paths.teams.join(&name));
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
            adopted.insert(slug.clone(), repo_facts(&repo));
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
    RepoFacts {
        size_bytes,
        projects,
        last_commit_at,
        machines,
    }
}

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
