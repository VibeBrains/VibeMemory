//! Running git, and looking for `.git` ourselves.
//!
//! The core decides what a store is called; this module records the two observations that
//! decision rests on. Both are hostile territory: git may be missing, wedged on a network mount,
//! or configured by environment variables that make it answer about a different repository
//! entirely — and the walk up the tree may hit a directory nobody may read.
//!
//! Nothing here interprets what it finds. The probe is data; the rule lives in the core, where it
//! is tested on fixtures instead of on whatever repository happens to be on this disk.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use vibememory_core::naming::{DotGit, DotGitProbe, GitProbe};

/// The one `GIT_*` variable that must survive: without it a git built with a relocated exec path
/// cannot find its own subcommands, and we would report "git is missing" on a working machine.
const KEPT_GIT_VAR: &str = "GIT_EXEC_PATH";
/// Prefix of the variables that make git answer about another repository.
const GIT_VAR_PREFIX: &str = "GIT_";
/// The entry every repository has at its root.
const DOT_GIT: &str = ".git";
/// How often the wait loop looks at the child again.
const POLL: Duration = Duration::from_millis(10);

/// Asks git which common dir the working directory belongs to.
///
/// The environment is stripped of every `GIT_*` variable except [`KEPT_GIT_VAR`]: a hook inherits
/// the environment of whatever invoked it, and `GIT_DIR` or `GIT_WORK_TREE` left over from a
/// wrapper script would make git describe a repository the session has nothing to do with.
///
/// A timeout is not an error but an answer: on a hung network mount git can block for minutes,
/// and a hook that blocks is worse than a hook that says "unavailable".
#[must_use]
pub fn probe(cwd: &Path, timeout: Duration) -> GitProbe {
    let started = Instant::now();
    let common_dir = match run_rev_parse(cwd, timeout) {
        Ok(output) => output,
        Err(reason) => return GitProbe::Unavailable { reason },
    };
    // The walk is always performed, whatever git said: git silently walks past a broken or empty
    // `.git` directory to an outer repository, and then names a store the session is not in.
    let left = timeout.saturating_sub(started.elapsed());
    GitProbe::Ran {
        common_dir,
        dot_git: walk_for_dot_git(cwd, left),
    }
}

/// Builds the command, with every variable in `remove` unset for the child.
#[must_use]
pub fn rev_parse_command(cwd: &Path, remove: &[String]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("rev-parse")
        .arg("--path-format=absolute")
        .arg("--git-common-dir")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in remove {
        command.env_remove(name);
    }
    command
}

/// Runs `git rev-parse --path-format=absolute --git-common-dir`, returning its trimmed stdout when
/// the exit code was 0, `None` when git ran and refused, and `Err` when git could not be run.
fn run_rev_parse(cwd: &Path, timeout: Duration) -> Result<Option<String>, String> {
    let command = rev_parse_command(cwd, &variables_to_remove(environment_names()));
    run_with_timeout(command, timeout)
}

/// Runs a prepared command to completion or to the deadline, whichever comes first.
///
/// `Ok(Some(stdout))` when it exited 0, `Ok(None)` when it ran and refused, `Err` when it could
/// not be run at all — including the deadline, because a hook that blocks is worse than a hook
/// that says "unavailable".
///
/// # Errors
///
/// The text of what went wrong, meant for a log and for `doctor`.
pub fn run_with_timeout(command: Command, timeout: Duration) -> Result<Option<String>, String> {
    run_capturing(command, timeout).map(Result::ok)
}

/// [`run_with_timeout`], keeping what a refusing command said: `Ok(Ok(stdout))` when it exited 0,
/// `Ok(Err(stderr))` when it ran and refused. For a caller that has to tell a person why — a probe
/// whose whole answer is the shell's "command not found".
///
/// # Errors
///
/// As [`run_with_timeout`].
pub fn run_capturing(
    mut command: Command,
    timeout: Duration,
) -> Result<Result<String, String>, String> {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Err(format!("git could not be started: {error}")),
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => return Err(format!("waiting for git failed: {error}")),
        }
        if Instant::now() >= deadline {
            // The child is killed and reaped: a hook that leaves a wedged git behind would leak
            // one process per session.
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "git did not answer within {}ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(POLL);
    }

    // The output of the commands run here is a path or a short list of names, far below the pipe
    // buffer, so reading after the exit cannot deadlock.
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => return Err(format!("reading git output failed: {error}")),
    };
    if !output.status.success() {
        return Ok(Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .to_owned()));
    }
    Ok(Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_owned()))
}

/// Runs a prepared command, writing `stdin_bytes` to its input first.
///
/// Used for `hash-object -w --stdin`, which is how a snapshot of a live file enters the object
/// database without `git add` ever looking at the file itself.
///
/// # Errors
///
/// The text of what went wrong; the deadline counts as a failure, as everywhere here.
pub fn run_with_input(
    mut command: Command,
    stdin_bytes: &[u8],
    timeout: Duration,
) -> Result<Option<String>, String> {
    use std::io::Write;

    command.stdin(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("git could not be started: {error}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "git took no stdin".to_owned())?;
        stdin
            .write_all(stdin_bytes)
            .map_err(|error| format!("writing to git failed: {error}"))?;
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(error) => return Err(format!("waiting for git failed: {error}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "git did not answer within {}ms",
                timeout.as_millis()
            ));
        }
        std::thread::sleep(POLL);
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("reading git output failed: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

/// A git command prepared to run in `dir` with the environment stripped.
#[must_use]
pub fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in variables_to_remove(environment_names()) {
        command.env_remove(name);
    }
    command
}

/// Which of the given variable names must be removed before git is spawned.
///
/// Kept pure and separate from reading the environment: this is the rule worth testing, and the
/// workspace forbids `unsafe`, so a test cannot set variables on its own process to check it.
#[must_use]
pub fn variables_to_remove<N: AsRef<str>>(names: impl IntoIterator<Item = N>) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| {
            let name = name.as_ref();
            name.starts_with(GIT_VAR_PREFIX) && name != KEPT_GIT_VAR
        })
        .map(|name| name.as_ref().to_owned())
        .collect()
}

/// The names of this process's environment variables.
fn environment_names() -> Vec<String> {
    std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect()
}

/// Walks up from `cwd` looking for a `.git` entry, stopping at the root of the volume.
///
/// The walk is our own rather than git's answer, and it has a deadline of its own: a mount that
/// stopped responding turns `symlink_metadata` into a call that never returns.
#[must_use]
pub fn walk_for_dot_git(cwd: &Path, timeout: Duration) -> DotGitProbe {
    let deadline = Instant::now() + timeout;
    let mut current = Some(cwd);
    while let Some(dir) = current {
        if Instant::now() >= deadline {
            return DotGitProbe::Unknown {
                reason: format!(
                    "the search for {DOT_GIT} did not finish within {}ms",
                    timeout.as_millis()
                ),
            };
        }
        match read_dot_git(&dir.join(DOT_GIT)) {
            Ok(Some(found)) => return DotGitProbe::Found(found),
            Ok(None) => {}
            Err(reason) => return DotGitProbe::Unknown { reason },
        }
        current = dir.parent();
    }
    DotGitProbe::NotFound
}

/// Reads one candidate `.git`: a directory, a pointer file, or nothing.
fn read_dot_git(path: &Path) -> Result<Option<DotGit>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    let text = path.display().to_string();
    if metadata.is_dir() {
        return Ok(Some(DotGit::Dir { path: text }));
    }
    // A pointer file is small; a huge one is not a pointer and reading it whole is still safe
    // because git itself refuses anything but a single `gitdir:` line.
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(DotGit::File {
            path: text,
            content,
        })),
        Err(error) => Err(format!("{text} could not be read: {error}")),
    }
}

/// Paths under `pathspec` that differ from HEAD or are untracked, as bare paths.
///
/// Two plumbing commands instead of `status --porcelain`: porcelain's two-character status
/// column makes the first line's leading space significant, and the runner trims output. Bare
/// path lists have nothing to trim away.
///
/// # Errors
///
/// The text of what went wrong.
pub fn changed_paths(
    store: &Path,
    pathspec: &str,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let has_head = run_with_timeout(
        command(store, &["rev-parse", "--verify", "--quiet", "HEAD"]),
        timeout,
    )?
    .is_some();
    let mut paths: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if has_head {
        let modified = run_with_timeout(
            command(store, &["diff", "--name-only", "HEAD", "--", pathspec]),
            timeout,
        )?
        .unwrap_or_default();
        paths.extend(
            modified
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned),
        );
    }
    let untracked = run_with_timeout(
        command(
            store,
            &["ls-files", "--others", "--exclude-standard", "--", pathspec],
        ),
        timeout,
    )?
    .unwrap_or_default();
    paths.extend(
        untracked
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned),
    );
    Ok(paths.into_iter().collect())
}
