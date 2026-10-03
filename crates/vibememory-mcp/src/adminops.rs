//! `vibememory-mcp admin …`: the host console's files and words — what `admin` decides, done on
//! disk. Run on the host as root (`sudo`): the snapshot belongs to the account that publishes it,
//! and the console writes it as that account wrote it, owner, group and mode kept.
//!
//! A host with the cabinet is refused: there the cabinet is the one writer of the snapshot, and a
//! second writer would lose its changes at the cabinet's next publication.

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::admin::{self, HostFacts, Operation, Outcome, Role};
use crate::layout;

/// The console's settings beside the snapshot: the host's domain, which grants name.
pub const SETTINGS_FILE: &str = "/srv/vibememory/access/admin.json";

/// The account machine keys log in as, unless the settings name another.
const DEFAULT_SSH_USER: &str = "vmgit";

/// The cabinet's unit: present, the cabinet writes the snapshot and the console stays out.
const CABINET_UNIT: &str = "/etc/systemd/system/vibememory-cabinet.service";

/// Bytes in a megabyte, as quotas are typed.
const MEGABYTE: u64 = 1_048_576;

/// Exit code of a misused command.
const USAGE_EXIT: u8 = 2;

/// Every command of the console, as `admin` without arguments prints it.
pub const USAGE: &str = "\
vibememory-mcp admin — teams, members, tokens and machine keys of a host without the cabinet
Run on the host with sudo. Every change is checked as the host checks it before it is written

  admin init --domain <domain> [--ssh-user vmgit]
  admin show

  admin team add <team> --owner <handle> [--sessions] [--quota-mb <MB>]
  admin team sessions <team> on|off
  admin team quota <team> <MB>
  admin team remove <team> --confirm

  admin project add <team> <project>
  admin project remove <team> <project>

  admin member add <team> <handle> [--admin]
  admin member role <team> <handle> admin|member
  admin member remove <team> <handle>

  admin token issue <team> <member> <agent> [--reader] [--history] [--projects a,b] [--expires 2027-01-01T00:00:00Z]
  admin token revoke <tk_…>

  admin key add <member> <machine> <team>[,<team>…]   (the machine's public key on stdin)
  admin key revoke <mk_…>

Options before the command: --access <file> (default /srv/vibememory/access/access.json),
--settings <file> (default /srv/vibememory/access/admin.json)";

/// Runs one console command. `arguments` follow `admin`.
#[must_use]
pub fn run(arguments: &[String]) -> ExitCode {
    let mut access = PathBuf::from(layout::ACCESS_FILE);
    let mut settings = PathBuf::from(SETTINGS_FILE);
    let mut rest = arguments;
    loop {
        match rest {
            [flag, value, tail @ ..] if flag == "--access" => {
                access = PathBuf::from(value);
                rest = tail;
            }
            [flag, value, tail @ ..] if flag == "--settings" => {
                settings = PathBuf::from(value);
                rest = tail;
            }
            _ => break,
        }
    }
    let words: Vec<&str> = rest.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["init", options @ ..] => init(&settings, options),
        ["show"] => show(&access),
        _ => match operation(&words) {
            Ok(operation) => change(&access, &settings, &operation),
            Err(why) => {
                eprintln!("admin: {why}\n\n{USAGE}");
                ExitCode::from(USAGE_EXIT)
            }
        },
    }
}

/// The value after `name` among `options`.
fn option<'a>(options: &[&'a str], name: &str) -> Option<&'a str> {
    options
        .iter()
        .position(|option| *option == name)
        .and_then(|at| options.get(at + 1).copied())
}

fn has(options: &[&str], name: &str) -> bool {
    options.contains(&name)
}

fn megabytes(text: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .map(|value| value * MEGABYTE)
        .ok_or_else(|| format!("{text} is not a whole number of megabytes"))
}

fn role(text: &str) -> Result<Role, String> {
    match text {
        "admin" => Ok(Role::Admin),
        "member" => Ok(Role::Member),
        other => Err(format!("role {other}: admin or member")),
    }
}

fn names(list: &str) -> Vec<String> {
    list.split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The words of a command, as an operation. The public key of `key add` comes from stdin: a key
/// line pasted into a terminal, or piped from the machine's `connect --key-request`.
fn operation(words: &[&str]) -> Result<Operation, String> {
    let owned = |text: &str| text.to_owned();
    match words {
        ["team", "add", slug, options @ ..] => Ok(Operation::TeamAdd {
            slug: owned(slug),
            owner: option(options, "--owner")
                .map(owned)
                .ok_or("team add needs --owner <handle>")?,
            sessions: has(options, "--sessions"),
            quota_bytes: option(options, "--quota-mb")
                .map_or(Ok(admin::DEFAULT_QUOTA_BYTES), megabytes)?,
        }),
        ["team", "sessions", slug, state @ ("on" | "off")] => Ok(Operation::TeamSessions {
            slug: owned(slug),
            on: *state == "on",
        }),
        ["team", "quota", slug, size] => Ok(Operation::TeamQuota {
            slug: owned(slug),
            quota_bytes: megabytes(size)?,
        }),
        ["team", "remove", slug, "--confirm"] => Ok(Operation::TeamRemove { slug: owned(slug) }),
        ["team", "remove", _] => Err(
            "team remove takes --confirm: the team's tokens and keys go, and its store is renamed aside".to_owned(),
        ),
        ["project", verb @ ("add" | "remove"), team, name] => {
            let (team, name) = (owned(team), owned(name));
            Ok(if *verb == "add" {
                Operation::ProjectAdd { team, name }
            } else {
                Operation::ProjectRemove { team, name }
            })
        }
        ["member", "add", team, handle, options @ ..] => Ok(Operation::MemberAdd {
            team: owned(team),
            handle: owned(handle),
            role: if has(options, "--admin") {
                Role::Admin
            } else {
                Role::Member
            },
        }),
        ["member", "role", team, handle, rank] => Ok(Operation::MemberRole {
            team: owned(team),
            handle: owned(handle),
            role: role(rank)?,
        }),
        ["member", "remove", team, handle] => Ok(Operation::MemberRemove {
            team: owned(team),
            handle: owned(handle),
        }),
        ["token", "issue", team, member, agent, options @ ..] => Ok(Operation::TokenIssue {
            team: owned(team),
            member: owned(member),
            agent: owned(agent),
            reader: has(options, "--reader"),
            history: has(options, "--history"),
            projects: option(options, "--projects").map(names),
            expires_at: option(options, "--expires").map(owned),
        }),
        ["token", "revoke", id] => Ok(Operation::TokenRevoke { id: owned(id) }),
        ["key", "add", member, machine, teams] => Ok(Operation::KeyAdd {
            member: owned(member),
            machine: owned(machine),
            teams: names(teams),
            public_key: read_public_key()?,
        }),
        ["key", "revoke", id] => Ok(Operation::KeyRevoke { id: owned(id) }),
        _ => Err("unknown command".to_owned()),
    }
}

/// The machine's public key from the first line of stdin, in the host's wire form.
fn read_public_key() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|error| format!("the public key could not be read: {error}"))?;
    vibememory_core::ssh_key::wire_line(&line).ok_or_else(|| {
        "the machine's public key is expected on stdin, `ssh-ed25519 <base64>`, as `vibememory connect --key-request` prints it".to_owned()
    })
}

/// The settings file: the domain grants name and the account keys log in as.
fn init(settings: &Path, options: &[&str]) -> ExitCode {
    let Some(domain) = option(options, "--domain") else {
        eprintln!(
            "admin: init needs --domain <domain>, the host's name the agents and machines reach"
        );
        return ExitCode::from(USAGE_EXIT);
    };
    let ssh_user = option(options, "--ssh-user").unwrap_or(DEFAULT_SSH_USER);
    let text = format!(
        "{}\n",
        serde_json::json!({ "domain": domain, "sshUser": ssh_user })
    );
    match write_beside(settings, text.as_bytes(), None) {
        Ok(()) => {
            println!(
                "settings written: {} — grants name https://{domain}",
                settings.display()
            );
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("admin: {why}");
            ExitCode::FAILURE
        }
    }
}

fn facts(settings: &Path) -> Result<HostFacts, String> {
    let text = std::fs::read_to_string(settings).map_err(|error| {
        format!(
            "{}: {error} — run `admin init --domain <domain>` first",
            settings.display()
        )
    })?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", settings.display()))?;
    let domain = value
        .get("domain")
        .and_then(Value::as_str)
        .filter(|domain| !domain.is_empty())
        .ok_or_else(|| format!("{} names no domain", settings.display()))?;
    Ok(HostFacts {
        domain: domain.to_owned(),
        ssh_user: value
            .get("sshUser")
            .and_then(Value::as_str)
            .unwrap_or(DEFAULT_SSH_USER)
            .to_owned(),
        host_keys: crate::hostops::host_keys(),
    })
}

fn read_snapshot(access: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(access)
        .map_err(|error| format!("{}: {error}", access.display()))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: {error}", access.display()))
}

fn show(access: &Path) -> ExitCode {
    match read_snapshot(access) {
        Ok(snapshot) => {
            print!("{}", admin::describe(&snapshot));
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("admin: {why}");
            ExitCode::FAILURE
        }
    }
}

fn change(access: &Path, settings: &Path, operation: &Operation) -> ExitCode {
    if Path::new(CABINET_UNIT).exists() {
        eprintln!(
            "admin: this host runs the cabinet, and the cabinet writes the snapshot: change access there"
        );
        return ExitCode::FAILURE;
    }
    let result = facts(settings).and_then(|facts| {
        let snapshot = read_snapshot(access)?;
        let today = vibememory_cli::clock::now()
            .get(..10)
            .unwrap_or_default()
            .to_owned();
        let mut random = |buffer: &mut [u8]| fill_random(buffer);
        let (changed, outcome) = admin::apply(&snapshot, operation, &facts, &today, &mut random)?;
        write_beside(access, admin::render(&changed).as_bytes(), Some(access))?;
        Ok(outcome)
    });
    match result {
        Ok(Outcome::Done(said)) => {
            println!("{said}");
            ExitCode::SUCCESS
        }
        Ok(Outcome::Grant { said, grant }) => {
            eprintln!("{said}");
            eprintln!(
                "The grant below is the member's — pass it privately: a token grant holds the token, which exists nowhere else."
            );
            eprintln!("On their machine: vibememory connect --grant — paste the line, then Enter");
            println!("{grant}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("admin: {why}");
            ExitCode::FAILURE
        }
    }
}

/// Fills `buffer` from the system's random source. A host without one cannot make a token, and
/// stops rather than hand out a guessable one.
fn fill_random(buffer: &mut [u8]) {
    let read = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(buffer));
    if let Err(error) = read {
        eprintln!("admin: no random source: {error}");
        std::process::exit(1);
    }
}

/// Writes `bytes` to `path` whole or not at all: a temporary file beside it, renamed over it. The
/// owner, group and mode of `like` are kept, so the readers of the snapshot read it as before.
fn write_beside(path: &Path, bytes: &[u8], like: Option<&Path>) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    let temporary = directory.join(format!(
        ".{}.admin-{}",
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        std::process::id()
    ));
    let written = std::fs::File::create(&temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("{}: {error}", temporary.display()));
    }
    if let Some(like) = like {
        keep_ownership(like, &temporary)?;
    }
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{}: {error}", path.display())
    })
}

#[cfg(unix)]
fn keep_ownership(like: &Path, target: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata =
        std::fs::metadata(like).map_err(|error| format!("{}: {error}", like.display()))?;
    std::fs::set_permissions(target, metadata.permissions())
        .map_err(|error| format!("{}: {error}", target.display()))?;
    std::os::unix::fs::chown(target, Some(metadata.uid()), Some(metadata.gid())).map_err(|error| {
        format!(
            "{}: {error} — run the console with sudo, the snapshot is not yours",
            target.display()
        )
    })
}

#[cfg(not(unix))]
fn keep_ownership(_like: &Path, _target: &Path) -> Result<(), String> {
    Ok(())
}
