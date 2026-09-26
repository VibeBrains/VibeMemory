//! The binary. Everything it decides lives in the library next to it.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

use std::path::PathBuf;
use std::process::ExitCode;

use std::collections::BTreeMap;
use vibememory_cli::config::{Config, DesktopStore};
use vibememory_cli::hook::parse_input;
use vibememory_cli::hook::prompt_gate::{Gate, decide};
use vibememory_cli::hook::session_start;
use vibememory_cli::hook::stop::{
    PUSH_DEBOUNCE, commit_snapshot, push_if_due, record_end, record_progress,
};

use vibememory_cli::hook::stop::{Tail, Tails};
use vibememory_cli::install::{Layout, State, apply, plan};
use vibememory_cli::links_file;
use vibememory_cli::memory::{
    COWORK_MEMORY_VAR, JOURNAL_FILE, MemoryLocation, Quarantined, REMOTE_MEMORY_VAR, memory_dir,
    settings_memory_dir, sync,
};
use vibememory_core::naming::{
    EncSlug, IgnoreReason, PathSyntax, Resolution, canonical_cwd, enc_from_transcript_path,
};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("status") => report(false, wants_json(&args.collect::<Vec<String>>())),
        Some("doctor") => report(true, wants_json(&args.collect::<Vec<String>>())),
        Some("install") => {
            let dry_run = args.any(|arg| arg == "--dry-run");
            install(dry_run)
        }
        Some("connect") => connect_command(&args.collect::<Vec<String>>()),
        Some("disconnect") => disconnect_command(&args.collect::<Vec<String>>()),
        Some(vibememory_cli::connect::HEADERS_COMMAND) => {
            headers_command(&args.collect::<Vec<String>>())
        }
        Some("tick") => tick_command(&args.collect::<Vec<String>>()),
        Some("migrate") => migrate_command(&args.collect::<Vec<String>>()),
        Some("switch") => switch_command(&args.collect::<Vec<String>>()),
        Some("relink") => relink_command(&args.collect::<Vec<String>>(), false),
        Some("import") => relink_command(&args.collect::<Vec<String>>(), true),
        Some("forget") => forget_command(args.next().as_deref()),
        Some("session") => session_command(&args.collect::<Vec<String>>()),
        Some("project") => project_command(&args.collect::<Vec<String>>()),
        Some("store") => store_command(&args.collect::<Vec<String>>()),
        Some("merge-driver") => merge_driver_command(&args.collect::<Vec<String>>()),
        Some("hook") => match args.next().as_deref() {
            Some("session-start") => session_start_hook(),
            Some("stop") => session_progress_hook(false),
            Some("session-end") => session_progress_hook(true),
            Some("user-prompt-submit") => prompt_gate_hook(),
            other => {
                eprintln!(
                    "unknown hook {other:?}; try session-start, stop, session-end or \
                     user-prompt-submit"
                );
                ExitCode::from(2)
            }
        },
        // `--version` is what every other tool answers to, and the engine is asked it by
        // scripts, by `doctor` on the other machine, and by a human wondering which build is in
        // `~/.vibememory/bin` after an upgrade.
        Some("--version" | "-V" | "version") => {
            println!("vibememory {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Nothing, or a request for help: the same page either way.
        None | Some("--help" | "-h" | "help") => {
            usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command {other:?}; try status, doctor, install or --help");
            ExitCode::from(2)
        }
    }
}

/// The paths this machine works with. Both are overridable so that nothing but a deliberate run
/// ever touches the real directories.
fn layout() -> Layout {
    // Exit 1, not 2: for a hook, 2 means "block the prompt", and a machine without a home is not
    // a reason to stop somebody typing.
    Layout::from_environment().unwrap_or_else(|problem| {
        eprintln!("vibememory: {problem}");
        std::process::exit(1)
    })
}

/// Reads the configuration, or explains why the engine will not start on it.
fn read_config(layout: &Layout) -> Result<Config, String> {
    let path = layout.engine_dir.join("config.json");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    Config::parse(&text, PathSyntax::Posix).map_err(|error| error.to_string())
}

/// Whether the engine is set up on this machine at all: a machine that only connects agents has
/// no `config.json`, and that is not a fault.
fn engine_configured(layout: &Layout) -> bool {
    layout.engine_dir.join("config.json").exists()
}

/// `status` prints where the machine stands; `doctor` does the same and fails when something is
/// not as it must be.
fn report(strict: bool, json: bool) -> ExitCode {
    let layout = layout();
    let tokens = vibememory_cli::credentials::kept_tokens(&layout);
    if !engine_configured(&layout) {
        return report_without_engine(&layout, &tokens, strict, json);
    }
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let actions = plan(&layout, &config, &[]);
    // The mirror lives on the host and is asked over the network, so only `doctor` pays for it.
    let mirror = strict.then(|| probe_mirror(&config));
    // The host's disk, asked in the same round: a full host refuses every push while each machine
    // goes on working, and nothing else would say so.
    let disk = if strict {
        vibememory_cli::mirror::probe_disk(config.remote.as_deref(), ask_host)
    } else {
        None
    };
    if json {
        return report_json(
            &config,
            &layout.engine_dir,
            &actions,
            mirror.as_ref(),
            disk.as_ref(),
            &tokens,
            strict,
        );
    }
    let mut wrong = 0;
    for action in &actions {
        let mark = match &action.state {
            State::Satisfied => "ok      ",
            State::Missing => {
                wrong += 1;
                "missing "
            }
            State::Conflict { .. } => {
                wrong += 1;
                "conflict"
            }
            State::Unknown { .. } => {
                wrong += 1;
                "unknown "
            }
        };
        println!("{mark} {}", action.step.describe());
        match &action.state {
            State::Conflict { found } => println!("         found: {found}"),
            State::Unknown { reason } => println!("         {reason}"),
            _ => {}
        }
    }
    // Not a step and never a failure: `ignoreCwd` is the owner's rule, and obeying it is correct.
    // It is printed because obeying it silently means a directory whose transcripts never leave
    // this machine cannot be found out about from anywhere.
    let ignored = vibememory_cli::guard::TickState::read(&layout.engine_dir).ignored;
    for directory in &ignored {
        println!(
            "left alone {} ({}) — {} transcript(s) stay on this machine only",
            directory.enc, directory.reason, directory.transcripts
        );
    }
    // Not a failure either: a session file that holds an agent token stays here by design, and
    // what the person does is revoke the token.
    for (path, file) in &vibememory_cli::held::Held::read(&layout.engine_dir).files {
        println!(
            "held     {} — holds agent token {}; stays on this machine since {}",
            vibememory_core::terminal::printable(path),
            file.tokens.iter().cloned().collect::<Vec<_>>().join(", "),
            file.since
        );
    }
    if let Some(mirror) = &mirror {
        println!("{}", mirror.describe());
        if mirror.is_fault() {
            wrong += 1;
        }
    }
    match &disk {
        Some(Ok(disk)) => {
            println!("{}", disk.describe());
            if disk.is_low() {
                wrong += 1;
            }
        }
        Some(Err(reason)) => println!("disk     host unknown \u{2014} {reason}"),
        None => {}
    }
    wrong += print_tokens(&tokens);
    wrong += print_teams(&layout);
    if strict && wrong > 0 {
        eprintln!(
            "{wrong} of {} steps are not in place",
            actions.len() + tokens.len()
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The team stores as the JSON report carries them.
fn teams_json(teams: &[vibememory_cli::team_connect::TeamFacts]) -> serde_json::Value {
    serde_json::Value::Array(
        teams
            .iter()
            .map(|facts| {
                serde_json::json!({
                    "team": facts.team,
                    "cabinet": facts.cabinet,
                    "problems": facts.problems,
                    "pause": facts.pause.as_ref().map(|pause| serde_json::json!({
                        "code": pause.code,
                        "lines": pause.lines,
                        "since": pause.since,
                        "recheckAt": pause.recheck_at,
                    })),
                    "failures": facts.failures,
                })
            })
            .collect(),
    )
}

/// The team stores: each connected team, what is wrong with its files, and its pause with what to
/// do about it. Answers how many teams need a person.
fn print_teams(layout: &Layout) -> usize {
    let mut wrong = 0;
    for facts in vibememory_cli::team_connect::team_facts(layout) {
        let line = match &facts.pause {
            Some(pause) => format!(
                "paused since {} ({}), asks again at {}",
                pause.since, pause.code, pause.recheck_at
            ),
            None if facts.failures > 0 => format!("{} failing run(s) in a row", facts.failures),
            None => "in step".to_owned(),
        };
        println!("team     {} — {line}", facts.team);
        if let Some(pause) = &facts.pause {
            println!(
                "         {}",
                pause_advice(layout, &facts.team, &pause.code)
            );
        }
        for problem in &facts.problems {
            println!("         {problem}");
        }
        if facts.pause.is_some() || !facts.problems.is_empty() {
            wrong += 1;
        }
    }
    wrong
}

/// The credentials section: each kept token, and what is wrong with its files. Answers how many
/// tokens have something wrong.
fn print_tokens(tokens: &[vibememory_cli::credentials::KeptToken]) -> usize {
    let mut wrong = 0;
    for token in tokens {
        // the sidecar is a file on this machine anyone with its rights could have edited
        println!(
            "token    {}/{} {} from {}",
            vibememory_core::terminal::printable(&token.team),
            vibememory_core::terminal::printable(&token.agent),
            vibememory_core::terminal::printable(&token.token_id),
            vibememory_core::terminal::printable(&token.cabinet)
        );
        if !token.problems.is_empty() {
            wrong += 1;
        }
        for problem in &token.problems {
            println!("         {}", vibememory_core::terminal::printable(problem));
        }
    }
    wrong
}

/// `status` and `doctor` on a machine without the engine: a member of a `memory` team has the two
/// binaries, curl and tokens, and nothing else — which is a complete machine, not a broken one.
fn report_without_engine(
    layout: &Layout,
    tokens: &[vibememory_cli::credentials::KeptToken],
    strict: bool,
    json: bool,
) -> ExitCode {
    let actions = vibememory_cli::install::plan_binaries(layout);
    let wrong_steps = actions
        .iter()
        .filter(|action| !matches!(action.state, State::Satisfied))
        .count();
    let wrong_tokens = tokens
        .iter()
        .filter(|token| !token.problems.is_empty())
        .count();
    if json {
        let report = serde_json::json!({
            "engine": "notInstalled",
            "steps": steps_json(&actions),
            "stepsWrong": wrong_steps,
            "credentials": tokens_json(tokens),
        });
        println!("{report:#}");
    } else {
        println!(
            "engine   not installed \u{2014} no {}: this machine connects agents only",
            layout.engine_dir.join("config.json").display()
        );
        for action in &actions {
            let mark = if matches!(action.state, State::Satisfied) {
                "ok      "
            } else {
                "missing "
            };
            println!("{mark} {}", action.step.describe());
            match &action.state {
                State::Conflict { found } => println!("         {found}"),
                State::Unknown { reason } => println!("         {reason}"),
                _ => {}
            }
        }
        print_tokens(tokens);
    }
    if strict && wrong_steps + wrong_tokens > 0 {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// The steps as `--json` gives them.
fn steps_json(actions: &[vibememory_cli::install::Action]) -> Vec<serde_json::Value> {
    actions
        .iter()
        .map(|action| {
            let (state, detail) = match &action.state {
                State::Satisfied => ("ok", None),
                State::Missing => ("missing", None),
                State::Conflict { found } => ("conflict", Some(found.clone())),
                State::Unknown { reason } => ("unknown", Some(reason.clone())),
            };
            serde_json::json!({ "step": action.step.describe(), "state": state, "detail": detail })
        })
        .collect()
}

/// The kept tokens as `--json` gives them: ids and places, never a token.
fn tokens_json(tokens: &[vibememory_cli::credentials::KeptToken]) -> Vec<serde_json::Value> {
    tokens
        .iter()
        .map(|token| {
            serde_json::json!({
                "team": token.team,
                "agent": token.agent,
                "tokenId": token.token_id,
                "cabinet": token.cabinet,
                "file": token.file.display().to_string(),
                "problems": token.problems,
            })
        })
        .collect()
}

/// `connect --cabinet <address> [--agent <name>]`: trades a claim code from the cabinet — typed
/// at the prompt or piped in — for a token and keeps it where only its owner reaches it. Prints
/// paths and the line that registers the server — never the token.
fn connect_command(args: &[String]) -> ExitCode {
    if let [flag, team] = args
        && flag == "--refresh"
    {
        return refresh_command(team);
    }
    let (asked, agent) = match connect_arguments(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("connect: {error}");
            eprintln!(
                "usage: vibememory connect --cabinet <address> [--agent <name>], and the code at the prompt; \
                 vibememory connect --refresh <team> after the host changed its key"
            );
            return ExitCode::from(2);
        }
    };
    let cabinet = match vibememory_core::claim::cabinet_address(&asked) {
        Ok(cabinet) => cabinet,
        Err(error) => {
            eprintln!("connect: --cabinet {error}");
            return ExitCode::from(2);
        }
    };
    let stdin = std::io::stdin();
    if std::io::IsTerminal::is_terminal(&stdin) {
        eprint!("claim code: ");
    }
    let code = match vibememory_cli::connect::read_code(stdin.lock()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("connect: {error}");
            return ExitCode::from(2);
        }
    };
    let layout = layout();
    // a key goes with every code; without ssh-keygen only a code for a machine is refused
    let pending = vibememory_cli::team_connect::PendingKey::make(&layout).ok();
    let reply = match vibememory_cli::connect::ask_cabinet(
        &cabinet,
        &code,
        pending.as_ref().map(|key| key.public.as_str()),
    ) {
        Ok(reply) => reply,
        Err(error) => {
            if let Some(pending) = pending {
                pending.discard();
            }
            eprintln!("connect: {error}");
            return ExitCode::FAILURE;
        }
    };
    let answer =
        vibememory_core::claim::read_answer(reply.exit, reply.status, &reply.body, &cabinet);
    let grant = match (answer, pending) {
        (Ok(vibememory_core::claim::Claim::Token(grant)), pending) => {
            if let Some(pending) = pending {
                pending.discard();
            }
            grant
        }
        (Ok(vibememory_core::claim::Claim::Key(grant)), Some(pending)) => {
            return connect_key(&layout, &grant, pending);
        }
        (Ok(vibememory_core::claim::Claim::Key(grant)), None) => {
            eprintln!(
                "connect: the code was for a machine of team {}, and ssh-keygen could not make a \
                 key here; revoke key {} in the cabinet, install OpenSSH and connect again",
                grant.team, grant.key_id
            );
            return ExitCode::FAILURE;
        }
        (Err(failure), pending) => {
            if let Some(pending) = pending {
                pending.discard();
            }
            print_claim_failure(&failure, &reply);
            return ExitCode::FAILURE;
        }
    };
    if let Some(agent) = agent
        && agent != grant.agent
    {
        eprintln!(
            "note: the code was made for agent {}, not {agent}: the token is kept as {}'s",
            grant.agent, grant.agent
        );
    }
    let kept = match vibememory_cli::connect::keep_token(&layout, &grant) {
        Ok(kept) => kept,
        Err(error) => {
            eprintln!(
                "connect: token {} was issued but could not be kept ({error}); revoke it in the \
                 cabinet and make a new code",
                grant.token_id
            );
            return ExitCode::FAILURE;
        }
    };
    print_connected(&grant, &kept);
    ExitCode::SUCCESS
}

/// `connect --refresh <team>`: the team's `known_hosts` from the cabinet, after the host changed its
/// key.
fn refresh_command(team: &str) -> ExitCode {
    match vibememory_cli::team_connect::refresh(&layout(), team) {
        Ok(path) => {
            println!("refreshed: {} from the cabinet's host keys", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("connect --refresh: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Why a claim gave nothing, with curl's own line and — when the cabinet had said yes to an answer
/// the engine then refused — the warning that something may have been issued.
fn print_claim_failure(
    failure: &vibememory_core::claim::ClaimFailure,
    reply: &vibememory_cli::connect::CabinetReply,
) {
    eprintln!("connect: {failure}");
    // curl's own line, which already names itself
    if !reply.stderr.is_empty() {
        eprintln!("{}", vibememory_core::terminal::printable(&reply.stderr));
    }
    // an answer the engine refused after the cabinet had said yes: the code is spent, and a
    // token or a key may have been issued that nothing here keeps
    if matches!(failure, vibememory_core::claim::ClaimFailure::Malformed(_)) && reply.status == 200
    {
        eprintln!(
            "connect: the cabinet may have issued a token or a key for this code: look at the \
                 team's page and revoke the ones you do not recognise"
        );
    }
}

/// A machine key answer: the key kept and the team's store cloned, or why not — with the key's id
/// to revoke, since the cabinet registered it either way.
fn connect_key(
    layout: &vibememory_cli::install::Layout,
    grant: &vibememory_core::claim::KeyGrant,
    pending: vibememory_cli::team_connect::PendingKey,
) -> ExitCode {
    match vibememory_cli::team_connect::keep_key(layout, grant, pending, engine_configured(layout))
    {
        Ok(store) => {
            // the clone's merge drivers and this machine's directory, before the first tick merges
            let applied = vibememory_cli::install::apply(
                layout,
                &vibememory_cli::install::plan_team(layout, &grant.team),
                false,
            );
            for (what, error) in &applied.failed {
                eprintln!("connect: {what} — {error}");
            }
            println!(
                "connected: team {} as {}, machine {}",
                grant.team, grant.member, grant.store_name
            );
            println!(
                "store:     {}{}",
                store.clone.display(),
                if store.cloned {
                    ""
                } else {
                    " (kept, now on the new key)"
                }
            );
            println!(
                "next:      name the team's projects in {} under stores.{}.cwd, or move one with \
                 `vibememory project move <dir> --to {}`",
                layout.engine_dir.join("config.json").display(),
                grant.team,
                grant.team
            );
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            eprintln!("connect: {refusal}");
            eprintln!(
                "connect: key {} is registered for this machine in the cabinet: revoke it there",
                grant.key_id
            );
            ExitCode::FAILURE
        }
    }
}

/// What `connect` says once the token is kept: where it lies and the line that registers it —
/// never the token.
fn print_connected(
    grant: &vibememory_core::claim::TokenGrant,
    kept: &vibememory_cli::connect::KeptToken,
) {
    println!(
        "connected: team {} as {}, agent {} (token {})",
        grant.team, grant.member, grant.agent, grant.token_id
    );
    println!("token     {}", kept.token.display());
    println!(
        "fragment  {} \u{2014} for agents that read a JSON list of MCP servers",
        kept.fragment.display()
    );
    println!("register with Claude Code:");
    println!(
        "  {}",
        vibememory_cli::connect::claude_code_registration(
            grant,
            &vibememory_cli::install::installed_binary(&layout())
        )
    );
}

/// `disconnect <team>`: the team's tokens off this machine, and what is left to do elsewhere —
/// revoke them in the cabinet, and remove the client registered in `~/.claude.json`, which the
/// engine never writes.
/// What leaving a team with sessions did, and what is left to do in the cabinet and the config.
fn report_left(team: &str, left: &vibememory_cli::team_ops::Left) {
    for name in &left.brought_back {
        println!("back     {name}: your sessions and memory are in the personal store again");
    }
    for name in &left.unlinked {
        println!("unlinked {name}: the team's project; its files stay in the archive");
    }
    println!("archive  {}", left.archive.display());
    println!(
        "revoke   machine key {} in {}/team/{team}: until then it still opens the team",
        vibememory_core::terminal::printable(&left.key_id),
        vibememory_core::terminal::printable(&left.cabinet)
    );
    println!("config   remove stores.{team} from config.json: its directories are yours again");
}

fn disconnect_command(args: &[String]) -> ExitCode {
    let [team] = args else {
        eprintln!("usage: vibememory disconnect <team>");
        return ExitCode::from(2);
    };
    let layout = layout();
    // a team with sessions first: its projects come back before the tokens go
    let left_sessions = vibememory_cli::team_connect::connected_teams(&layout).contains(team);
    if left_sessions {
        let Ok(config) = read_config(&layout) else {
            eprintln!(
                "disconnect: config.json does not read, so the team's projects cannot be placed"
            );
            return ExitCode::FAILURE;
        };
        match vibememory_cli::team_ops::leave(&layout, &config, team, &vibememory_cli::clock::now())
        {
            Ok(left) => report_left(team, &left),
            Err(error) => {
                eprintln!("disconnect: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    let done = match vibememory_cli::connect::disconnect(&layout, team) {
        Ok(done) => done,
        Err(error) => {
            eprintln!("disconnect: {error}");
            return ExitCode::FAILURE;
        }
    };
    if done.removed.is_empty() && done.left.is_empty() {
        if !left_sessions {
            println!("no token of team {team} is kept on this machine");
        }
        return ExitCode::SUCCESS;
    }
    for path in &done.removed {
        println!("removed  {}", path.display());
    }
    for path in &done.left {
        println!(
            "left     {} \u{2014} not written by connect; revoke it, then remove it by hand",
            path.display()
        );
    }
    for (token_id, cabinet) in &done.revoke {
        // the sidecar is a file on this machine anyone with its rights could have edited
        println!(
            "revoke   {} in {}: until then it still opens team {team}",
            vibememory_core::terminal::printable(token_id),
            vibememory_core::terminal::printable(cabinet)
        );
    }
    println!(
        "Claude Code: claude mcp remove --scope user {}",
        vibememory_core::claim::server_name(team)
    );
    ExitCode::SUCCESS
}

/// `mcp-headers <team> <agent>`: what Claude Code runs on every connection to the team's memory
/// server — the authorization header of the kept token, on stdout and nowhere else.
fn headers_command(args: &[String]) -> ExitCode {
    let [team, agent] = args else {
        eprintln!(
            "usage: vibememory {} <team> <agent>",
            vibememory_cli::connect::HEADERS_COMMAND
        );
        return ExitCode::from(2);
    };
    match vibememory_cli::connect::headers_of(&layout(), team, agent) {
        Ok(headers) => {
            println!("{headers}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("vibememory: {why}");
            ExitCode::FAILURE
        }
    }
}

/// The arguments of `connect`, read strictly: `--cabinet <address>` once, `--agent <name>` at most
/// once, and nothing else. A code is never taken from the command line — `--code`, `--code=…` and
/// a bare word alike are refused, so that nobody learns only later that `ps` showed it.
fn connect_arguments(args: &[String]) -> Result<(String, Option<String>), String> {
    let mut cabinet = None;
    let mut agent = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let slot = match arg.as_str() {
            "--cabinet" => &mut cabinet,
            "--agent" => &mut agent,
            code if code.starts_with("--code") => {
                return Err(
                    "the code is not taken from the command line, where every user of this machine \
                     can read it: run the command without it and paste the code when asked"
                        .to_owned(),
                );
            }
            other => {
                return Err(format!(
                    "{:?} is not an argument of connect",
                    vibememory_core::terminal::printable(other)
                ));
            }
        };
        let value = rest.next().ok_or_else(|| format!("{arg} needs a value"))?;
        if slot.replace(value.clone()).is_some() {
            return Err(format!("{arg} is given twice"));
        }
    }
    let cabinet = cabinet.ok_or_else(|| "--cabinet is required".to_owned())?;
    Ok((cabinet, agent))
}

/// Brings the machine to the planned state, or says what it would do. Without a `config.json`
/// the engine is not set up here, and only its binaries are placed: what a member of a `memory`
/// team needs, and what the archive's `./vibememory install` promises.
fn install(dry_run: bool) -> ExitCode {
    let layout = layout();
    let actions = if engine_configured(&layout) {
        match read_config(&layout) {
            Ok(config) => plan(&layout, &config, &[]),
            Err(error) => {
                eprintln!("config: {error}");
                return ExitCode::from(2);
            }
        }
    } else {
        println!(
            "engine not configured (no {}): placing the binaries only",
            layout.engine_dir.join("config.json").display()
        );
        vibememory_cli::install::plan_binaries(&layout)
    };
    let applied = apply(&layout, &actions, dry_run);
    let verb = if dry_run { "would do" } else { "did" };
    for what in &applied.performed {
        println!("{verb}: {what}");
    }
    for what in &applied.untouched {
        println!("already in place: {what}");
    }
    for (what, found) in &applied.refused {
        println!("left alone: {what} — found {found}");
    }
    for (what, error) in &applied.failed {
        eprintln!("failed: {what} — {error}");
    }
    if applied.is_complete() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `store reclone <team>`: the team's clone made anew after its host refused what it held.
fn store_command(args: &[String]) -> ExitCode {
    let [verb, team] = args else {
        eprintln!("usage: vibememory store reclone <team>");
        return ExitCode::from(2);
    };
    if verb != "reclone" {
        eprintln!("usage: vibememory store reclone <team>");
        return ExitCode::from(2);
    }
    let layout = layout();
    match vibememory_cli::team_ops::reclone(&layout, team, &vibememory_cli::clock::now()) {
        Ok(done) => {
            let applied = vibememory_cli::install::apply(
                &layout,
                &vibememory_cli::install::plan_team(&layout, team),
                false,
            );
            for (what, error) in &applied.failed {
                eprintln!("store reclone: {what} \u{2014} {error}");
            }
            println!(
                "recloned: team {team}; {} file(s) carried over, the refused clone is in {}",
                done.carried.len(),
                done.rejected.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("store reclone: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `project move <dir> --to <team>|personal`: a project between the personal store and a team's —
/// see `project_move` for what goes and what stays.
fn project_command(args: &[String]) -> ExitCode {
    let [verb, dir, flag, to] = args else {
        eprintln!("usage: vibememory project move <dir> --to <team>|personal");
        return ExitCode::from(2);
    };
    if verb != "move" || flag != "--to" {
        eprintln!("usage: vibememory project move <dir> --to <team>|personal");
        return ExitCode::from(2);
    }
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let real =
        std::fs::canonicalize(dir).map_or_else(|_| dir.clone(), |path| path.display().to_string());
    let cwd = canonical_cwd(&real, session_start::host_syntax());
    let portable = portable_cwd(&config, &cwd);
    let stamp = vibememory_cli::clock::now();
    let moved = if to == "personal" {
        vibememory_cli::project_move::to_personal(&layout, &config, &cwd, &portable)
    } else {
        vibememory_cli::project_move::to_team(&layout, &config, &cwd, &portable, to, &stamp)
    };
    match moved {
        Ok(report) => {
            println!(
                "moved: {} to {to} — {} file(s), {} of them memory; the next tick sends what is shared",
                report.name,
                report.copied.len(),
                report.memory_versions
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("project move: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `session share <sid>`: gives the team a session from before its project went to the team. The
/// session stays where it is; it only leaves the list of what this machine keeps to itself, and the
/// next tick commits it into the team.
fn session_command(args: &[String]) -> ExitCode {
    let [verb, session] = args else {
        eprintln!("usage: vibememory session share <session-id>");
        return ExitCode::from(2);
    };
    if verb != "share" {
        eprintln!("usage: vibememory session share <session-id>");
        return ExitCode::from(2);
    }
    let layout = layout();
    let mut shared = false;
    for team in vibememory_cli::team_connect::connected_teams(&layout) {
        match vibememory_cli::local_only::release(&layout.team_store(&team), session) {
            Ok(released) if !released.is_empty() => {
                shared = true;
                println!(
                    "shared with team {team}: {} file(s); the next tick sends them",
                    released.len()
                );
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("session share: team {team}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    if shared {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "session share: no team keeps {} on this machine alone",
            vibememory_core::terminal::printable(session)
        );
        ExitCode::FAILURE
    }
}

/// `SessionStart`: put the link in place before the CLI creates a real directory.
///
/// Always exits 0. A hook that fails takes the session with it, and no synchronisation is worth
/// that; whatever went wrong is said through `additionalContext` instead.
fn session_start_hook() -> ExitCode {
    let mut text = String::new();
    if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        return say(&format!("VibeMemory could not read its input: {error}"));
    }
    let input = match parse_input(&text) {
        Ok(input) => input,
        Err(error) => return say(&error.message()),
    };

    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => return say(&format!("VibeMemory is not configured: {error}")),
    };

    let syntax = session_start::host_syntax();
    // The encoded directory name comes from the transcript path the CLI itself reports: the
    // encoding is its answer, never our guess.
    let enc = match enc_from_transcript_path(&input.transcript_path, syntax) {
        Ok(enc) => enc,
        Err(error) => {
            return say(&format!(
                "VibeMemory could not read the transcript path: {error}"
            ));
        }
    };
    let cwd = canonical_cwd(&input.cwd, syntax);
    // The store the working directory is routed to: the personal one, or a connected team's
    let store = match vibememory_cli::stores::for_cwd(&layout, &config, &cwd, syntax) {
        Ok(store) => store,
        Err(reason) => return say(&format!("VibeMemory: {reason}")),
    };
    // The rule the memory server names its project by, too: see `project::resolve`.
    let resolution = vibememory_cli::project::resolve(&store.clone, &config.naming, &cwd);

    let decision = match resolution {
        Ok(Resolution::Named { name, .. }) => {
            session_start::decide(&layout, &store.clone, &enc, Some(&name), None)
        }
        Ok(Resolution::Ignored { reason }) => {
            let reason = match reason {
                IgnoreReason::Pattern(pattern) => format!("ignoreCwd matched {pattern}"),
                IgnoreReason::ProjectDirName(name) => {
                    format!("CLAUDE_CODE_PROJECT_DIR_NAME is {name}")
                }
            };
            session_start::decide(&layout, &store.clone, &enc, None, Some(&reason))
        }
        Err(error) => return say(&format!("VibeMemory could not name this project: {error}")),
    };

    // Before anything else that could take time: from this moment the tick knows the working
    // directory is busy, whatever the session goes on to do.
    if let Ok(clone) = std::fs::canonicalize(&store.clone) {
        let _ = vibememory_cli::hook::stop::record_live(
            &clone,
            &store.machine_id,
            &input.session_id,
            &portable_cwd(&config, &cwd),
            &vibememory_cli::clock::now(),
        );
    }
    if let Err(error) = session_start::perform(&layout, &store.clone, &decision) {
        return say(&format!(
            "VibeMemory could not put the link in place: {error}"
        ));
    }
    // What this machine now knows about the link, for the other machines to read. The
    // `transcript_path` is what proves it: the encoding came from the CLI, not from our guess.
    if let Some(name) = decision.store_name() {
        let _ = links_file::record(
            &store.clone,
            &store.machine_id,
            &links_file::Observation {
                enc: enc.as_str(),
                name,
                cwd: &portable_cwd(&config, &cwd),
                syntax,
                source: vibememory_core::links::LinkSource::Observed,
                confirmed_by: Some(&input.transcript_path),
            },
        );
    }
    // Anything a merge in the tick needed a person to know has been waiting for a session to
    // exist; this is that session.
    let mut notes: Vec<String> = vibememory_cli::merge_report::read_pending(&layout.engine_dir);
    if !notes.is_empty() {
        let _ = vibememory_cli::merge_report::clear_pending(&layout.engine_dir);
    }
    if let Some(note) = handoff_note(&layout, &enc) {
        notes.push(note);
    }
    for note in quarantine_notes(&layout.engine_dir) {
        notes.push(note);
    }
    if let Some(message) = decision.additional_context() {
        notes.push(message);
    }
    if notes.is_empty() {
        ExitCode::SUCCESS
    } else {
        say(&notes.join(" "))
    }
}

/// Says one sentence to the session and exits 0, which is the only exit code a hook may use.
fn say(message: &str) -> ExitCode {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": message,
        }
    });
    println!("{payload}");
    ExitCode::SUCCESS
}

/// `Stop` and `SessionEnd`: commit what the session has written so far.
///
/// Both events do the same work, because neither can be relied on alone: a session whose turn
/// fails produces `SessionEnd` and no `Stop` (measured on 2.1.258), while a long session produces
/// many `Stop`s and one `SessionEnd` at the very end. Committing twice costs nothing — an
/// unchanged snapshot makes no commit — and missing one costs the session's history.
///
/// Exits 0 like every hook. The snapshot is cut at the last newline, so what is committed is
/// always a whole number of records; the live file itself is never handed to git.
fn session_progress_hook(ended: bool) -> ExitCode {
    let mut text = String::new();
    if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
        return say(&format!("VibeMemory could not read its input: {error}"));
    }
    let input = match parse_input(&text) {
        Ok(input) => input,
        Err(error) => return say(&error.message()),
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => return say(&format!("VibeMemory is not configured: {error}")),
    };

    // The transcript is reached through the link, so its real path is inside a store's clone —
    // the personal one or a team's — unless this session is one the hook could not link, and then
    // there is nothing to commit here.
    let transcript = std::path::PathBuf::from(&input.transcript_path);
    let Ok(real) = std::fs::canonicalize(&transcript) else {
        return ExitCode::SUCCESS;
    };
    let Some((owner, relative)) = vibememory_cli::stores::for_file(&layout, &config, &real) else {
        // A session in a real directory: the tick imports it, and saying so on every stop would
        // be noise, since SessionStart already said it once.
        return ExitCode::SUCCESS;
    };
    let store = owner.clone.clone();
    let relative_text = relative.to_string_lossy().replace('\\', "/");
    // a session from before the project went to the team stays on this machine, resumed or not
    if owner.team.is_some() && vibememory_cli::local_only::is_local(&store, &relative_text) {
        return ExitCode::SUCCESS;
    }
    let relative = relative.to_string_lossy().replace('\\', "/");
    let stamp = vibememory_cli::clock::now();

    let kept = vibememory_cli::held::Kept::read(&owner.state_dir);
    let stopped = match commit_snapshot(&store, &real, &relative, &stamp, &kept) {
        Ok(stopped) => stopped,
        Err(error) => {
            return say(&format!(
                "VibeMemory could not commit this session: {error}"
            ));
        }
    };
    // Kept back when it holds an agent token; the next session hears of it, `doctor` lists it.
    let held = if stopped.held.is_empty() {
        vibememory_cli::held::release(&owner.state_dir, &relative)
    } else {
        vibememory_cli::held::hold_found(&owner.state_dir, &relative, stopped.held, &stamp)
    };
    if let Err(error) = held {
        return say(&format!(
            "VibeMemory could not record what it keeps back: {error}"
        ));
    }
    let cwd = portable_cwd(&config, &input.cwd);
    if let Err(error) = record_progress(
        &store,
        &owner.machine_id,
        &input.session_id,
        &cwd,
        &real,
        &stamp,
    ) {
        return say(&format!("VibeMemory could not record progress: {error}"));
    }
    if ended && let Err(error) = record_end(&store, &owner.machine_id, &input.session_id) {
        return say(&format!("VibeMemory could not close this session: {error}"));
    }
    // Memory is synchronised on the same events as the transcript: edits first, then the
    // projection. A model that wrote a note during the turn has it recorded before the commit of
    // the next stop, not two minutes later.
    let notes = sync_memory(&layout, &config, &input.transcript_path, &stamp);

    // A push that does not happen costs nothing here: the commit is already on this disk, and the
    // tick pushes again in two minutes.
    let _ = push_if_due(&store, &owner.state_dir, epoch_seconds(), PUSH_DEBOUNCE);
    match notes {
        Some(message) => say(&message),
        None => ExitCode::SUCCESS,
    }
}

/// Reads memory edits back into the journal and writes the projection out again.
///
/// Returns a sentence for the session only when there is something it must know: versions two
/// machines wrote at once, or a document the engine could not read. Everything else is silent.
fn sync_memory(
    layout: &Layout,
    config: &Config,
    transcript_path: &str,
    stamp: &str,
) -> Option<String> {
    let syntax = session_start::host_syntax();
    let enc = enc_from_transcript_path(transcript_path, syntax).ok()?;
    let memory_dir = project_memory_dir(layout, &enc);

    // The journal lives beside the transcripts of the project, in the store; the projection is
    // wherever the CLI keeps it, which need not be the same place at all.
    let journal = std::fs::canonicalize(&memory_dir)
        .ok()
        .and_then(|dir| dir.parent().map(|project| project.join(JOURNAL_FILE)))?;

    let kept = vibememory_cli::held::Kept::read(&layout.engine_dir);
    let synced = match sync(&memory_dir, &journal, stamp, &config.machine_id, &kept) {
        Ok(synced) => synced,
        Err(error) => return Some(format!("VibeMemory could not synchronise memory: {error}")),
    };
    let mut notes = Vec::new();
    if !synced.divergent.is_empty() {
        notes.push(format!(
            "VibeMemory: {} memory record(s) were written on two machines at once and both \
             versions are kept ({}). The rival version sits next to each one as \
             `<id>.rival-<version>.md`; please reconcile them.",
            synced.divergent.len(),
            synced.divergent.join(", ")
        ));
    }
    for (name, error) in &synced.rejected {
        notes.push(format!(
            "VibeMemory could not read the memory document {name}: {error}. The file is left as \
             it is."
        ));
    }
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(" "))
    }
}

/// One sentence per kind of thing waiting in the quarantine.
///
/// Per kind, not one for everything: the quarantine used to hold only memory records, and a
/// notice that calls a folded Desktop card a "version of a memory file" sends the reader hunting
/// for a conflict that is not there. What to do about each kind differs too — a memory version
/// waits to be reconciled, a folded card waits only to be deleted or kept.
fn quarantine_notes(engine_dir: &std::path::Path) -> Vec<String> {
    let waiting = vibememory_cli::memory::quarantined(engine_dir);
    let mut by_kind: BTreeMap<Quarantined, Vec<String>> = BTreeMap::new();
    for name in waiting {
        by_kind
            .entry(vibememory_cli::memory::quarantined_kind(&name))
            .or_default()
            .push(name);
    }
    by_kind
        .into_iter()
        .map(|(kind, names)| {
            format!(
                "VibeMemory: {} {} in the quarantine — {} ({}).",
                names.len(),
                kind.plural(),
                kind.what_to_do(),
                names.join(", ")
            )
        })
        .collect()
}

/// Where Desktop keeps its cards, when this machine has them at all.
fn desktop_store_path(config: &Config) -> Option<PathBuf> {
    let path = match &config.desktop_store {
        DesktopStore::Path(path) => PathBuf::from(path),
        DesktopStore::Auto => {
            let home = vibememory_cli::install::home_dir()?;
            vibememory_cli::desktop_store::default_store(&home)
        }
    };
    path.is_dir().then_some(path)
}

/// The named roots of this machine, as the core wants them.
fn roots_of(config: &Config) -> vibememory_core::desktop::roots::Roots {
    vibememory_core::desktop::roots::Roots::new(
        config.roots.clone().into_iter().collect(),
        PathSyntax::Posix,
    )
}

/// The working directory in the form other machines can read, or the local one when no root
/// covers it — a path nobody can translate is still better in a log than nothing.
fn portable_cwd(config: &Config, cwd: &str) -> String {
    roots_of(config)
        .to_portable(cwd)
        .unwrap_or_else(|_| cwd.to_owned())
}

/// Seconds since the epoch, for the push debounce.
fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `UserPromptSubmit`: hold a prompt back only when another machine is provably ahead.
///
/// Every failure path here lets the prompt through. A gate that stops somebody because a file
/// could not be read is a gate that gets switched off, and then it guards nothing.
fn prompt_gate_hook() -> ExitCode {
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).is_err() {
        return ExitCode::SUCCESS;
    }
    let Ok(input) = parse_input(&text) else {
        return ExitCode::SUCCESS;
    };
    let layout = layout();
    let Ok(config) = read_config(&layout) else {
        return ExitCode::SUCCESS;
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        return ExitCode::SUCCESS;
    };

    // An unreadable local transcript must never become a block: `0 lines` would make every other
    // machine look ahead. Not knowing our own state is exactly the doubt this gate resolves in
    // favour of the person typing.
    let local_lines = match std::fs::read(&input.transcript_path) {
        Ok(bytes) => vibememory_core::merge::jsonl::survey(&bytes).lines,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => return ExitCode::SUCCESS,
    };
    let others = other_machines_tails(&store, &config.machine_id, &input.session_id);

    let gate = decide(local_lines, &others);
    match gate {
        Gate::Allow => ExitCode::SUCCESS,
        Gate::Block { .. } => {
            let Some(reason) = gate.reason() else {
                return ExitCode::SUCCESS;
            };
            let payload = serde_json::json!({
                "decision": "block",
                "reason": reason,
            });
            println!("{payload}");
            ExitCode::SUCCESS
        }
    }
}

/// What the other machines report about this session, from what the tick has already fetched.
/// Anything unreadable is simply absent: it may not turn into a block.
fn other_machines_tails(
    store: &std::path::Path,
    own_machine: &str,
    session_id: &str,
) -> Vec<(String, Tail)> {
    let Ok(entries) = std::fs::read_dir(store.join("machines")) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let machine = entry.file_name().to_string_lossy().into_owned();
        if machine == own_machine {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("tails.json")) else {
            continue;
        };
        let Ok(tails) = serde_json::from_str::<Tails>(&text) else {
            continue;
        };
        if let Some(tail) = tails.sessions.get(session_id) {
            found.push((machine, tail.clone()));
        }
    }
    found
}

/// `merge-driver jsonl|keepboth %O %A %B %P` — what git spawns while merging.
///
/// Exit 0 means the result is in `%A`; exit 1 means the merge could not be done and `%A` was left
/// exactly as git wrote it, so the caller can abort with a clean working tree.
fn merge_driver_command(args: &[String]) -> ExitCode {
    let [name, base, ours, theirs, path] = args else {
        eprintln!("usage: vibememory merge-driver <jsonl|keepboth> %O %A %B %P");
        return ExitCode::FAILURE;
    };
    let Some(driver) = vibememory_cli::merge_driver::Driver::parse(name) else {
        eprintln!("unknown driver {name:?}: expected jsonl or keepboth");
        return ExitCode::FAILURE;
    };
    let layout = layout();
    let machine = read_config(&layout).map_or_else(|_| "unknown".to_owned(), |c| c.machine_id);
    let stamp = format!(
        "{machine}-{}",
        vibememory_cli::clock::now().replace(':', "-")
    );

    match vibememory_cli::merge_driver::run(
        driver,
        std::path::Path::new(base),
        std::path::Path::new(ours),
        std::path::Path::new(theirs),
        path,
        &stamp,
    ) {
        Ok(outcome) => {
            // The report is split three ways here: one line to the log always, the parts a person
            // must act on into a file the next session reads, the rest into doctor's warnings.
            if let vibememory_cli::merge_driver::DriverOutcome::Jsonl(report) = &outcome {
                let merged = std::fs::read(std::path::Path::new(ours)).unwrap_or_default();
                let duplicates = vibememory_cli::merge_report::duplicate_message_ids(&merged);
                let routed = vibememory_cli::merge_report::route(path, report, &duplicates);
                let _ = vibememory_cli::merge_report::append_to_log(
                    &layout.engine_dir,
                    &vibememory_cli::clock::now(),
                    &routed.log,
                );
                for note in routed.session.iter().chain(&routed.doctor) {
                    eprintln!("vibememory: {note}");
                }
                let _ = vibememory_cli::merge_report::save_pending(&layout.engine_dir, &routed);
            }
            // The losing version is written to disk here, not inside the driver: the core has no
            // file system, and a version that was only reported would be a version lost.
            if let vibememory_cli::merge_driver::DriverOutcome::KeepBoth(outcome) = &outcome
                && let Some(set_aside) = &outcome.quarantined
            {
                match vibememory_cli::memory::quarantine(
                    &layout.engine_dir,
                    &set_aside.name,
                    &set_aside.bytes,
                ) {
                    Ok(path) => eprintln!("vibememory: set aside {}", path.display()),
                    Err(error) => {
                        // Losing the other machine's version is worse than failing the merge:
                        // the caller aborts, and nothing is lost.
                        eprintln!("vibememory merge-driver: could not quarantine: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            // The message goes to git's stderr, which ends up in the tick's log. `%A` is
            // untouched, so `git merge --abort` restores the tree exactly.
            eprintln!("vibememory merge-driver: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `forget <session-id>` — ask every machine to drop a session.
///
/// Writes a tombstone into this machine's outbox and stops there. The removal itself belongs to
/// the tick: deleting the file here would meet the other machine's copy as a tree conflict that
/// no merge driver is ever asked about.
fn forget_command(session_id: Option<&str>) -> ExitCode {
    let Some(session_id) = session_id else {
        eprintln!("usage: vibememory forget <session-id>");
        return ExitCode::from(2);
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    // The store that holds the session — a team's, when it went to a team — or else the personal
    // one: a tombstone there still stops the session from being brought back from elsewhere
    let personal = vibememory_cli::stores::personal(&layout, &config);
    let teams = vibememory_cli::team_connect::connected_teams(&layout);
    let mut stores = std::iter::once(personal.clone()).chain(
        teams
            .iter()
            .filter_map(|team| vibememory_cli::stores::team(&layout, team).ok()),
    );
    let found = stores.find_map(|store| {
        let clone = std::fs::canonicalize(&store.clone).ok()?;
        let path = find_transcript(&clone, session_id)?;
        Some((vibememory_cli::stores::StoreOf { clone, ..store }, path))
    });
    let (owner, path) = if let Some(found) = found {
        found
    } else {
        let Ok(clone) = std::fs::canonicalize(&personal.clone) else {
            eprintln!("the store is not there yet; run `vibememory install` first");
            return ExitCode::FAILURE;
        };
        (
            vibememory_cli::stores::StoreOf { clone, ..personal },
            String::new(),
        )
    };
    match vibememory_cli::forget::forget(
        &owner.clone,
        &owner.machine_id,
        session_id,
        &path,
        &vibememory_cli::clock::now(),
    ) {
        Ok(()) => {
            println!("recorded: {session_id} will be removed on every machine at its next tick");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("forget: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Where a session's transcript sits inside the store, if this machine has it.
fn find_transcript(store: &std::path::Path, session_id: &str) -> Option<String> {
    let projects = store.join("projects");
    for project in std::fs::read_dir(projects).ok()?.filter_map(Result::ok) {
        let candidate = project.path().join(format!("{session_id}.jsonl"));
        if candidate.exists() {
            let relative = candidate.strip_prefix(store).ok()?;
            return Some(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

/// `tick` — fetch, merge what is safe to merge, push, and keep the store honest.
fn tick_command(args: &[String]) -> ExitCode {
    // One run released from the deletion cap, and only one: passing a rail by weakening it in the
    // config would weaken it for every run after, which is how a guard quietly stops guarding.
    let released = args.iter().any(|arg| arg == "--release-deletions");
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        eprintln!("the store is not there yet; run `vibememory install` first");
        return ExitCode::FAILURE;
    };
    let _lock = match vibememory_cli::tick::TickLock::take(&layout.engine_dir) {
        Ok(lock) => lock,
        Err(error) => {
            // Not an error: the tick runs every two minutes, and one of them being slow is
            // normal.
            println!("skipped: {error}");
            return ExitCode::SUCCESS;
        }
    };

    // when a store paused by its host this run asks the host again
    let recheck = vibememory_cli::clock::iso8601(
        epoch_seconds_signed()
            + i64::try_from(vibememory_cli::tick::PAUSE_RECHECK.as_secs()).unwrap_or(0),
    );
    let roots = roots_of(&config);
    let desktop = desktop_store_path(&config);
    let machine = vibememory_cli::tick::Machine {
        store: &store,
        config_dir: &layout.config_dir,
        machine_id: &config.machine_id,
        roots: &roots,
        naming: &config.naming,
        desktop_store: desktop.as_deref(),
        max_deletions: config.max_deletions_per_tick,
        deletions_released: released,
        team: None,
        routes: &config.routes,
        recheck_at: &recheck,
    };
    let stamp = vibememory_cli::clock::now();
    let cutoff = vibememory_cli::clock::iso8601(
        epoch_seconds_signed()
            - i64::try_from(vibememory_cli::tick::HEARTBEAT_STALE_AFTER.as_secs()).unwrap_or(0),
    );
    let ticked = vibememory_cli::tick::run(&machine, &stamp, &cutoff);
    let moments = Moments {
        stamp: stamp.clone(),
        cutoff: cutoff.clone(),
        recheck: recheck.clone(),
    };
    report_tick(&ticked, config.max_deletions_per_tick);
    let mut failed = !ticked.problems.is_empty();
    for team in vibememory_cli::team_connect::connected_teams(&layout) {
        println!("team {team}:");
        match tick_team(&layout, &config, &roots, &team, released, &moments) {
            Ok(Some(team_ticked)) => {
                report_tick(&team_ticked, config.max_deletions_per_tick);
                if team_ticked.store_cycle == vibememory_cli::tick::StoreCycle::SessionsOff {
                    // the store holds memory only now: the moved projects come back, the clone is
                    // archived; a live session makes this wait for a later tick
                    match vibememory_cli::team_ops::leave(&layout, &config, &team, &stamp) {
                        Ok(left) => report_left(&team, &left),
                        Err(error) => println!("team {team}: leaving waits: {error}"),
                    }
                    continue;
                }
                if let Some(pause) = &team_ticked.pause {
                    println!("{}", pause_advice(&layout, &team, &pause.code));
                }
                failed |= !team_ticked.problems.is_empty();
            }
            Ok(None) => println!("skipped: another tick is running for this team"),
            Err(error) => {
                eprintln!("team {team}: {error}");
                failed = true;
            }
        }
    }
    // Once a day, ask whether the backup still follows the host. Nobody runs `doctor` on a
    // schedule, so without this a mirror could stop following the day after it was set up and
    // nothing would ever say so.
    if let Some(state) = mirror_watch(&layout, &config) {
        println!("mirror: {state}");
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What a person does about a team store's pause: the cabinet for the team's standing, a new
/// clone for a path the store may not hold.
fn pause_advice(layout: &Layout, team: &str, code: &str) -> String {
    if vibememory_core::push_refusal::remedy(code) == vibememory_core::push_refusal::Remedy::Reclone
    {
        return format!(
            "fix:    the clone holds what team {team}'s store may not: `vibememory store reclone {team}`"
        );
    }
    let cabinet = vibememory_cli::team_connect::read_record(layout, team)
        .map(|record| record.cabinet)
        .unwrap_or_default();
    format!("fix:    team {team}'s standing in the cabinet: {cabinet}/team/{team}")
}

/// The moments one tick command works with, read once so every store of the run agrees on them.
struct Moments {
    /// Now.
    stamp: String,
    /// Before this, a heartbeat of this machine is stale.
    cutoff: String,
    /// When a store paused this run asks its host again.
    recheck: String,
}

/// One run over a team's store: its own lock and state beside its clone, the machine named by its
/// name in the team, and no Desktop — cards stay in the personal store. `None` when another run
/// holds the team's lock.
fn tick_team(
    layout: &Layout,
    config: &Config,
    roots: &vibememory_core::desktop::roots::Roots,
    team: &str,
    released: bool,
    moments: &Moments,
) -> Result<Option<vibememory_cli::tick::Ticked>, String> {
    let record = vibememory_cli::team_connect::read_record(layout, team)?;
    let store = std::fs::canonicalize(layout.team_store(team))
        .map_err(|error| format!("the team's clone is not there: {error}"))?;
    let Ok(_lock) = vibememory_cli::tick::TickLock::take(&layout.team_state_dir(team)) else {
        return Ok(None);
    };
    let machine = vibememory_cli::tick::Machine {
        store: &store,
        config_dir: &layout.config_dir,
        machine_id: &record.store_name,
        roots,
        naming: &config.naming,
        desktop_store: None,
        max_deletions: config.max_deletions_per_tick,
        deletions_released: released,
        team: Some(team),
        routes: &config.routes,
        recheck_at: &moments.recheck,
    };
    Ok(Some(vibememory_cli::tick::run(
        &machine,
        &moments.stamp,
        &moments.cutoff,
    )))
}

/// `relink <enc> <name> <cwd>` and `import <enc> <name> <cwd>`.
///
/// Both refuse while any machine reports a live session in that working directory. That check is
/// the whole reason these are commands and not something a hook does quietly.
fn relink_command(args: &[String], import: bool) -> ExitCode {
    let verb = if import { "import" } else { "relink" };
    let [enc, name, cwd] = args else {
        eprintln!("usage: vibememory {verb} <enc> <store-name> <cwd>");
        return ExitCode::from(2);
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        eprintln!("the store is not there yet; run `vibememory install` first");
        return ExitCode::FAILURE;
    };
    let portable = portable_cwd(&config, &canonical_cwd(cwd, PathSyntax::Posix));

    if import {
        match vibememory_cli::relink::import_real_directory(
            &layout.config_dir,
            &store,
            enc,
            name,
            &portable,
            &layout.engine_dir,
            &vibememory_cli::clock::now(),
        ) {
            Ok(imported) => {
                println!(
                    "imported {} file(s) into projects/{name}; {} already in the store, {} set \
                     aside in the quarantine",
                    imported.copied.len(),
                    imported.kept.len(),
                    imported.quarantined.len()
                );
                ExitCode::SUCCESS
            }
            Err(refusal) => {
                eprintln!("import refused: {}", refusal.message());
                ExitCode::FAILURE
            }
        }
    } else {
        match vibememory_cli::relink::relink(&layout.config_dir, &store, enc, name, &portable) {
            Ok(target) => {
                println!("{enc} now points at {}", target.display());
                ExitCode::SUCCESS
            }
            Err(refusal) => {
                eprintln!("relink refused: {}", refusal.message());
                ExitCode::FAILURE
            }
        }
    }
}

/// What one tick did, in the order it did it.
fn report_tick(ticked: &vibememory_cli::tick::Ticked, max_deletions: usize) {
    if let Some(pause) = &ticked.pause {
        println!(
            "paused: the host refused pushes ({}) since {}; asks again at {}{}",
            pause.code,
            pause.since,
            pause.recheck_at,
            if pause.lines.is_empty() {
                String::new()
            } else {
                format!(": {}", pause.lines.join(", "))
            }
        );
    }
    if ticked.merged {
        println!("merged what the other machines wrote");
    }
    for session in &ticked.held_back {
        println!("held back: {session} is live here, so its records wait for it to finish");
    }
    for session in &ticked.forgotten {
        println!("forgotten: {session}");
    }
    // Above the same cap that holds deletions: putting back one file is routine, putting back a
    // hundred means something on this machine is sweeping the store, and the per-file lines would
    // bury that instead of saying it.
    if ticked.restored.len() > max_deletions {
        println!(
            "restored {} transcript(s) that had vanished from the working copy — something on \
             this machine is deleting them; the engine put them back, but find what is removing \
             them",
            ticked.restored.len()
        );
    } else {
        for path in &ticked.restored {
            println!("restored: {path} was deleted in the working copy and put back");
        }
    }
    if ticked.recorded_links > 0 {
        println!(
            "recorded {} link(s) other machines did not know about",
            ticked.recorded_links
        );
    }
    for imported in &ticked.imported_directories {
        println!("imported: {imported}");
    }
    // Said once per directory, not every run: the rule that skips it is the owner's own, and a
    // line repeated every two minutes is a line nobody reads. `status` answers at any time.
    for ignored in &ticked.ignored_directories {
        println!(
            "left alone: {} ({}) — {} transcript(s) stay on this machine only",
            ignored.enc, ignored.reason, ignored.transcripts
        );
    }
    for enc in &ticked.linked {
        println!("linked: {enc}");
    }
    for enc in &ticked.disagreements {
        println!(
            "disagreement: {enc} resolves to a different store name here than on another \
             machine; nothing was changed"
        );
    }
    for project in &ticked.projected_memory {
        println!("memory projected: {project}");
    }
    if ticked.deletions_held > 0 {
        println!(
            "HELD: {} deletion(s) asked for, more than the cap — nothing was deleted. Check whose \
             machines/*/forgotten.json asks for this; to go ahead once, run `vibememory tick \
             --release-deletions`",
            ticked.deletions_held
        );
    }
    match ticked.store_cycle {
        vibememory_cli::tick::StoreCycle::Ran => {}
        vibememory_cli::tick::StoreCycle::Paused => {
            println!("store cycle paused after repeated failures; local work ran as usual");
        }
        vibememory_cli::tick::StoreCycle::Recovered => {
            println!("store cycle recovered: fetching, merging and pushing again");
        }
        vibememory_cli::tick::StoreCycle::SessionsOff => {
            println!("the team's sessions are switched off: this machine leaves its store");
        }
    }
    let managed = &ticked.managed;
    for name in &managed.withheld {
        println!(
            "managed copies: {}",
            vibememory_cli::managed::withheld_reason(name)
        );
    }
    if !managed.pushed.is_empty() || !managed.pulled.is_empty() || !managed.conflicting.is_empty() {
        println!(
            "managed copies: {} pushed, {} pulled, {} conflicting{}",
            managed.pushed.len(),
            managed.pulled.len(),
            managed.conflicting.len(),
            if managed.conflicting.is_empty() {
                String::new()
            } else {
                format!(
                    " ({}; this machine's version is in the quarantine)",
                    managed.conflicting.join(", ")
                )
            }
        );
    }
    if ticked.cards_out > 0
        || ticked.cards_in > 0
        || ticked.cards_repaired > 0
        || ticked.cards_folded > 0
    {
        println!(
            "Desktop cards: {} published, {} brought in, {} repaired, {} folded",
            ticked.cards_out, ticked.cards_in, ticked.cards_repaired, ticked.cards_folded
        );
    }
    if ticked.imported.history_in > 0 || ticked.imported.tasks_in > 0 {
        println!(
            "brought in: {} history line(s), {} task file(s)",
            ticked.imported.history_in, ticked.imported.tasks_in
        );
    }
    if ticked.shared_files_committed > 0 {
        println!("shared files committed: {}", ticked.shared_files_committed);
    }
    if ticked.outbox_committed > 0 {
        println!("outbox committed: {} file(s)", ticked.outbox_committed);
    }
    for session in &ticked.stale_sessions {
        println!("no longer live here: {session} has not been heard from in an hour");
    }
    if ticked.pushed {
        println!("pushed");
    }
    for problem in &ticked.problems {
        eprintln!("problem: {problem}");
    }
}

/// Seconds since the epoch as a signed number, for arithmetic on stamps.
fn epoch_seconds_signed() -> i64 {
    i64::try_from(epoch_seconds()).unwrap_or(i64::MAX)
}

/// `--from <dir>` and an optional `--apply`.
fn migrate_args(args: &[String]) -> Result<(PathBuf, bool), String> {
    let mut from: Option<PathBuf> = None;
    let mut apply_it = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--from" => from = iter.next().map(PathBuf::from),
            "--apply" => apply_it = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    from.map(|source| (source, apply_it))
        .ok_or_else(|| "usage: vibememory migrate --from <dir> [--apply]".to_owned())
}

/// `migrate --from <dir> [--apply]` — the old synced folder into the store.
///
/// Without `--apply` nothing is read but metadata: the plan says what would go where, and how
/// many files are cloud placeholders that must be pinned first. The source is never written to.
fn migrate_command(args: &[String]) -> ExitCode {
    let (source, apply_it) = match migrate_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        eprintln!("the store is not there yet; run `vibememory install` first");
        return ExitCode::FAILURE;
    };

    let plan = match vibememory_cli::migrate::plan(&source, &config.machine_id) {
        Ok(plan) => plan,
        Err(error) => {
            eprintln!("migrate: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "plan: {} file(s), {} MiB; {} left in the archive-only store; {} refused by the export \
         gate; {} cloud placeholder(s) not on this disk",
        plan.files.len(),
        plan.bytes / (1024 * 1024),
        plan.archived,
        plan.refused.len(),
        plan.dehydrated.len()
    );
    for (path, rule) in plan.refused.iter().take(20) {
        println!("  refused: {path} — {rule}");
    }
    if plan.refused.len() > 20 {
        println!("  … and {} more", plan.refused.len() - 20);
    }
    if !config.naming.ignores("/") {
        println!(
            "note: the archive-only store holds sessions with cwd `/`; add \"/\" to ignoreCwd in \
             config.json so the hook leaves such sessions alone"
        );
    }
    if !apply_it {
        println!("dry run: nothing was written. Add --apply to migrate.");
        return if plan.dehydrated.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    match vibememory_cli::migrate::apply(
        &source,
        &store,
        &layout.engine_dir,
        &plan,
        &vibememory_cli::clock::now(),
    ) {
        Ok(applied) => {
            println!(
                "copied {}, unchanged {}, merged {} transcript(s), quarantined {}, repaired {} \
                 card(s), committed: {}",
                applied.copied,
                applied.unchanged,
                applied.merged.len(),
                applied.quarantined.len(),
                applied.repaired_cards.len(),
                applied.committed
            );
            for (path, ours, theirs) in &applied.merged {
                println!("  merged: {path} (+{ours} store, +{theirs} source)");
            }
            for path in &applied.quarantined {
                println!("  set aside: {path}");
            }
            for path in &applied.held {
                println!("  held on this machine, it holds an agent token: {path}");
            }
            if applied.mismatched.is_empty() {
                ExitCode::SUCCESS
            } else {
                for path in &applied.mismatched {
                    eprintln!(
                        "MISMATCH: {path} does not hash to its source; nothing was committed"
                    );
                }
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("migrate: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `switch --from <dir> [--apply|--rollback]` — this machine from the old synced folder to the
/// store. Without `--apply` it only says what it would do.
fn switch_command(args: &[String]) -> ExitCode {
    let layout = layout();
    if args.iter().any(|arg| arg == "--rollback") {
        return match vibememory_cli::switch::rollback(&layout.engine_dir) {
            Ok(undone) => {
                println!("rolled back {} change(s)", undone.len());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rollback: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let (from, apply_it) = match migrate_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{}", message.replace("migrate", "switch"));
            return ExitCode::from(2);
        }
    };
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let Ok(store) = std::fs::canonicalize(layout.store()) else {
        eprintln!("the store is not there yet; run `vibememory install` first");
        return ExitCode::FAILURE;
    };
    let desktop = desktop_store_link(&config);
    let input = vibememory_cli::switch::SwitchInput {
        config_dir: &layout.config_dir,
        store: &store,
        from: &from,
        desktop_store: desktop.as_deref(),
        desktop_running: desktop_is_running(&layout.config_dir),
        engine_dir: &layout.engine_dir,
    };
    match vibememory_cli::switch::switch(&input, !apply_it) {
        Ok(done) => {
            let verb = if apply_it { "did" } else { "would" };
            for change in &done.changes {
                println!("{verb}: {change:?}");
            }
            for (path, why) in &done.skipped {
                println!("left alone: {} — {why}", path.display());
            }
            for path in &done.real_directories {
                println!(
                    "real directory, use `vibememory import`: {}",
                    path.display()
                );
            }
            if !apply_it {
                println!("dry run: nothing was changed. Add --apply to switch.");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("switch: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The Desktop store path even when it is still a symlink (the tick wants a directory; the
/// switch wants the link itself).
fn desktop_store_link(config: &Config) -> Option<PathBuf> {
    match &config.desktop_store {
        DesktopStore::Path(path) => Some(PathBuf::from(path)),
        DesktopStore::Auto => {
            let home = vibememory_cli::install::home_dir()?;
            Some(vibememory_cli::desktop_store::default_store(&home))
        }
    }
}

/// Whether Claude Desktop is running: its process in the process list, or a session it spawned
/// still alive in the CLI's registry. Either is enough — on the first real run `pgrep` missed the
/// Desktop process entirely while two of its sessions were plainly there.
fn desktop_is_running(config_dir: &std::path::Path) -> bool {
    let in_process_list = std::process::Command::new("ps")
        .args(["-axo", "comm"])
        .output()
        .is_ok_and(|output| {
            String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                line.trim_end()
                    .ends_with("/Claude.app/Contents/MacOS/Claude")
            })
        });
    in_process_list || vibememory_cli::switch::desktop_session_live(config_dir)
}

/// Where this project's memory actually lives on this machine.
///
/// Not `config_dir/projects/<enc>/memory`: the CLI lets that be redirected — by the cowork
/// variable, by a remote memory directory, by `settings.json` — and code that guesses the path
/// instead of asking looks into an empty directory and reports that nothing is there.
fn project_memory_dir(layout: &Layout, enc: &EncSlug) -> PathBuf {
    let cowork = std::env::var(COWORK_MEMORY_VAR).ok();
    let remote = std::env::var(REMOTE_MEMORY_VAR).ok();
    let settings = settings_memory_dir(&layout.config_dir.join("settings.json"));
    let location = MemoryLocation {
        cowork_override: cowork.as_deref(),
        remote_dir: remote.as_deref(),
        settings_dir: settings.as_deref(),
    };
    memory_dir(&location, &layout.config_dir, enc.as_str())
}

/// Note about this project's open hand-offs, or `None` when there are none.
///
/// Read from the hand-off files themselves: the index in `MEMORY.md` is rewritten back to the
/// CLI's default template, so a session that trusted it would start believing nothing is in
/// progress.
fn handoff_note(layout: &Layout, enc: &EncSlug) -> Option<String> {
    let open = vibememory_cli::memory::open_handoffs(&project_memory_dir(layout, enc));
    if open.is_empty() {
        return None;
    }
    Some(format!(
        "VibeMemory: {} hand-off(s) of this project are open \u{2014} say \u{ab}\u{43f}\u{440}\u{43e}\u{434}\u{43e}\u{43b}\u{436}\u{438}\u{bb} to resume: {}.",
        open.len(),
        open.join(", ")
    ))
}

/// Asks the host whether its backup mirror still holds what the host holds.
fn probe_mirror(config: &Config) -> vibememory_cli::mirror::Mirror {
    vibememory_cli::mirror::probe(
        config.remote.as_deref(),
        vibememory_cli::tick::BRANCH,
        ask_host,
    )
}

/// One question put to the host over ssh.
fn ask_host(host: &str, script: &str) -> Result<String, String> {
    match vibememory_cli::git::run_with_timeout(
        ssh_command(host, script),
        vibememory_cli::mirror::PROBE_TIMEOUT,
    ) {
        Ok(Some(text)) => Ok(text),
        Ok(None) => Err("\u{445}\u{43e}\u{441}\u{442} \u{43e}\u{442}\u{432}\u{435}\u{442}\u{438}\u{43b} \u{43e}\u{448}\u{438}\u{431}\u{43a}\u{43e}\u{439}".to_owned()),
        Err(reason) => Err(reason),
    }
}

/// An ssh command that never asks the human anything: `doctor` may run from a launchd tick where
/// there is nobody to answer a passphrase prompt.
fn ssh_command(host: &str, script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("ssh");
    command
        .args(["-o", "BatchMode=yes", host, script])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

/// The daily mirror check: asks the host, tells the next session when the backup fell behind,
/// and returns the sentence for this run's log. The column `doctor` aligns its report in belongs
/// to that report, not to the tick's log.
///
/// A network failure does not count as an answer — the timer is not reset, so the question is
/// asked again on the next tick instead of a day later.
fn mirror_watch(layout: &Layout, config: &Config) -> Option<String> {
    use vibememory_cli::mirror::{Mirror, probe, session_note, should_probe};

    let now = epoch_seconds_signed();
    let state = MirrorState::read(&layout.engine_dir);
    if !should_probe(state.last_checked, state.last_attempt, now) {
        return None;
    }
    let mirror = probe(
        config.remote.as_deref(),
        vibememory_cli::tick::BRANCH,
        ask_host,
    );
    let answered = !matches!(mirror, Mirror::Unknown { .. });
    let _ = MirrorState {
        last_checked: answered.then_some(now).or(state.last_checked),
        last_attempt: Some(now),
    }
    .write(&layout.engine_dir);
    if let Some(note) = session_note(&mirror) {
        // The tick has no session to talk to; the note waits for one, next to the merge notes.
        let _ = vibememory_cli::merge_report::add_pending(&layout.engine_dir, &note);
    }
    // The host's disk, on the same daily clock: a day's warning is what a filling disk allows.
    if answered
        && let Some(Ok(disk)) =
            vibememory_cli::mirror::probe_disk(config.remote.as_deref(), ask_host)
        && let Some(note) = vibememory_cli::mirror::disk_note(&disk)
    {
        let _ = vibememory_cli::merge_report::add_pending(&layout.engine_dir, &note);
    }
    Some(mirror.summary())
}

/// When the mirror was last asked about, kept next to the engine's other small states.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MirrorState {
    /// When the mirror last gave an answer of any kind.
    last_checked: Option<i64>,
    /// When it was last asked, answer or not — the back-off for an unreachable host.
    last_attempt: Option<i64>,
}

impl MirrorState {
    const FILE: &'static str = "mirror-state.json";

    fn read(engine_dir: &std::path::Path) -> Self {
        std::fs::read_to_string(engine_dir.join(Self::FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn write(&self, engine_dir: &std::path::Path) -> Result<(), String> {
        // A machine that has not been installed yet has no engine directory, and a tick there
        // must not fail on the state file it was only trying to leave behind.
        std::fs::create_dir_all(engine_dir).map_err(|error| error.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(engine_dir.join(Self::FILE), text).map_err(|error| error.to_string())
    }
}

/// The session files this machine keeps back, as `--json` gives them: the path, the public ids of
/// the tokens they hold, and since when.
fn held_json(engine_dir: &std::path::Path) -> Vec<serde_json::Value> {
    vibememory_cli::held::Held::read(engine_dir)
        .files
        .into_iter()
        .map(|(path, file)| {
            serde_json::json!({ "path": path, "tokens": file.tokens, "since": file.since })
        })
        .collect()
}

/// Whether the caller wants the machine-readable form.
fn wants_json(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--json")
}

/// `status`/`doctor` for something that is not a person: a monitor, a dashboard, a script.
///
/// The exit code is the same as the human form's, so a check can use either.
fn report_json(
    config: &Config,
    engine_dir: &std::path::Path,
    actions: &[vibememory_cli::install::Action],
    mirror: Option<&vibememory_cli::mirror::Mirror>,
    disk: Option<&Result<vibememory_cli::mirror::Disk, String>>,
    tokens: &[vibememory_cli::credentials::KeptToken],
    strict: bool,
) -> ExitCode {
    use vibememory_cli::mirror::Mirror;

    let steps = steps_json(actions);
    let wrong = actions
        .iter()
        .filter(|action| !matches!(action.state, State::Satisfied))
        .count();
    let mirror_value = mirror.map_or_else(
        // `status` does not go to the network, so it says so instead of implying the mirror is
        // fine — a monitor reading `null` would have to guess which of the two it means.
        || serde_json::json!({ "state": "notChecked" }),
        |mirror| match mirror {
            Mirror::NoHost => serde_json::json!({ "state": "noHost" }),
            Mirror::NotConfigured => serde_json::json!({ "state": "notConfigured" }),
            Mirror::InSync { repo, head } => {
                serde_json::json!({ "state": "inSync", "repo": repo, "head": head })
            }
            Mirror::Diverged { repo, host, mirror } => serde_json::json!({
                "state": "diverged", "repo": repo, "hostHead": host, "mirrorHead": mirror
            }),
            Mirror::Unknown { reason } => {
                serde_json::json!({ "state": "unknown", "reason": reason })
            }
        },
    );
    let disk_value = match disk {
        None => serde_json::json!({ "state": "notChecked" }),
        Some(Ok(disk)) => serde_json::json!({
            "state": if disk.is_low() { "low" } else { "ok" },
            "availableKib": disk.available_kib,
            "totalKib": disk.total_kib,
        }),
        Some(Err(reason)) => serde_json::json!({ "state": "unknown", "reason": reason }),
    };
    let disk_low = matches!(disk, Some(Ok(disk)) if disk.is_low());
    let tokens_wrong = tokens.iter().any(|token| !token.problems.is_empty());
    let teams = vibememory_cli::team_connect::team_facts(&vibememory_cli::install::Layout {
        config_dir: std::path::PathBuf::new(),
        engine_dir: engine_dir.to_path_buf(),
    });
    let teams_wrong = teams
        .iter()
        .any(|facts| facts.pause.is_some() || !facts.problems.is_empty());
    let failed = wrong > 0
        || mirror.is_some_and(Mirror::is_fault)
        || disk_low
        || tokens_wrong
        || teams_wrong;
    let report = serde_json::json!({
        "machineId": config.machine_id,
        "remote": config.remote,
        "steps": steps,
        "stepsWrong": wrong,
        "mirror": mirror_value,
        "hostDisk": disk_value,
        "credentials": tokens_json(tokens),
        "held": held_json(engine_dir),
        "teams": teams_json(&teams),
        "ok": !failed,
    });
    match serde_json::to_string_pretty(&report) {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("report: {error}");
            return ExitCode::from(2);
        }
    }
    if strict && failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// What the engine answers to, printed for a human who asked for nothing in particular.
fn usage() {
    println!("vibememory {}", env!("CARGO_PKG_VERSION"));
    println!(
        "commands: status [--json], doctor [--json], install [--dry-run], \
         connect --cabinet <address> [--agent <name>], disconnect <team>, hook <event>, \
         merge-driver <jsonl|keepboth> %O %A %B %P, forget <session-id>, tick [--release-deletions], \
         relink <enc> <name> <cwd>, import <enc> <name> <cwd>, \
         migrate --from <dir> [--apply], switch --from <dir> [--apply|--rollback], --version"
    );
}
