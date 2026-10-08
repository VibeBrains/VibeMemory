//! The binary. Everything it decides lives in the library next to it.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

mod rules_command;

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

use vibememory_cli::agents::{Handler, Preset};
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
        Some("update") => update_command(),
        Some("mcp-config") => mcp_config_command(args.next().as_deref()),
        Some("migrate") => migrate_command(&args.collect::<Vec<String>>()),
        Some("switch") => switch_command(&args.collect::<Vec<String>>()),
        Some("relink") => relink_command(&args.collect::<Vec<String>>(), false),
        Some("import") => relink_command(&args.collect::<Vec<String>>(), true),
        Some("forget") => forget_command(args.next().as_deref()),
        Some("session") => session_command(&args.collect::<Vec<String>>()),
        Some("project") => project_command(&args.collect::<Vec<String>>()),
        Some("route") => route_command(&args.collect::<Vec<String>>()),
        Some("rule") => rules_command::rule(&layout(), &args.collect::<Vec<String>>()),
        Some("skill") => rules_command::skill(&layout(), &args.collect::<Vec<String>>()),
        Some("rules") => rules_command::rules(&layout(), &args.collect::<Vec<String>>()),
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
    vibememory_cli::install::engine_configured(layout)
}

/// `status` prints where the machine stands; `doctor` does the same and fails when something is
/// not as it must be.
fn report(strict: bool, json: bool) -> ExitCode {
    let layout = layout();
    let tokens = vibememory_cli::credentials::kept_tokens(&layout);
    if !json {
        print_version(&layout);
    }
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
            &layout,
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
    // Which agents hand sessions over is read once: the answer is the same for the credentials
    // section, for the clients no token names, and for the lines about other agents' sessions.
    let sessions = foreign_sessions(&layout);
    let readings = vibememory_cli::agents::Readings::with_delivered(
        &layout,
        sessions
            .iter()
            .map(|(_, agent, _, _)| agent.clone())
            .collect(),
    );
    print_kept_here(&layout, &sessions);
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
    wrong += print_tokens(&tokens, &readings);
    print_clients(&layout, &tokens, &readings);
    wrong += print_watched_agents(&layout);
    wrong += print_teams(&layout);
    print_rules(&layout);
    print_advice(&layout, &config);
    if strict && wrong > 0 {
        eprintln!(
            "{wrong} of {} steps are not in place",
            actions.len() + tokens.len()
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// What is not a failure but most likely not what was meant: routes that differ from the disk,
/// claims of machines unheard of for a week, and memory servers the agent has to guess between.
fn print_advice(layout: &Layout, config: &Config) {
    for line in vibememory_cli::route::warnings(config, &layout.store()) {
        println!("route    {}", vibememory_core::terminal::printable(&line));
    }
    print_forgotten_claims(layout);
    let registered = vibememory_cli::registrations::read(layout);
    for line in vibememory_cli::registrations::advice(layout, config, &registered) {
        println!("mcp      {line}");
    }
}

/// How long another machine's claim may go unrenewed before `doctor` offers to release it. A week
/// is far past any clock drift and any closed lid; it is a machine nobody runs any more.
const FORGOTTEN_CLAIM_AFTER_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Other machines' claims unrenewed for a week, in every store, with the command that clears them.
/// Never cleared here: whether that machine is gone is its owner's word.
fn print_forgotten_claims(layout: &Layout) {
    let cutoff =
        vibememory_cli::clock::iso8601(epoch_seconds_signed() - FORGOTTEN_CLAIM_AFTER_SECONDS);
    let mut stores = vec![layout.store()];
    stores.extend(
        vibememory_cli::team_connect::connected_teams(layout)
            .iter()
            .map(|team| layout.team_store(team)),
    );
    for store in stores {
        for claim in vibememory_cli::relink::live_everywhere(&store) {
            if claim.here || claim.mark.at.as_str() >= cutoff.as_str() {
                continue;
            }
            println!(
                "live     session {} on {} — last renewed {}; if {} is gone for good: vibememory session \
                 release {} {} --confirm",
                vibememory_core::terminal::printable(&claim.session),
                vibememory_core::terminal::printable(&claim.machine),
                claim.mark.at,
                vibememory_core::terminal::printable(&claim.machine),
                vibememory_core::terminal::printable(&claim.machine),
                vibememory_core::terminal::printable(&claim.session)
            );
        }
    }
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
                    "lastFailure": facts.last_failure,
                })
            })
            .collect(),
    )
}

/// The person's rules and skills: how many, which agents of this machine get them, and the files an agent would
/// refuse. Not a failure: a broken rule file is skipped, the others still reach the agents.
fn print_rules(layout: &Layout) {
    let store = layout.store();
    let settings = vibememory_cli::config::RulesConfig::of_engine(&layout.engine_dir);
    let (rules, _) =
        vibememory_cli::rules::read_rules(&store.join(vibememory_cli::rules::PERSONAL_RULES));
    let agents =
        vibememory_cli::rules::Agents::of(&layout.config_dir, layout.home.as_deref(), &settings);
    let mut names = vec![vibememory_cli::rules::CLAUDE_NAME];
    names.extend(agents.assembled.iter().map(|(agent, _)| *agent));
    let long = rules
        .values()
        .filter(|rule| rule.body.len() > settings.long_rule_bytes)
        .count();
    println!(
        "rules    {} personal rule(s) for {}{}",
        rules.len(),
        names.join(", "),
        if long > 0 {
            format!("; {long} long — `vibememory rules lint`")
        } else {
            String::new()
        }
    );
    for warning in vibememory_cli::rules::check(&store, &agents) {
        println!("         {warning}");
    }
    // a team's rule waits for a session in its project: without one it would wait unseen
    for team in vibememory_cli::team_connect::connected_teams(layout) {
        let waiting = vibememory_cli::rules_shown::team_rules(&layout.team_store(&team), false)
            .waiting
            .len();
        if waiting > 0 {
            println!(
                "         team {team}: {waiting} new or changed rule(s) wait to be shown — `vibememory rules status` \
                 in its project shows them"
            );
        }
    }
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
        if facts.pause.is_none()
            && facts.failures > 0
            && let Some(why) = &facts.last_failure
        {
            println!("         last: {why}");
        }
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

/// What stays on this machine by rule, and what other agents handed over: never a failure, and
/// said because silence about it would look the same as nothing being there.
fn print_kept_here(layout: &Layout, sessions: &[(String, String, usize, String)]) {
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
    print_foreign_sessions(sessions);
}

/// The first line of `status` and `doctor`: which version runs, and a newer one the tick heard of.
fn print_version(layout: &Layout) {
    match vibememory_cli::update::newer_known(layout) {
        Some(newer) => println!(
            "version  vibememory {} \u{2014} {newer} is out: vibememory update",
            vibememory_cli::update::CURRENT
        ),
        None => println!("version  vibememory {}", vibememory_cli::update::CURRENT),
    }
}

/// The sessions other agents handed over with `session put`, by store and agent: a wrapper that
/// stopped working shows as a date that no longer moves.
fn print_foreign_sessions(sessions: &[(String, String, usize, String)]) {
    for (store, agent, sessions, newest) in sessions {
        println!(
            "agent    {} \u{2014} {sessions} session(s) in the {store} store, newest written {newest}",
            vibememory_core::terminal::printable(agent)
        );
    }
}

/// Every store's sessions of other agents: the store (`personal` or `team <id>`), the agent, how
/// many sessions and when the newest was written. One list for the text report and the JSON one.
fn foreign_sessions(layout: &Layout) -> Vec<(String, String, usize, String)> {
    let mut stores = vec![("personal".to_owned(), layout.store())];
    stores.extend(
        vibememory_cli::team_connect::connected_teams(layout)
            .into_iter()
            .map(|team| (format!("team {team}"), layout.team_store(&team))),
    );
    let mut found = Vec::new();
    for (store, clone) in stores {
        for (agent, delivered) in vibememory_cli::foreign_session::delivered(&clone) {
            let newest = i64::try_from(delivered.newest).unwrap_or(i64::MAX);
            found.push((
                store.clone(),
                agent,
                delivered.sessions,
                vibememory_cli::clock::iso8601(newest),
            ));
        }
    }
    found
}

/// The agents a memory server started as here and that no kept token names: a client of the
/// personal store needs no token, and this is the only place it shows.
fn print_clients(
    layout: &Layout,
    tokens: &[vibememory_cli::credentials::KeptToken],
    readings: &vibememory_cli::agents::Readings,
) {
    for (agent, start) in vibememory_cli::credentials::started_clients(&layout.engine_dir) {
        if tokens.iter().all(|token| token.agent != agent) {
            println!(
                "client   {} \u{2014} a memory server started as this agent {}",
                vibememory_core::terminal::printable(&agent),
                describe_start(&start)
            );
            if let Some(note) = readings.note(&agent) {
                println!("         {note}");
            }
        }
    }
}

/// When and as which version a memory server started, and what to do when it is older than the
/// engine on this disk: it runs the old binary until its client starts it again.
fn describe_start(start: &vibememory_cli::credentials::ClientStart) -> String {
    let version = start
        .version
        .as_deref()
        .map_or_else(String::new, |version| format!(" (version {version})"));
    let behind = if start.behind(vibememory_cli::update::CURRENT) {
        format!(
            " \u{2014} older than vibememory {} on this disk: restart the client so it starts the \
             new server",
            vibememory_cli::update::CURRENT
        )
    } else {
        String::new()
    };
    format!("at {}{version}{behind}", start.stamp)
}

/// The credentials section: each kept token, and what is wrong with its files. Answers how many
/// tokens have something wrong.
fn print_tokens(
    tokens: &[vibememory_cli::credentials::KeptToken],
    readings: &vibememory_cli::agents::Readings,
) -> usize {
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
        // Not a failure: a token may be taken before its client is set up. It is said because a
        // token nobody uses looks exactly like a working one everywhere else.
        match &token.client_started {
            Some(start) => {
                println!(
                    "         client: a memory server started as --agent {} {}",
                    vibememory_core::terminal::printable(&token.agent),
                    describe_start(start)
                );
                if let Some(note) = readings.note(&token.agent) {
                    println!("         {note}");
                }
            }
            None => println!(
                "         client: no memory server has started as --agent {} on this machine; \
                 the client that uses this token must pass exactly this name",
                vibememory_core::terminal::printable(&token.agent)
            ),
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
            "version": vibememory_cli::update::CURRENT,
            "newerRelease": vibememory_cli::update::newer_known(layout),
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
        print_tokens(tokens, &vibememory_cli::agents::Readings::default());
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
                "clientStarted": token.client_started.as_ref().map(|start| &start.stamp),
                "clientVersion": token.client_started.as_ref().and_then(|start| start.version.as_ref()),
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
    if let [flag] = args
        && flag == "--key-request"
    {
        return key_request_command();
    }
    if args.first().is_some_and(|flag| flag == "--grant") {
        return grant_command(args.get(1..).unwrap_or_default());
    }
    let ConnectArguments {
        cabinet: asked,
        agent,
        machine_id,
    } = match connect_arguments(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("connect: {error}");
            eprintln!(
                "usage: vibememory connect --cabinet <address> [--agent <name>] [--machine-id <name>], and the code at \
                 the prompt; \
                 vibememory connect --refresh <team> after the host changed its key; \
                 on a host without the cabinet: vibememory connect --key-request, then \
                 vibememory connect --grant [--agent <name>] [--machine-id <name>] with the grant on stdin"
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
    if let Some(handed_over) = update_before_connect(&cabinet, args) {
        return handed_over;
    }
    let Some(code) = claim_code() else {
        return ExitCode::from(2);
    };
    let layout = layout();
    // a key goes with every code; without ssh-keygen only a code for a machine is refused
    let pending = vibememory_cli::team_connect::PendingKey::make(&layout).ok();
    // the name the machine goes by in the store, or its network name on a machine without one
    let machine = read_config(&layout).map_or_else(
        |_| vibememory_cli::personal_connect::machine_id(machine_id.as_deref()),
        |config| Some(config.machine_id),
    );
    let reply = match vibememory_cli::connect::ask_cabinet(
        &cabinet,
        &code,
        pending.as_ref().map(|key| key.public.as_str()),
        machine.as_deref(),
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
            if grant.mode == vibememory_core::team_store::PERSONAL_MODE {
                return connect_personal(&layout, &grant, pending, machine_id.as_deref());
            }
            return connect_key(&layout, &grant, pending, machine_id.as_deref());
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
    keep_granted_token(&layout, &grant, agent.as_deref())
}

/// `connect --key-request`: the key of this machine for a host without the cabinet. Its public half
/// is printed for the host's owner, who registers it with `vibememory-mcp admin key add`; the grant
/// that answers comes back through `connect --grant`. Asked again, it shows the same key.
fn key_request_command() -> ExitCode {
    let layout = layout();
    match vibememory_cli::team_connect::PendingKey::for_grant(&layout) {
        Ok(key) => {
            eprintln!(
                "This machine's key for the host. Give the line below to the host's owner: it goes to \
                 `vibememory-mcp admin key add <member> <machine> <team>` on its stdin. It is the public half, \
                 safe to send"
            );
            println!("{}", key.public);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("connect: the key could not be made: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `connect --grant [--agent <name>] [--machine-id <name>]`: what the console of a host without the
/// cabinet issued — one line of JSON on stdin, never an argument, since a token grant holds the
/// token — taken exactly as the cabinet's answer to a claim code would be.
fn grant_command(args: &[String]) -> ExitCode {
    let mut agent = None;
    let mut machine_id = None;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.as_str() {
            "--agent" => &mut agent,
            "--machine-id" => &mut machine_id,
            other => {
                eprintln!(
                    "connect: unknown {other:?}; usage: vibememory connect --grant [--agent <name>] [--machine-id <name>], and the grant on stdin"
                );
                return ExitCode::from(2);
            }
        };
        let Some(value) = rest.next() else {
            eprintln!("connect: {flag} takes a value");
            return ExitCode::from(2);
        };
        *slot = Some(value.clone());
    }
    eprintln!("Paste the grant the host's owner gave you, then Enter:");
    let mut line = String::new();
    if let Err(error) = std::io::BufRead::read_line(&mut std::io::stdin().lock(), &mut line) {
        eprintln!("connect: the grant could not be read: {error}");
        return ExitCode::FAILURE;
    }
    let line = line.trim();
    // the grant names the host it came from; the checks hold every other address to it
    let named = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("cabinet")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let Some(host) = named.and_then(|named| vibememory_core::claim::cabinet_address(&named).ok())
    else {
        eprintln!(
            "connect: this is not a grant: one line of JSON, as `vibememory-mcp admin` printed it"
        );
        return ExitCode::FAILURE;
    };
    let layout = layout();
    let answer = vibememory_core::claim::read_answer(0, 200, line, &host);
    match answer {
        Ok(vibememory_core::claim::Claim::Token(grant)) => {
            keep_granted_token(&layout, &grant, agent.as_deref())
        }
        Ok(vibememory_core::claim::Claim::Key(grant)) => {
            let Some(pending) =
                vibememory_cli::team_connect::PendingKey::waiting_for_grant(&layout)
            else {
                eprintln!(
                    "connect: this grant is for a machine key, and this machine has none waiting: run \
                     `vibememory connect --key-request`, have key {} revoked and the new key added",
                    grant.key_id
                );
                return ExitCode::FAILURE;
            };
            if grant.mode == vibememory_core::team_store::PERSONAL_MODE {
                return connect_personal(&layout, &grant, pending, machine_id.as_deref());
            }
            connect_key(&layout, &grant, pending, machine_id.as_deref())
        }
        Err(failure) => {
            eprintln!("connect: the grant is refused: {failure}");
            ExitCode::FAILURE
        }
    }
}

/// The end of a `connect` that brought a token: it is kept, and the line that registers the client
/// is printed. An agent name asked for that is not the code's is said, not obeyed.
fn keep_granted_token(
    layout: &Layout,
    grant: &vibememory_core::claim::TokenGrant,
    agent: Option<&str>,
) -> ExitCode {
    if let Some(agent) = agent
        && agent != grant.agent
    {
        eprintln!(
            "note: the code was made for agent {}, not {agent}: the token is kept as {}'s",
            grant.agent, grant.agent
        );
    }
    let kept = match vibememory_cli::connect::keep_token(layout, grant) {
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
    print_connected(grant, &kept);
    ExitCode::SUCCESS
}

/// Before the code is asked for: when the cabinet's host publishes a newer release, installs it and
/// runs `connect` again with the new program, which then asks for the code itself. A newer cabinet
/// may hand out what an older engine cannot read, and a code is spent the moment it is read.
///
/// `None` when this program goes on: nothing newer, the host unreadable, or the update failing —
/// said in one line, since the version running may well be enough.
fn update_before_connect(cabinet: &str, args: &[String]) -> Option<ExitCode> {
    if std::env::var_os(vibememory_cli::update::NO_SELF_UPDATE_VAR).is_some() {
        return None;
    }
    let layout = layout();
    let base = std::env::var(vibememory_cli::update::RELEASES_VAR)
        .ok()
        .filter(|base| !base.trim().is_empty())
        .unwrap_or_else(|| vibememory_cli::update::base_of_cabinet(cabinet));
    let newer = match vibememory_cli::update::latest(&base) {
        Ok(version)
            if vibememory_core::release::is_newer(&version, vibememory_cli::update::CURRENT) =>
        {
            version
        }
        Ok(_) => return None,
        Err(error) => {
            eprintln!(
                "connect: could not ask for a newer release ({error}); going on with this one"
            );
            return None;
        }
    };
    eprintln!(
        "connect: vibememory {newer} is out (this is {}); updating first",
        vibememory_cli::update::CURRENT
    );
    if let Err(error) = vibememory_cli::update::update(&layout, &base) {
        eprintln!("connect: the update failed ({error}); going on with this version");
        return None;
    }
    let installed = vibememory_cli::install::installed_binary(&layout);
    let status = std::process::Command::new(&installed)
        .arg("connect")
        .args(args)
        .env(vibememory_cli::update::NO_SELF_UPDATE_VAR, "1")
        .status();
    Some(match status {
        Ok(status) => status
            .code()
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        Err(error) => {
            eprintln!(
                "connect: {} could not be started: {error}",
                installed.display()
            );
            ExitCode::FAILURE
        }
    })
}

/// The claim code, asked for at the terminal or read from a pipe; `None` after saying why not.
fn claim_code() -> Option<String> {
    let stdin = std::io::stdin();
    if std::io::IsTerminal::is_terminal(&stdin) {
        eprint!("claim code: ");
    }
    match vibememory_cli::connect::read_code(stdin.lock()) {
        Ok(code) => Some(code),
        Err(error) => {
            eprintln!("connect: {error}");
            None
        }
    }
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
    // an answer this version cannot read is most often one a newer version can
    if matches!(failure, vibememory_core::claim::ClaimFailure::Malformed(_)) {
        eprintln!(
            "connect: this is vibememory {}, and it does not know this answer; `vibememory \
             update`, then a new code",
            vibememory_cli::update::CURRENT
        );
    }
}

/// Writes `config.json` with the machine's id on a machine that has none: the one asked for with `--machine-id`,
/// Or the machine's network name, by the rule the personal store's connection follows
fn configure_engine(
    layout: &vibememory_cli::install::Layout,
    asked: Option<&str>,
) -> Result<(), String> {
    let machine_id = vibememory_cli::personal_connect::machine_id(asked).ok_or_else(|| {
        "no machine id: name this machine with --machine-id <name> (letters, digits, - _ .)"
            .to_owned()
    })?;
    std::fs::create_dir_all(&layout.engine_dir).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(&serde_json::json!({ "machineId": machine_id }))
        .map_err(|error| error.to_string())?;
    let path = layout.engine_dir.join("config.json");
    vibememory_cli::route::write_config(&path, &format!("{text}\n"))?;
    println!(
        "config:    {} written, machine {machine_id}",
        path.display()
    );
    Ok(())
}

/// A personal store answer: the key kept, the main store cloned or re-aimed, `config.json` written
/// when there was none, and the full install — hooks, schedule, links, PATH — on top.
fn connect_personal(
    layout: &vibememory_cli::install::Layout,
    grant: &vibememory_core::claim::KeyGrant,
    pending: vibememory_cli::team_connect::PendingKey,
    machine_id: Option<&str>,
) -> ExitCode {
    let connected = match vibememory_cli::personal_connect::connect(
        layout, grant, pending, machine_id,
    ) {
        Ok(connected) => connected,
        Err(refusal) => {
            eprintln!("connect: {refusal}");
            eprintln!(
                "connect: key {} is registered for this machine on the host: revoke it in the cabinet, or with \
                 `vibememory-mcp admin key revoke` on a host without one",
                grant.key_id
            );
            return ExitCode::FAILURE;
        }
    };
    println!(
        "connected: the personal store of {} as machine {}",
        grant.member, connected.machine_id
    );
    println!(
        "store:     {}{}",
        connected.store.display(),
        if connected.cloned {
            ""
        } else {
            " (kept, now on the new key)"
        }
    );
    if connected.configured {
        println!(
            "config:    {} written",
            layout.engine_dir.join("config.json").display()
        );
    }
    let installed = install(false);
    print_agent_hints(layout);
    println!(
        "next:      open a new terminal, run vibememory doctor, and sign in to Claude Code again"
    );
    installed
}

/// The end of a `connect` that set up a store: the line that registers the memory server with
/// Claude Code when it is not registered, and where the rest of the clients are told how.
/// Registering stays the person's: every client keeps it in a file of its own.
fn print_agent_hints(layout: &Layout) {
    if let Ok(config) = read_config(layout) {
        let registered = vibememory_cli::registrations::read(layout);
        for line in vibememory_cli::registrations::advice(layout, &config, &registered) {
            println!("mcp       {line}");
        }
    }
    let others: Vec<&str> = vibememory_cli::mcp_config::CLIENTS
        .iter()
        .filter(|client| **client != vibememory_cli::mcp_config::Client::ClaudeCode)
        .map(|client| client.name())
        .collect();
    println!(
        "agents    memory for other agents: vibememory mcp-config <{}>",
        others.join("|")
    );
}

/// A machine key answer: the key kept and the team's store cloned, or why not — with the key's id
/// to revoke, since the cabinet registered it either way.
fn connect_key(
    layout: &vibememory_cli::install::Layout,
    grant: &vibememory_core::claim::KeyGrant,
    pending: vibememory_cli::team_connect::PendingKey,
    machine_id: Option<&str>,
) -> ExitCode {
    // Sessions travel with the engine, and a member's machine may have no store of its own to have set it up:
    // The engine is set up here, as `connect` does for the personal store, before the key is kept
    if !engine_configured(layout) {
        if let Err(error) = configure_engine(layout, machine_id) {
            pending.discard();
            eprintln!("connect: {error}");
            eprintln!(
                "connect: key {} is registered for this machine on the host: revoke it in the cabinet, or with \
                 `vibememory-mcp admin key revoke` on a host without one",
                grant.key_id
            );
            return ExitCode::FAILURE;
        }
        if install(false) != ExitCode::SUCCESS {
            eprintln!(
                "connect: the engine is configured but not fully installed: vibememory doctor says what is left"
            );
        }
    }
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
            print_agent_hints(layout);
            println!(
                "next:      vibememory route add <dir> --to {} for each project of the team, or move one with \
                 vibememory project move <dir> --to {}",
                grant.team, grant.team
            );
            ExitCode::SUCCESS
        }
        Err(refusal) => {
            eprintln!("connect: {refusal}");
            eprintln!(
                "connect: key {} is registered for this machine on the host: revoke it in the cabinet, or with \
                 `vibememory-mcp admin key revoke` on a host without one",
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
    let layout = layout();
    // with the engine here, the one local server reaches the team through the directories routed
    // to it; a server of the team's own would only make the agent guess between two memories
    if read_config(&layout).is_ok() {
        println!(
            "route     vibememory route add <dir> --to {}: the local memory server {} reaches the team in that \
             directory",
            grant.team,
            vibememory_cli::registrations::LOCAL_SERVER
        );
        print_agent_hints(&layout);
        return;
    }
    println!("register with Claude Code:");
    println!(
        "  {}",
        vibememory_cli::connect::claude_code_registration(
            grant,
            &vibememory_cli::install::installed_binary(&layout)
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
    println!(
        "route    vibememory route list, then vibememory route remove <dir> for each directory of {team}: they \
         are yours again"
    );
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
fn connect_arguments(args: &[String]) -> Result<ConnectArguments, String> {
    let mut cabinet = None;
    let mut agent = None;
    let mut machine_id = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let slot = match arg.as_str() {
            "--cabinet" => &mut cabinet,
            "--agent" => &mut agent,
            "--machine-id" => &mut machine_id,
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
    Ok(ConnectArguments {
        cabinet,
        agent,
        machine_id,
    })
}

/// What `connect` was given.
struct ConnectArguments {
    cabinet: String,
    agent: Option<String>,
    /// The name a machine joining the personal store takes when it has no `config.json` yet.
    machine_id: Option<String>,
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
            "engine not configured (no {}): placing the binaries only — to make this a machine of your personal \
             store, get a code for it on the personal store's page in the cabinet and run vibememory connect \
             --cabinet <address>",
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
    if applied
        .performed
        .iter()
        .any(|what| *what == vibememory_cli::install::Step::OnPath.describe())
    {
        println!("open a new terminal: vibememory answers there by name");
    }
    print_waiting_agents(&layout);
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
    let portable = config.portable_cwd(&cwd);
    let stamp = vibememory_cli::clock::now();
    // the route goes with the move: a project in a team's store its directory is not routed to
    // would send its next session back to the personal store
    let path = layout.engine_dir.join("config.json");
    let Ok(before) = std::fs::read_to_string(&path) else {
        eprintln!("project move: {} cannot be read", path.display());
        return ExitCode::FAILURE;
    };
    let rerouted = if to == "personal" {
        vibememory_cli::route::remove(&before, &cwd)
    } else if vibememory_cli::team_connect::connected_teams(&layout).contains(to) {
        vibememory_cli::route::add(&before, &cwd, to)
    } else {
        Err(format!("team {to} is not connected on this machine"))
    };
    let config = match rerouted {
        Ok(vibememory_cli::route::Changed::Text { text, patterns }) => {
            if let Err(error) = vibememory_cli::route::write_config(&path, &text) {
                eprintln!("project move: {error}");
                return ExitCode::FAILURE;
            }
            for pattern in patterns {
                println!(
                    "route    {pattern}: {}",
                    if to == "personal" { "removed" } else { "added" }
                );
            }
            match Config::parse(&text, PathSyntax::Posix) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("project move: {error}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Ok(vibememory_cli::route::Changed::Already) => config,
        Err(error) => {
            eprintln!("project move: {error}");
            return ExitCode::FAILURE;
        }
    };
    let moved = if to == "personal" {
        vibememory_cli::project_move::to_personal(&layout, &config, &cwd, &portable)
    } else {
        vibememory_cli::project_move::to_team(&layout, &config, &cwd, &portable, to, &stamp)
    };
    if moved.is_err() {
        // nothing moved, so the routes stay as they were
        let _ = vibememory_cli::route::write_config(&path, &before);
    }
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

/// `route add <dir> --to <team>`, `route remove <dir>`, `route list`, `route which [dir]`: the one
/// rule of which store a directory's sessions and memory belong to.
fn route_command(args: &[String]) -> ExitCode {
    const USAGE: &str = "usage: vibememory route add <dir> --to <team>\n       vibememory route remove <dir>\n       \
                         vibememory route list\n       vibememory route which [dir]";
    let layout = layout();
    let path = layout.engine_dir.join("config.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("route: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let config = match Config::parse(&text, PathSyntax::Posix) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("config: {error}");
            return ExitCode::from(2);
        }
    };
    let changed = match args {
        [verb, dir, flag, team] if verb == "add" && flag == "--to" => {
            if !vibememory_cli::team_connect::connected_teams(&layout).contains(team)
                && !vibememory_cli::stores::memory_only(&layout, team)
            {
                eprintln!(
                    "route: team {} is not connected on this machine: vibememory connect --cabinet <address> first",
                    vibememory_core::terminal::printable(team)
                );
                return ExitCode::FAILURE;
            }
            vibememory_cli::route::add(&text, dir, team)
        }
        [verb, dir] if verb == "remove" => vibememory_cli::route::remove(&text, dir),
        [verb] if verb == "list" => return route_list(&config, &layout),
        [verb, rest @ ..] if verb == "which" && rest.len() <= 1 => {
            return route_which(&config, rest.first().cloned());
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match changed {
        Ok(vibememory_cli::route::Changed::Text { text, patterns }) => {
            if let Err(error) = vibememory_cli::route::write_config(&path, &text) {
                eprintln!("route: {error}");
                return ExitCode::FAILURE;
            }
            let verb = if args.first().map(String::as_str) == Some("add") {
                "added"
            } else {
                "removed"
            };
            for pattern in patterns {
                println!("route    {pattern}: {verb}");
            }
            if let [verb, _, _, team] = args
                && verb == "add"
                && let Err(error) = keep_sessions_before_route(&layout, &text, team)
            {
                eprintln!("route: {error}");
                return ExitCode::FAILURE;
            }
            println!(
                "new sessions follow the route; a project whose sessions are already in another store moves with \
                 vibememory project move"
            );
            ExitCode::SUCCESS
        }
        Ok(vibememory_cli::route::Changed::Already) => {
            println!("the route is already so");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("route: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The sessions a project already has here stay on this machine when it is routed to a team: set aside in the
/// team's clone now, at the moment of the route, so that every session started after it goes to the team
/// A memory-only team has no clone and no sessions to set aside
fn keep_sessions_before_route(layout: &Layout, text: &str, team: &str) -> Result<(), String> {
    if !vibememory_cli::team_connect::connected_teams(layout).contains(&team.to_owned()) {
        return Ok(());
    }
    let routed = Config::parse(text, PathSyntax::Posix).map_err(|error| error.to_string())?;
    let clone = layout.team_store(team);
    let before = vibememory_cli::tick::sessions_before_route(
        &layout.config_dir,
        &clone,
        &routed.naming,
        &routed.routes,
        team,
    );
    vibememory_cli::local_only::keep(&clone, &before)?;
    if !before.is_empty() {
        println!(
            "kept     {} file(s) of sessions from before the route: they stay on this machine, and the team \
             gets the sessions started from now on",
            before.len()
        );
    }
    Ok(())
}

/// `route list`: every route, then what is wrong with them.
fn route_list(config: &Config, layout: &Layout) -> ExitCode {
    let mut any = false;
    for (pattern, team, _) in config.routes.patterns() {
        any = true;
        println!("{team}\t{pattern}");
    }
    if !any {
        println!("no routes: every directory belongs to the personal store");
    }
    for line in vibememory_cli::route::warnings(config, &layout.store()) {
        println!("route    {}", vibememory_core::terminal::printable(&line));
    }
    ExitCode::SUCCESS
}

/// `route which [dir]`: the store a session opened in the directory goes to.
fn route_which(config: &Config, dir: Option<String>) -> ExitCode {
    let dir = dir
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|dir| dir.display().to_string())
        })
        .unwrap_or_default();
    let real =
        std::fs::canonicalize(&dir).map_or_else(|_| dir.clone(), |path| path.display().to_string());
    let cwd = canonical_cwd(&real, session_start::host_syntax());
    match config.routes.route(&cwd, session_start::host_syntax()) {
        Ok(Some(team)) => {
            println!("{cwd}: team {team}");
            ExitCode::SUCCESS
        }
        Ok(None) => {
            println!("{cwd}: personal store");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("route: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `session share <sid>`: gives the team a session from before its project went to the team. The
/// session stays where it is; it only leaves the list of what this machine keeps to itself, and the
/// next tick commits it into the team.
fn session_command(args: &[String]) -> ExitCode {
    const USAGE: &str = "usage: vibememory session share <session-id>\n       \
                         vibememory session release <machine> <session-id> --confirm\n       \
                         vibememory session put --agent <name> --id <session-id> --cwd <dir> \
                         --from <file.jsonl> [--end]\n       \
                         vibememory session agent add --agent <name> --dir <log-dir> --run \
                         <wrapper> [--backfill]\n       \
                         vibememory session agent add --agent <name> --preset dsh|codex [--dir <log-dir>] \
                         [--backfill]\n       \
                         vibememory session agent remove --agent <name>\n       \
                         vibememory session agent decline --agent <name>\n       \
                         vibememory session agent list [--json]";
    if args.first().is_some_and(|verb| verb == "put") {
        return session_put(args.get(1..).unwrap_or_default(), USAGE);
    }
    if args.first().is_some_and(|verb| verb == "agent") {
        return session_agent(args.get(1..).unwrap_or_default(), USAGE);
    }
    if let [verb, machine, session, confirm] = args
        && verb == "release"
    {
        if confirm != "--confirm" {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        return session_release(machine, session);
    }
    let [verb, session] = args else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if verb == "release" {
        eprintln!(
            "session release takes the machine, the session and --confirm: it clears a claim of \
             another machine, and only its owner may say that machine is gone"
        );
        return ExitCode::from(2);
    }
    if verb != "share" {
        eprintln!("{USAGE}");
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

/// `session put`: a session of another agent, as a file in the format of
/// `docs/manuals/foreignSessionSpec.md`, goes into the store of its working directory.
fn session_put(args: &[String], usage: &str) -> ExitCode {
    let mut agent = None;
    let mut session = None;
    let mut cwd = None;
    let mut source = None;
    let mut ended = false;
    let mut rest = args.iter();
    while let Some(flag) = rest.next() {
        let slot = match flag.as_str() {
            "--end" => {
                ended = true;
                continue;
            }
            "--agent" => &mut agent,
            "--id" => &mut session,
            "--cwd" => &mut cwd,
            "--from" => &mut source,
            _ => {
                eprintln!("{usage}");
                return ExitCode::from(2);
            }
        };
        let Some(value) = rest.next() else {
            eprintln!("{flag} takes a value\n{usage}");
            return ExitCode::from(2);
        };
        *slot = Some(value.clone());
    }
    let (Some(agent), Some(session), Some(cwd), Some(source)) = (agent, session, cwd, source)
    else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    let layout = layout();
    let config = match read_config(&layout) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("session put: the engine is not configured here: {error}");
            return ExitCode::FAILURE;
        }
    };
    let bytes = match std::fs::read(&source) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("session put: {source}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let handed = vibememory_cli::foreign_session::Handed {
        agent: &agent,
        session: &session,
        cwd: &cwd,
        origin: &source,
        bytes: &bytes,
        ended,
    };
    let placed = match vibememory_cli::foreign_session::hand_over(&layout, &config, &handed) {
        Ok(placed) => placed,
        Err(error) => {
            eprintln!("session put: {error}");
            return ExitCode::FAILURE;
        }
    };
    let store = placed
        .store
        .team
        .as_deref()
        .map_or_else(|| "personal".to_owned(), |team| format!("team {team}"));
    let outcome = if !placed.held.is_empty() {
        format!(
            "held on this machine: it holds agent token(s) {}; revoke them in the cabinet",
            placed.held.join(", ")
        )
    } else if placed.committed {
        "committed".to_owned()
    } else {
        "unchanged since the last put".to_owned()
    };
    println!(
        "put: {} record(s), {} searchable, into project {} of the {store} store ({}) — {outcome}",
        placed.checked.lines, placed.checked.spoken, placed.project, placed.relative
    );
    ExitCode::SUCCESS
}

/// `session agent add|remove`: an agent without hooks of its own, watched by the tick — each log
/// written into its directory goes to its wrapper, which hands the session over with `session put`,
/// or, for an agent the engine knows by a preset, is read by the engine itself.
fn session_agent(args: &[String], usage: &str) -> ExitCode {
    let Some((verb, rest)) = args.split_first() else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    // Before the flags are read: `list` takes `--json` and nothing else, and the loop below would
    // refuse it as an unknown flag of `add`.
    if verb == "list" {
        let json = match rest {
            [] => false,
            [only] if only == "--json" => true,
            _ => {
                eprintln!("{usage}");
                return ExitCode::from(2);
            }
        };
        return session_agent_list(&layout(), json);
    }
    let mut agent = None;
    let mut dir = None;
    let mut run = None;
    let mut preset = None;
    let mut backfill = false;
    let mut flags = rest.iter();
    while let Some(flag) = flags.next() {
        let slot = match flag.as_str() {
            "--backfill" => {
                backfill = true;
                continue;
            }
            "--agent" => &mut agent,
            "--dir" => &mut dir,
            "--run" => &mut run,
            "--preset" => &mut preset,
            _ => {
                eprintln!("{usage}");
                return ExitCode::from(2);
            }
        };
        let Some(value) = flags.next() else {
            eprintln!("{flag} takes a value\n{usage}");
            return ExitCode::from(2);
        };
        *slot = Some(value.clone());
    }
    let layout = layout();
    let flags = AgentFlags {
        agent,
        dir,
        run,
        preset,
        backfill,
    };
    if verb == "add" {
        return session_agent_add(&layout, &flags, usage);
    }
    if verb == "decline" {
        return session_agent_decline(&layout, &flags, usage);
    }
    let AgentFlags {
        agent: Some(agent),
        dir: None,
        run: None,
        preset: None,
        backfill: false,
    } = flags
    else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    if verb != "remove" {
        eprintln!("{usage}");
        return ExitCode::from(2);
    }
    match vibememory_cli::agents::unregister(&layout, &agent) {
        Ok(true) => {
            println!(
                "{} is no longer watched; its sessions in the store stay, and the engine will not \
                 offer it again — `session agent add` takes that back",
                vibememory_core::terminal::printable(&agent)
            );
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!(
                "session agent remove: {} is not watched here",
                vibememory_core::terminal::printable(&agent)
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("session agent remove: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `session agent decline`: the answer "their sessions stay on this machine, stop offering". The
/// logs are not touched, nothing is registered, and `add` is the way back.
fn session_agent_decline(layout: &Layout, flags: &AgentFlags, usage: &str) -> ExitCode {
    let AgentFlags {
        agent: Some(agent),
        dir: None,
        run: None,
        preset: None,
        backfill: false,
    } = flags
    else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    // Declining an agent the engine cannot read would be a decision about nothing: that agent's
    // logs are nobody's to read, and the note would only silence a line nobody would print.
    if vibememory_cli::agents::preset_for(agent).is_none() {
        eprintln!(
            "session agent decline: {} is no agent the engine reads logs of; the engine knows {}",
            vibememory_core::terminal::printable(agent),
            vibememory_cli::agents::Preset::ALL
                .iter()
                .map(|preset| preset.agent())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return ExitCode::from(2);
    }
    // Watched and declined at once would be two answers to one question, and `doctor` would print
    // both: a removal is the way, and it writes the same decision.
    if vibememory_cli::agents::registry(layout).contains_key(agent) {
        eprintln!(
            "session agent decline: {} is watched here; `vibememory session agent remove --agent \
             {}` stops that and remembers the decision",
            vibememory_core::terminal::printable(agent),
            vibememory_core::terminal::printable(agent)
        );
        return ExitCode::FAILURE;
    }
    match vibememory_cli::agents::decline(layout, agent) {
        Ok(()) => {
            println!(
                "{}: its sessions stay on this machine; the engine will not offer to watch it again",
                vibememory_core::terminal::printable(agent)
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("session agent decline: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `session agent list`: every agent this machine knows, and whether its sessions arrive — the one
/// question the other lines of the report answer in three different places.
fn session_agent_list(layout: &Layout, json: bool) -> ExitCode {
    let sessions = foreign_sessions(layout);
    let delivered: std::collections::BTreeMap<&str, (usize, &str)> = sessions
        .iter()
        .map(|(_, agent, count, newest)| (agent.as_str(), (*count, newest.as_str())))
        .collect();
    let watched = vibememory_cli::agents::watched(layout);
    let waiting = vibememory_cli::agents::waiting(layout);
    let declined = vibememory_cli::agents::declined(layout);
    let readings = vibememory_cli::agents::Readings::with_delivered(
        layout,
        sessions
            .iter()
            .map(|(_, agent, _, _)| agent.clone())
            .collect(),
    );
    let clients = vibememory_cli::credentials::started_clients(&layout.engine_dir);
    if json {
        return print_agents_json(layout, &watched, &clients, &sessions, &readings);
    }
    let mut rows = 0;
    for watched in &watched {
        let agent = vibememory_core::terminal::printable(&watched.agent);
        let by = watched.registration.preset.map_or_else(
            || {
                format!(
                    "wrapper {}",
                    vibememory_core::terminal::printable(
                        watched.registration.run.as_deref().unwrap_or_default()
                    )
                )
            },
            |preset| format!("{} preset", preset.name()),
        );
        let state = match delivered.get(watched.agent.as_str()) {
            Some((count, newest)) => format!("{count} session(s) in the store, newest {newest}"),
            None => "no session in the store yet".to_owned(),
        };
        let trouble = if watched.missing {
            ", its log directory is gone"
        } else if watched.silent {
            ", logs go on and sessions do not"
        } else if watched.failure.is_some() {
            ", its last run failed"
        } else {
            ""
        };
        println!(
            "watched  {agent} \u{2014} {by}, logs in {}{trouble}; {state}",
            vibememory_core::terminal::printable(&watched.registration.dir)
        );
        rows += 1;
    }
    for waiting in &waiting {
        println!("{}", waiting.line());
        rows += 1;
    }
    for declined in &declined {
        println!("{}", declined.line());
        rows += 1;
    }
    // A client that only writes memory is an agent too: an answer about the machine that left it
    // out would be an answer about some agents, not about this machine.
    for (agent, start) in &clients {
        if readings.note(agent).is_none() {
            continue;
        }
        println!(
            "client   {} \u{2014} a memory server started as this agent {}; {note}",
            vibememory_core::terminal::printable(agent),
            describe_start(start),
            note = readings.note(agent).unwrap_or_default()
        );
        rows += 1;
    }
    if rows == 0 {
        println!(
            "no agent is watched here and no known agent keeps logs on this machine: \
             vibememory session put --agent <name> takes a session from any agent"
        );
    }
    ExitCode::SUCCESS
}

/// The same list as `--json`: one document, so that a monitor reads the machine once.
fn print_agents_json(
    layout: &Layout,
    watched: &[vibememory_cli::agents::Watched],
    clients: &[(String, vibememory_cli::credentials::ClientStart)],
    sessions: &[(String, String, usize, String)],
    readings: &vibememory_cli::agents::Readings,
) -> ExitCode {
    let report = serde_json::json!({
        "watched": watched_json(watched),
        "waiting": waiting_json(layout),
        "unwatched": unwatched_json(layout),
        // A client the engine cannot read hands over memory alone; the flag is what a monitor
        // watches for, because that state is otherwise only a sentence in a report.
        "clients": clients
            .iter()
            .map(|(agent, start)| serde_json::json!({
                "agent": agent,
                "started": start.stamp,
                "version": start.version,
                "handsSessionsOver": readings.note(agent).is_none(),
            }))
            .collect::<Vec<_>>(),
        "sessions": sessions
            .iter()
            .map(|(store, agent, count, newest)| serde_json::json!({
                "store": store,
                "agent": agent,
                "sessions": count,
                "newest": newest,
            }))
            .collect::<Vec<_>>(),
    });
    println!("{report:#}");
    ExitCode::SUCCESS
}

/// The flags of `session agent`, as typed.
struct AgentFlags {
    agent: Option<String>,
    dir: Option<String>,
    run: Option<String>,
    preset: Option<String>,
    backfill: bool,
}

/// `session agent add`: a wrapper or a preset, never both; a preset knows where its agent keeps
/// its logs, so the directory may go unsaid.
fn session_agent_add(layout: &Layout, flags: &AgentFlags, usage: &str) -> ExitCode {
    let AgentFlags {
        agent,
        dir,
        run,
        preset,
        backfill,
    } = flags;
    let backfill = *backfill;
    let Some(agent) = agent else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    let preset = match preset.as_deref().map(|name| (name, Preset::of(name))) {
        None => None,
        Some((_, Some(preset))) => Some(preset),
        Some((name, None)) => {
            eprintln!(
                "session agent add: no preset {}; the engine knows {}",
                vibememory_core::terminal::printable(name),
                Preset::ALL
                    .iter()
                    .map(|preset| preset.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return ExitCode::from(2);
        }
    };
    let dir = match (dir, preset) {
        (Some(dir), _) => PathBuf::from(dir),
        (None, Some(preset)) => {
            let Some(home) = vibememory_cli::install::home_dir() else {
                eprintln!("session agent add: no home directory to find the logs in; give --dir");
                return ExitCode::FAILURE;
            };
            preset.default_dir(&home)
        }
        (None, None) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let handler = match (&run, preset) {
        (Some(run), None) => Handler::Run(std::path::Path::new(run)),
        (None, Some(preset)) => Handler::Preset(preset),
        (Some(_), Some(_)) => {
            eprintln!(
                "session agent add: --run and --preset exclude each other: with a preset the \
                 engine reads the log itself"
            );
            return ExitCode::from(2);
        }
        (None, None) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    match vibememory_cli::agents::register(layout, agent, &dir, handler, backfill) {
        Ok(registration) => {
            let by = registration.preset.map_or_else(
                || {
                    format!(
                        "goes to {}",
                        registration.run.as_deref().unwrap_or_default()
                    )
                },
                |preset| format!("is read by the engine ({} preset)", preset.name()),
            );
            let from = if backfill {
                "every log there, the past ones too, a few per tick,"
            } else {
                "each log written from now on"
            };
            println!(
                "watching {agent}: {from} in {} {by}; the tick looks every two minutes, and \
                 doctor says when sessions stop coming",
                registration.dir
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("session agent add: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `session release <machine> <sid> --confirm`: clears another machine's claim that a session is
/// live, in the personal store and in every connected team's.
fn session_release(machine: &str, session: &str) -> ExitCode {
    let layout = layout();
    let mut stores = vec![layout.store()];
    stores.extend(
        vibememory_cli::team_connect::connected_teams(&layout)
            .iter()
            .map(|team| layout.team_store(team)),
    );
    let mut released = false;
    for store in stores {
        match vibememory_cli::relink::release(&store, machine, session) {
            Ok(true) => {
                released = true;
                println!(
                    "released: {} of {} in {}; the next tick sends it",
                    vibememory_core::terminal::printable(session),
                    vibememory_core::terminal::printable(machine),
                    store.display()
                );
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("session release: {}: {error}", store.display());
                return ExitCode::FAILURE;
            }
        }
    }
    if released {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "session release: no store holds a claim of {} for {}",
            vibememory_core::terminal::printable(machine),
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
            &config.portable_cwd(&cwd),
            &vibememory_cli::clock::now(),
            vibememory_cli::process::agent_process,
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
                cwd: &config.portable_cwd(&cwd),
                syntax,
                source: vibememory_core::links::LinkSource::Observed,
                confirmed_by: Some(&input.transcript_path),
            },
        );
    }
    // Anything a merge in the tick needed a person to know has been waiting for a session to
    // exist; this is that session.
    let mut notes = session_notes(&layout, &enc);
    if let Some(message) = decision.additional_context() {
        notes.push(message);
    }
    // the project's own rule files against the rules in force: once a day, in `advise` mode
    if let Some(name) = decision.store_name() {
        let today: String = vibememory_cli::clock::now().chars().take(10).collect();
        if let Some(note) = vibememory_cli::rules_sync::notice(
            &layout.engine_dir,
            &layout.store(),
            &store.clone,
            store.team.is_some(),
            name,
            std::path::Path::new(&input.cwd),
            &today,
        ) {
            notes.push(note);
        }
    }
    // a team's rules that came with a pull are told here, and go out with the engine's next run
    if let Some(team) = store.team.as_deref()
        && let Some(note) = vibememory_cli::rules_shown::notice(&store.clone, team)
    {
        notes.push(note);
    }
    if notes.is_empty() {
        ExitCode::SUCCESS
    } else {
        say(&notes.join(" "))
    }
}

/// What this machine owes a session at its start, besides the link: what a merge left for a person
/// to know, the hand-offs open in this directory, what waits in quarantine, an agent nobody watches
/// — whose sessions go nowhere until somebody says otherwise — and a newer release.
fn session_notes(layout: &Layout, enc: &EncSlug) -> Vec<String> {
    let mut notes: Vec<String> = vibememory_cli::merge_report::read_pending(&layout.engine_dir);
    if !notes.is_empty() {
        let _ = vibememory_cli::merge_report::clear_pending(&layout.engine_dir);
    }
    if let Some(note) = handoff_note(layout, enc) {
        notes.push(note);
    }
    for note in quarantine_notes(&layout.engine_dir) {
        notes.push(note);
    }
    // The person is in this session and does not read the machine's report; the sentence is said
    // once a day and again when the logs grow, and the library decides which.
    if let Some(note) =
        vibememory_cli::agents::notice_waiting(layout, vibememory_cli::agents::now())
    {
        notes.push(note);
    }
    if let Some(newer) = vibememory_cli::update::newer_known(layout) {
        notes.push(format!(
            "VibeMemory {newer} is out (this machine runs {}): `vibememory update` installs it.",
            vibememory_cli::update::CURRENT
        ));
    }
    notes
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

/// Closes a session in the live list `SessionStart` put it on: the store its working directory is routed to.
///
/// A session that ended outside every clone — in a real directory the tick has yet to import, or one that never
/// wrote — has nothing to commit, but it is still on that list
/// Left there, it counts as running until its heartbeat goes stale, an hour, and the tick does not import a
/// directory with a running session: the project's sessions would wait that hour to reach the store
fn end_where_started(layout: &Layout, config: &Config, input: &vibememory_cli::hook::HookInput) {
    let syntax = session_start::host_syntax();
    let cwd = canonical_cwd(&input.cwd, syntax);
    if let Ok(store) = vibememory_cli::stores::for_cwd(layout, config, &cwd, syntax)
        && let Ok(clone) = std::fs::canonicalize(&store.clone)
    {
        let _ = record_end(&clone, &store.machine_id, &input.session_id);
    }
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
    // An empty path is not a session that has written nothing yet: it is a caller that did not say
    // where the session is, and staying silent about it means its sessions never arrive.
    if input.transcript_path.trim().is_empty() {
        eprintln!(
            "vibememory: hook with an empty transcript_path; nothing to commit. An agent other \
             than Claude Code hands its sessions over with `vibememory session put`"
        );
        return ExitCode::SUCCESS;
    }
    let transcript = std::path::PathBuf::from(&input.transcript_path);
    let Ok(real) = std::fs::canonicalize(&transcript) else {
        if ended {
            end_where_started(&layout, &config, &input);
        }
        return ExitCode::SUCCESS;
    };
    let Some((owner, relative)) = vibememory_cli::stores::for_file(&layout, &config, &real) else {
        // A session in a real directory: the tick imports it, and saying so on every stop would
        // be noise, since SessionStart already said it once.
        if ended {
            end_where_started(&layout, &config, &input);
        }
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
    let stopped = match commit_snapshot(
        &store,
        &real,
        &relative,
        &stamp,
        &kept,
        vibememory_core::foreign::CLAUDE_CODE,
    ) {
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
    let cwd = config.portable_cwd(&input.cwd);
    if let Err(error) = record_progress(
        &store,
        &owner.machine_id,
        &input.session_id,
        &cwd,
        &real,
        &stamp,
        vibememory_cli::process::agent_process,
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
/// `mcp-config [<client>]`: how to connect the memory server to a client on this machine, printed
/// for a person to put into that client's own configuration.
fn mcp_config_command(client: Option<&str>) -> ExitCode {
    let Some(name) = client else {
        print!("{}", vibememory_cli::mcp_config::overview());
        return ExitCode::SUCCESS;
    };
    let Some(client) = vibememory_cli::mcp_config::Client::named(name) else {
        eprint!(
            "mcp-config: no client {:?}\n{}",
            vibememory_core::terminal::printable(name),
            vibememory_cli::mcp_config::overview()
        );
        return ExitCode::from(2);
    };
    print!(
        "{}",
        vibememory_cli::mcp_config::instructions(
            client,
            &vibememory_cli::mcp_config::Machine::of(&layout())
        )
    );
    ExitCode::SUCCESS
}

/// `update`: the newest release from the host, installed over this one by its own `install`.
fn update_command() -> ExitCode {
    let layout = layout();
    let base = vibememory_cli::update::releases_base(&layout);
    let code = match vibememory_cli::update::update(&layout, &base) {
        Ok(vibememory_cli::update::Updated::Current(version)) => {
            println!("vibememory {version} is the newest release");
            ExitCode::SUCCESS
        }
        Ok(vibememory_cli::update::Updated::Installed(version)) => {
            println!(
                "updated: vibememory {} \u{2192} {version}",
                vibememory_cli::update::CURRENT
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("update: {error}");
            ExitCode::FAILURE
        }
    };
    // The moment after an update is a moment a person reads the output, and a new engine version
    // is exactly when an agent nobody registered appears: `install` and `update` say it here, not
    // only in a `doctor` somebody has to think of running.
    print_waiting_agents(&layout);
    code
}

/// One line per agent the engine knows how to read and nobody registered, in the words of `doctor`:
/// the same state must be recognized wherever a person meets it.
fn print_waiting_agents(layout: &Layout) {
    if !engine_configured(layout) {
        return;
    }
    for waiting in vibememory_cli::agents::waiting(layout) {
        println!("{}", waiting.line());
    }
    // The other answer to the same question: a person decided these sessions stay here. Said
    // without a fault, because a decision is not one — but said, so the state has a name.
    for declined in vibememory_cli::agents::declined(layout) {
        println!("{}", declined.line());
    }
}

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
    let roots = config.roots();
    let desktop = desktop_store_path(&config);
    let machine = vibememory_cli::tick::Machine {
        store: &store,
        config_dir: &layout.config_dir,
        home: layout.home.as_deref(),
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
    // Agents without hooks of their own: each log written since the last run goes to its wrapper.
    // A wrapper that fails is the agent's fault, not the sync's: it is written down for `doctor`,
    // and the tick itself does not fail over it.
    for ran in vibememory_cli::agents::tick(&layout, &config) {
        report_agent_run(&ran);
    }
    // Once a day, ask whether the backup still follows the host. Nobody runs `doctor` on a
    // schedule, so without this a mirror could stop following the day after it was set up and
    // nothing would ever say so.
    if let Some(state) = mirror_watch(&layout, &config) {
        println!("mirror: {state}");
    }
    // Once a day, whether a newer release is out: the tick only asks and remembers, `SessionStart`
    // and `doctor` say it, and installing stays a person's `vibememory update`.
    let release_dir = vibememory_cli::update::releases_base(&layout);
    if let Some(newer) =
        vibememory_cli::update::check_if_due(&layout, &release_dir, epoch_seconds())
    {
        println!("update: vibememory {newer} is out \u{2014} vibememory update");
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// One agent's part of the tick, said only when there is something to say.
fn report_agent_run(ran: &vibememory_cli::agents::Ran) {
    let agent = vibememory_core::terminal::printable(&ran.agent);
    if ran.missing {
        println!("agent {agent}: its log directory is not there");
        return;
    }
    if ran.handed > 0 {
        println!("agent {agent}: {} log(s) handed to the wrapper", ran.handed);
    }
    if let Some(failure) = &ran.failure {
        println!(
            "agent {agent}: the wrapper failed on {} \u{2014} {}",
            vibememory_core::terminal::printable(&failure.log),
            failure.outcome
        );
        for line in &failure.lines {
            println!("  {line}");
        }
    }
    for skipped in &ran.skipped {
        println!(
            "agent {agent}: set aside {} \u{2014} {}; tried again when it is written to",
            vibememory_core::terminal::printable(&skipped.log),
            skipped.outcome
        );
    }
    if ran.waiting > 0 {
        println!(
            "agent {agent}: {} log(s) wait for the next run",
            ran.waiting
        );
    }
}

/// The agents the tick watches here, and what is wrong with each: the wrapper failing, or logs
/// written while no session comes. Answers how many are wrong.
fn print_watched_agents(layout: &Layout) -> usize {
    let mut wrong = 0;
    if let Some(problem) = vibememory_cli::agents::registry_problem(layout) {
        wrong += 1;
        println!(
            "watch    the agents file cannot be read, so no agent is watched: {}",
            vibememory_core::terminal::printable(&problem)
        );
    }
    let at =
        |seconds: u64| vibememory_cli::clock::iso8601(i64::try_from(seconds).unwrap_or(i64::MAX));
    let watched_agents = vibememory_cli::agents::watched(layout);
    // An agent the engine knows how to read, whose logs are on this machine, and which nobody
    // registered: not a fault — handing sessions over is a decision — but the decision is written
    // in the registry, and until it is there the sessions lie in the logs and nowhere else. The
    // count is what makes it visible: "no sessions" and "twenty-two sessions nobody will ever see"
    // look the same from every other line of this report.
    for waiting in vibememory_cli::agents::waiting(layout) {
        println!("{}", waiting.line());
    }
    for watched in watched_agents {
        let agent = vibememory_core::terminal::printable(&watched.agent);
        let put = watched.last_put.map_or_else(
            || "no session put yet".to_owned(),
            |put| format!("last put {}", at(put)),
        );
        let by = watched.registration.preset.map_or_else(
            || {
                format!(
                    "wrapper {}",
                    vibememory_core::terminal::printable(
                        watched.registration.run.as_deref().unwrap_or_default()
                    )
                )
            },
            |preset| format!("{} preset", preset.name()),
        );
        println!(
            "watch    {agent} \u{2014} logs in {}, {by}; {put}",
            vibememory_core::terminal::printable(&watched.registration.dir)
        );
        if watched.missing {
            wrong += 1;
            println!("         the log directory is not there: nothing of this agent is delivered");
        }
        if let Some(failure) = &watched.failure {
            wrong += 1;
            println!(
                "         the wrapper failed at {} on {} \u{2014} {}",
                at(failure.at),
                vibememory_core::terminal::printable(&failure.log),
                failure.outcome
            );
            for line in &failure.lines {
                println!("           {line}");
            }
        }
        // Set aside, not failing: the agent's other sessions go on, and a session whose project
        // directory is gone is no fault of the integration — but it is a session that does not
        // reach the history, and that is said.
        for skipped in &watched.skipped {
            println!(
                "         not delivered: {} \u{2014} {}",
                vibememory_core::terminal::printable(&skipped.log),
                skipped.outcome
            );
        }
        if watched.silent {
            wrong += 1;
            println!(
                "         silent: logs written at {} and no session put since; the wrapper runs \
                 but does not deliver",
                watched.newest_log.map_or_else(String::new, at)
            );
        }
    }
    wrong
}

/// The agents whose logs are here and which nobody registered, as `--json` gives them.
fn waiting_json(layout: &Layout) -> Vec<serde_json::Value> {
    vibememory_cli::agents::waiting(layout)
        .into_iter()
        .map(|waiting| {
            serde_json::json!({
                "agent": waiting.agent,
                "preset": waiting.preset.name(),
                "dir": waiting.dir,
                "logs": waiting.logs,
                "newest": waiting.newest.map(|seconds| {
                    vibememory_cli::clock::iso8601(i64::try_from(seconds).unwrap_or(i64::MAX))
                }),
            })
        })
        .collect()
}

/// The agents a person decided not to watch, as `--json` gives them.
fn unwatched_json(layout: &Layout) -> Vec<serde_json::Value> {
    vibememory_cli::agents::declined(layout)
        .into_iter()
        .map(|declined| {
            serde_json::json!({
                "agent": declined.agent,
                "preset": declined.preset.name(),
                "dir": declined.dir,
                "logs": declined.logs,
                "since": vibememory_cli::clock::iso8601(
                    i64::try_from(declined.at).unwrap_or(i64::MAX)
                ),
            })
        })
        .collect()
}

/// The watched agents as `--json` gives them.
fn watched_json(watched: &[vibememory_cli::agents::Watched]) -> Vec<serde_json::Value> {
    watched
        .iter()
        .map(|watched| {
            serde_json::json!({
                "agent": watched.agent,
                "dir": watched.registration.dir,
                "run": watched.registration.run,
                "preset": watched.registration.preset.map(Preset::name),
                "skipped": watched.skipped.iter().map(|skipped| serde_json::json!({
                    "log": skipped.log,
                    "outcome": skipped.outcome,
                })).collect::<Vec<_>>(),
                "since": watched.registration.since,
                "lastPut": watched.last_put,
                "lastLook": watched.last_look,
                "newestLog": watched.newest_log,
                "failure": watched.failure.as_ref().map(|failure| serde_json::json!({
                    "at": failure.at,
                    "log": failure.log,
                    "outcome": failure.outcome,
                    "lines": failure.lines,
                })),
                "silent": watched.silent,
                "missing": watched.missing,
            })
        })
        .collect()
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
        home: layout.home.as_deref(),
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
    let portable = config.portable_cwd(&canonical_cwd(cwd, PathSyntax::Posix));

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
    let rules = &ticked.rules;
    if !rules.written.is_empty() || !rules.taken.is_empty() {
        println!(
            "rules and skills: {} agent file(s) written, {} edit(s) taken from agents' files ({})",
            rules.written.len(),
            rules.taken.len(),
            rules.taken.join(", ")
        );
    }
    for id in &rules.conflicts {
        println!(
            "rules: {id} was changed in an agent's file and in the store both — the agent's version is in the quarantine"
        );
    }
    for problem in &rules.problems {
        println!("rules: {problem}");
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
    layout: &Layout,
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
    let teams = vibememory_cli::team_connect::team_facts(layout);
    let teams_wrong = teams
        .iter()
        .any(|facts| facts.pause.is_some() || !facts.problems.is_empty());
    let watched = vibememory_cli::agents::watched(layout);
    let watch_problem = vibememory_cli::agents::registry_problem(layout);
    let watched_wrong = watch_problem.is_some()
        || watched
            .iter()
            .any(vibememory_cli::agents::Watched::is_wrong);
    let failed = wrong > 0
        || mirror.is_some_and(Mirror::is_fault)
        || disk_low
        || tokens_wrong
        || teams_wrong
        || watched_wrong;
    let report = serde_json::json!({
        "version": vibememory_cli::update::CURRENT,
        "newerRelease": vibememory_cli::update::newer_known(layout),
        "machineId": config.machine_id,
        "remote": config.remote,
        "steps": steps,
        "stepsWrong": wrong,
        "mirror": mirror_value,
        "hostDisk": disk_value,
        "credentials": tokens_json(tokens),
        "held": held_json(&layout.engine_dir),
        "teams": teams_json(&teams),
        "clients": vibememory_cli::credentials::started_clients(&layout.engine_dir)
            .into_iter()
            .map(|(agent, start)| serde_json::json!({
                "agent": agent,
                "started": start.stamp,
                "version": start.version,
                "behind": start.behind(vibememory_cli::update::CURRENT),
            }))
            .collect::<Vec<_>>(),
        "watched": watched_json(&watched),
        "waiting": waiting_json(layout),
        "unwatched": unwatched_json(layout),
        "watchProblem": watch_problem,
        "agents": foreign_sessions(layout)
            .into_iter()
            .map(|(store, agent, sessions, newest)| serde_json::json!({
                "store": store, "agent": agent, "sessions": sessions, "newest": newest,
            }))
            .collect::<Vec<_>>(),
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
         update, mcp-config [<client>], connect --cabinet <address> [--agent <name>], \
         disconnect <team>, hook <event>, \
         merge-driver <jsonl|keepboth> %O %A %B %P, forget <session-id>, tick [--release-deletions], \
         relink <enc> <name> <cwd>, import <enc> <name> <cwd>, \
         session put --agent <name> --id <session-id> --cwd <dir> --from <file.jsonl> [--end], \
         migrate --from <dir> [--apply], switch --from <dir> [--apply|--rollback], --version"
    );
}
