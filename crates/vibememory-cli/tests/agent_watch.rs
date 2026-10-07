//! Watching an agent without hooks of its own: `session agent add`, the tick's look at the log
//! directory and the wrapper it runs — against real files and a real wrapper process.

// The test writes files and runs a wrapper, so the purity gate is lifted here.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use support::TempDir;
use vibememory_cli::agents::{
    Handler, NOTICE_AFTER, Preset, Readings, decline, declined_under, last_put,
    notice_waiting_under, record_put, register, registry, registry_problem, tick, unregister,
    waiting_under, watched,
};
use vibememory_cli::config::Config;
use vibememory_cli::install::Layout;
use vibememory_core::naming::PathSyntax;

const AGENT: &str = "dsh-desktop";

struct Machine {
    temp: TempDir,
    layout: Layout,
    config: Config,
    logs: PathBuf,
}

fn machine(label: &str) -> Machine {
    let temp = TempDir::new(label);
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
        home: None,
    };
    let logs = temp.dir("logs");
    let config = Config::parse(r#"{"machineId": "mac-main"}"#, PathSyntax::Posix).unwrap();
    Machine {
        temp,
        layout,
        config,
        logs,
    }
}

/// A log of the agent, written now, a few levels down as DSH keeps them. The wrapper gets it by
/// the canonical path of the registered directory, so a test compares canonical paths.
fn write_log(logs: &Path, name: &str) -> PathBuf {
    let dir = logs.join("enc").join(name);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.log");
    fs::write(&path, name).unwrap();
    path
}

/// Moves a file's time, as a log written at another moment.
fn set_modified(path: &Path, when: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// A wrapper that notes every log it is given and succeeds, or fails with words on stderr.
#[cfg(unix)]
fn wrapper(temp: &TempDir, name: &str, fails: bool) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let noted = temp.path().join(format!("{name}.noted"));
    let script = temp.path().join(format!("{name}.sh"));
    let tail = if fails {
        "echo 'reading the log' >&2\necho 'zstd: frame is cut' >&2\nexit 3\n"
    } else {
        "exit 0\n"
    };
    fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$VIBEMEMORY_AGENT $1\" >> '{}'\n{tail}",
            noted.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (script, noted)
}

/// A known agent whose logs lie on this machine and which nobody registered: the switch that was
/// never thrown is named, with the count of what waits behind it, and it stops being named the
/// moment it is thrown.
#[test]
fn an_unwatched_known_agent_is_reported_with_what_waits() {
    let m = machine("agent-waiting");
    let home = m.temp.path();
    let sessions = home.join(".dsh").join("sessions");
    let session = sessions.join("enc").join("one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"log").unwrap();
    // Beside a log lies its lock, and the lock is not a session: a directory of locks alone is an
    // agent that has not started, and saying otherwise would invent history.
    fs::write(session.join("session.lock"), b"").unwrap();

    let waiting = waiting_under(&m.layout, home);
    assert_eq!(waiting.len(), 1, "one known agent, one line");
    let dsh = &waiting[0];
    assert_eq!(dsh.agent, AGENT);
    assert_eq!(dsh.preset, Preset::Dsh);
    assert_eq!(dsh.logs, 1, "the lock beside the log is not a session");
    assert!(dsh.newest.is_some(), "when the newest log was written");
    let line = dsh.line();
    assert!(
        line.starts_with("waiting  DeepSeek Harness: 1 session(s)"),
        "{line}"
    );
    assert!(
        line.contains(AGENT) && line.contains("--backfill"),
        "{line}"
    );

    // Registered: the decision is made, and the same directory stops waiting.
    register(
        &m.layout,
        AGENT,
        &sessions,
        Handler::Preset(Preset::Dsh),
        false,
    )
    .unwrap();
    assert!(waiting_under(&m.layout, home).is_empty());

    // A home the agent never wrote into has nothing to say.
    assert!(waiting_under(&m.layout, &home.join("nohome")).is_empty());
}

/// An agent that is here with nothing written yet: still said, because the next session would go
/// the same way — nowhere — and the only moment to say it is before there is something to lose.
#[test]
fn a_known_agent_with_an_empty_log_directory_still_waits() {
    let m = machine("agent-waiting-empty");
    let home = m.temp.path();
    fs::create_dir_all(home.join(".dsh").join("sessions")).unwrap();

    let waiting = waiting_under(&m.layout, home);
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].logs, 0);
    let line = waiting[0].line();
    assert!(line.contains("keeps sessions in"), "{line}");
    assert_eq!(line.matches("waiting  ").count(), 1, "{line}");
}

/// Which agents hand sessions over and which hand over memory alone: the engine reads the logs of
/// a watched agent and of a known preset, and of nobody else. A client nobody can read is named,
/// because a person waiting for its sessions would wait forever.
#[test]
fn a_client_whose_sessions_never_arrive_is_named() {
    let m = machine("agent-readings");
    // A machine that only connects agents hands nothing over at all, and says that once, about
    // itself: a note under every agent would only repeat it.
    assert!(Readings::of(&m.layout).note("cursor").is_none());

    fs::create_dir_all(&m.layout.engine_dir).unwrap();
    fs::write(m.layout.engine_dir.join("config.json"), "{}").unwrap();
    let readings = Readings::of(&m.layout);
    assert!(
        readings.note("cursor").is_some(),
        "a client the engine cannot read hands over memory alone"
    );
    // The hooks agent writes the transcripts the engine syncs, and a known preset waits for its
    // registration — the `waiting` line says that one, with the count and the command.
    assert!(readings.note("claude-code").is_none());
    assert!(readings.note("claude-desktop").is_none());
    assert!(readings.note(AGENT).is_none());

    // A session of that client in the store settles it: its sessions do arrive.
    let session = m
        .layout
        .store()
        .join("projects")
        .join("Probe")
        .join("agents")
        .join("cursor");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session-one.jsonl"), b"{}").unwrap();
    assert!(Readings::of(&m.layout).note("cursor").is_none());

    // Registered agents are the engine's own business, and their line is the `watch` one.
    register(
        &m.layout,
        AGENT,
        &m.logs,
        Handler::Preset(Preset::Dsh),
        false,
    )
    .unwrap();
    assert!(Readings::of(&m.layout).note(AGENT).is_none());
}

/// The whole path a person walks: with an agent's logs on the machine and no registration, the
/// engine says it itself — in `status` and in `install` — and not only inside the library.
#[test]
fn the_engine_says_out_loud_that_an_unwatched_agent_hands_nothing_over() {
    let m = machine("agent-waiting-binary");
    let home = m.temp.path();
    fs::write(
        m.layout.engine_dir.join("config.json"),
        r#"{"machineId": "mac-main"}"#,
    )
    .unwrap();
    let session = home.join(".dsh").join("sessions").join("enc").join("one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"log").unwrap();

    let run = |arg: &str| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
            .arg(arg)
            .arg("--dry-run")
            .env("HOME", home)
            .env("VIBEMEMORY_DIR", &m.layout.engine_dir)
            .env("CLAUDE_CONFIG_DIR", &m.layout.config_dir)
            .output()
            .expect("the engine binary runs");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    // `--dry-run` belongs to `install`; `status` ignores it and only reads.
    for text in [run("status"), run("install")] {
        assert!(
            text.contains("waiting  DeepSeek Harness: 1 session(s)"),
            "the state of an unregistered agent is not said:\n{text}"
        );
        assert!(
            text.contains("--agent dsh-desktop --preset dsh --backfill"),
            "the line does not carry the command that changes it:\n{text}"
        );
    }
}

/// A question a person answered is not asked again: the refusal is written down, `doctor` names it
/// instead of the offer, and a removal is an answer too — otherwise the engine would argue with the
/// person after every `remove`.
#[test]
fn a_person_can_answer_the_question_and_the_engine_stops_asking() {
    let m = machine("agent-decline");
    let home = m.temp.path();
    let session = home.join(".dsh").join("sessions").join("enc").join("one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"log").unwrap();
    fs::write(
        m.layout.engine_dir.join("config.json"),
        r#"{"machineId": "mac-main"}"#,
    )
    .unwrap();

    assert_eq!(
        waiting_under(&m.layout, home).len(),
        1,
        "the offer is there"
    );

    decline(&m.layout, AGENT).unwrap();
    assert!(
        waiting_under(&m.layout, home).is_empty(),
        "an answered question is not repeated"
    );
    let answers = declined_under(&m.layout, home);
    assert_eq!(answers.len(), 1);
    assert_eq!(
        answers[0].logs, 1,
        "what waits behind the decision is counted"
    );
    let line = answers[0].line();
    assert!(
        line.starts_with("unwatched DeepSeek Harness: 1 session(s)"),
        "{line}"
    );
    assert!(line.contains("--agent dsh-desktop --preset dsh"), "{line}");

    // Registering over a refusal is the later word, and the removal writes the refusal back.
    register(
        &m.layout,
        AGENT,
        &session,
        Handler::Preset(Preset::Dsh),
        false,
    )
    .unwrap();
    assert!(
        declined_under(&m.layout, home).is_empty(),
        "the decision is taken back"
    );
    assert!(watched(&m.layout).iter().any(|one| one.agent == AGENT));
    record_put(&m.layout, AGENT).unwrap();
    assert_eq!(
        last_put(&m.layout, AGENT),
        Some(last_put(&m.layout, AGENT).unwrap())
    );

    assert!(unregister(&m.layout, AGENT).unwrap());
    assert_eq!(
        declined_under(&m.layout, home).len(),
        1,
        "a removal is an answer too"
    );
    assert!(
        waiting_under(&m.layout, home).is_empty(),
        "and it is not asked again"
    );
    assert_eq!(
        last_put(&m.layout, AGENT),
        None,
        "the note about a wrapper nobody runs would outlive the registration"
    );
}

/// The sentence a session gets: once a day, again when the logs grow, and never for an agent whose
/// fate is decided. The person is in the session; the machine's report is not what they read.
#[test]
fn a_session_is_told_once_a_day_and_again_when_the_logs_grow() {
    let m = machine("agent-notice");
    let home = m.temp.path();
    let session = home.join(".dsh").join("sessions").join("enc").join("one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"log").unwrap();
    let at = 1_800_000_000_u64;

    let first = notice_waiting_under(&m.layout, home, at).expect("the session is told");
    assert!(first.contains("not going to history"), "{first}");
    assert!(
        first.contains("session agent decline --agent dsh-desktop"),
        "{first}"
    );
    assert!(
        notice_waiting_under(&m.layout, home, at + 60).is_none(),
        "the same sentence at every session start is noise"
    );
    assert!(
        notice_waiting_under(&m.layout, home, at + NOTICE_AFTER + 1).is_some(),
        "tomorrow it is worth saying again"
    );

    // A count that changed is news: another log means another session that would be lost.
    fs::write(session.join("session.v5.jsonl.zstd"), b"log").unwrap();
    assert!(notice_waiting_under(&m.layout, home, at + NOTICE_AFTER + 2).is_some());

    // Nothing to lose yet: the directory is there and no session has been written.
    let empty = machine("agent-notice-empty");
    fs::create_dir_all(empty.temp.path().join(".dsh").join("sessions")).unwrap();
    assert!(notice_waiting_under(&empty.layout, empty.temp.path(), at).is_none());

    // Decided: the offer is gone, and a sentence about it would be about nothing.
    decline(&m.layout, AGENT).unwrap();
    assert!(notice_waiting_under(&m.layout, home, at + 10 * NOTICE_AFTER).is_none());
}

/// The strongest channel of the three: the session itself is told, because the person reads the
/// session and not the machine's report. And told once: the note that throttles the sentence is
/// written by the same run.
#[test]
fn the_session_start_hook_tells_the_session_about_a_waiting_agent() {
    use std::io::Write as _;

    let m = machine("agent-notice-hook");
    let home = m.temp.path();
    fs::write(
        m.layout.engine_dir.join("config.json"),
        r#"{"machineId": "mac-main"}"#,
    )
    .unwrap();
    let session = home.join(".dsh").join("sessions").join("enc").join("one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("session.v4.jsonl.zstd"), b"log").unwrap();
    let work = m.temp.dir("work");
    let input = format!(
        r#"{{"session_id":"11111111-1111-4111-8111-111111111111","transcript_path":"{}/projects/-tmp-work/11111111-1111-4111-8111-111111111111.jsonl","cwd":"{}","source":"startup"}}"#,
        m.layout.config_dir.display(),
        work.display()
    );

    let run = || {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_vibememory"))
            .args(["hook", "session-start"])
            .env("HOME", home)
            .env("VIBEMEMORY_DIR", &m.layout.engine_dir)
            .env("CLAUDE_CONFIG_DIR", &m.layout.config_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("the engine binary runs");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("the hook reads its input");
        let out = child.wait_with_output().expect("the hook answers");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let first = run();
    assert!(first.contains("not going to history"), "{first}");
    assert!(
        first.contains("session agent decline --agent dsh-desktop"),
        "the sentence carries the way to say no: {first}"
    );
    let second = run();
    assert!(
        second.is_empty(),
        "the same sentence at every session start is noise: {second}"
    );
}

#[test]
fn registration_checks_what_it_is_given() {
    let m = machine("agent-register");
    let file = m.temp.path().join("wrapper");
    fs::write(&file, "").unwrap();
    assert!(
        register(&m.layout, "Not A Slug", &m.logs, Handler::Run(&file), false).is_err(),
        "the agent becomes a file name"
    );
    assert!(
        register(
            &m.layout,
            AGENT,
            &m.temp.path().join("nowhere"),
            Handler::Run(&file),
            false
        )
        .is_err(),
        "a log directory that is not there watches nothing"
    );
    assert!(
        register(&m.layout, AGENT, &m.logs, Handler::Run(&m.logs), false).is_err(),
        "the wrapper is a program, not a directory"
    );
    let registration = register(&m.layout, AGENT, &m.logs, Handler::Run(&file), false).unwrap();
    assert_eq!(registry(&m.layout).get(AGENT), Some(&registration));
    assert!(unregister(&m.layout, AGENT).unwrap());
    assert!(
        !unregister(&m.layout, AGENT).unwrap(),
        "a second removal finds nothing"
    );
    assert!(registry(&m.layout).is_empty());
}

#[cfg(unix)]
#[test]
fn the_tick_hands_each_new_log_to_the_wrapper_once() {
    let m = machine("agent-tick");
    let before = write_log(&m.logs, "before");
    let (script, noted) = wrapper(&m.temp, "ok", false);
    register(&m.layout, AGENT, &m.logs, Handler::Run(&script), false).unwrap();

    let ran = tick(&m.layout, &m.config);
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (0, 0),
        "history is not handed over"
    );
    assert!(!noted.exists());

    let fresh = write_log(&m.logs, "fresh");
    let ran = tick(&m.layout, &m.config);
    assert_eq!(ran[0].handed, 1);
    assert_eq!(
        fs::read_to_string(&noted).unwrap(),
        format!("{AGENT} {}\n", fs::canonicalize(&fresh).unwrap().display()),
        "the wrapper gets the log's path, and the agent's name in its environment"
    );

    assert_eq!(
        tick(&m.layout, &m.config)[0].handed,
        0,
        "a log is handed over once"
    );
    set_modified(&before, SystemTime::now() + Duration::from_secs(5));
    assert_eq!(
        tick(&m.layout, &m.config)[0].handed,
        1,
        "and again once it is written to"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_wrapper_is_written_down_and_tried_again() {
    let m = machine("agent-fail");
    let (script, _) = wrapper(&m.temp, "fails", true);
    register(&m.layout, AGENT, &m.logs, Handler::Run(&script), false).unwrap();
    let log = write_log(&m.logs, "one");

    let ran = tick(&m.layout, &m.config);
    let failure = ran[0].failure.clone().expect("the wrapper failed");
    assert_eq!(failure.outcome, "exit code 3");
    assert_eq!(
        failure.log,
        fs::canonicalize(&log).unwrap().display().to_string()
    );
    assert_eq!(
        failure.lines,
        vec!["reading the log", "zstd: frame is cut"],
        "what the wrapper said is kept for doctor, not lost in a console"
    );
    assert_eq!(watched(&m.layout)[0].failure, Some(failure));
    assert_eq!(
        tick(&m.layout, &m.config)[0].handed,
        0,
        "the log it failed on stays new and is tried again"
    );
    assert!(tick(&m.layout, &m.config)[0].failure.is_some());

    // The log it failed on is gone: nothing is failing any more, and doctor must not stay red.
    fs::remove_file(&log).unwrap();
    assert_eq!(tick(&m.layout, &m.config)[0].failure, None);
    assert_eq!(watched(&m.layout)[0].failure, None);
}

#[cfg(unix)]
#[test]
fn a_tick_hands_over_a_bounded_number_of_logs() {
    let m = machine("agent-bound");
    let (script, _) = wrapper(&m.temp, "ok", false);
    register(&m.layout, AGENT, &m.logs, Handler::Run(&script), false).unwrap();
    for index in 0..=vibememory_cli::agents::LOGS_PER_TICK {
        write_log(&m.logs, &format!("log-{index}"));
    }
    let ran = tick(&m.layout, &m.config);
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (vibememory_cli::agents::LOGS_PER_TICK, 1)
    );
    let ran = tick(&m.layout, &m.config);
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (1, 0),
        "the rest go next run"
    );
}

#[test]
fn an_agent_whose_logs_outrun_its_sessions_is_silent() {
    let m = machine("agent-silent");
    let file = m.temp.path().join("wrapper");
    fs::write(&file, "").unwrap();
    register(&m.layout, AGENT, &m.logs, Handler::Run(&file), false).unwrap();
    let log = write_log(&m.logs, "talking");
    assert!(
        !watched(&m.layout)[0].silent,
        "a log just written is not silence yet"
    );

    set_modified(&log, SystemTime::now() + Duration::from_mins(20));
    let seen = &watched(&m.layout)[0];
    assert!(seen.silent, "logs twenty minutes ahead of any session put");
    assert_eq!(seen.last_put, None);

    record_put(&m.layout, AGENT).unwrap();
    assert!(
        watched(&m.layout)[0].last_put.is_some(),
        "session put notes itself"
    );
}

#[test]
fn a_registry_that_cannot_be_read_is_said() {
    let m = machine("agent-broken");
    assert_eq!(registry_problem(&m.layout), None, "no file is no problem");
    fs::write(m.layout.engine_dir.join("agents.json"), "{ not json").unwrap();
    assert!(registry_problem(&m.layout).is_some());
    assert!(
        tick(&m.layout, &m.config).is_empty(),
        "the tick goes on without the agents"
    );
}

#[test]
fn a_missing_log_directory_is_said() {
    let m = machine("agent-missing");
    let file = m.temp.path().join("wrapper");
    fs::write(&file, "").unwrap();
    register(&m.layout, AGENT, &m.logs, Handler::Run(&file), false).unwrap();
    fs::remove_dir_all(&m.logs).unwrap();
    assert!(tick(&m.layout, &m.config)[0].missing);
    assert!(watched(&m.layout)[0].missing);
}

/// A log of DSH from `fixtures/foreign/dshLogs.json`: zstd frames as Node writes them.
fn dsh_log(id: &str) -> Vec<u8> {
    let file: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/foreign/dshLogs.json")).unwrap();
    let text = file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap()["log"]
        .as_str()
        .unwrap()
        .to_owned();
    let value = |c: u8| match c {
        b'A'..=b'Z' => u32::from(c - b'A'),
        b'a'..=b'z' => u32::from(c - b'a' + 26),
        b'0'..=b'9' => u32::from(c - b'0' + 52),
        b'+' => 62,
        _ => 63,
    };
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0);
    for &c in text.trim_end_matches('=').as_bytes() {
        buffer = (buffer << 6) | value(c);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).unwrap());
        }
    }
    out
}

/// A session directory as DSH keeps it: the log, and the lock beside it.
fn write_dsh_session(logs: &Path, session: &str, log: &[u8]) -> PathBuf {
    let dir = logs.join("-work-Promed").join(session);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.lock"), "").unwrap();
    let path = dir.join("session.v4.jsonl.zstd");
    fs::write(&path, log).unwrap();
    path
}

/// A log of DSH for a session in `cwd`, compressed in two frames as DSH writes them.
fn dsh_log_for(cwd: &str, session: &str) -> Vec<u8> {
    let lines = |records: &[serde_json::Value]| {
        let mut text = String::new();
        for record in records {
            text.push_str(&record.to_string());
            text.push('\n');
        }
        text
    };
    let first = lines(&[
        serde_json::json!({"type": "session", "version": 4, "id": session, "cwd": cwd}),
        serde_json::json!({"type": "user/message", "seq": 1, "time": 1_790_000_001_000_u64,
            "data": {"id": "u-1", "content": [{"type": "text", "text": "list the files"}]}}),
    ]);
    let second = lines(&[serde_json::json!({"type": "assistant/message", "seq": 2,
        "time": 1_790_000_002_000_u64,
        "data": {"message": {"id": "a-1", "content": [{"type": "text", "text": "here"}]}}})]);
    let mut log = Vec::new();
    for frame in [first, second] {
        log.extend(ruzstd::encoding::compress_to_vec(
            frame.as_bytes(),
            ruzstd::encoding::CompressionLevel::Fastest,
        ));
    }
    log
}

#[test]
fn the_dsh_preset_reads_the_log_itself_and_takes_the_past_when_asked() {
    let m = machine("agent-dsh");
    fs::create_dir_all(m.layout.store()).unwrap();
    support::git_repo_with_commit(&m.layout.store());
    let work = fs::canonicalize(m.temp.dir("work/Promed")).unwrap();
    let session = "session-22222222-2222-4222-8222-000000000009";
    write_dsh_session(
        &m.logs,
        "past",
        &dsh_log_for(&work.display().to_string(), session),
    );
    write_dsh_session(&m.logs, "just-begun", &dsh_log("emptyLog"));

    register(
        &m.layout,
        AGENT,
        &m.logs,
        Handler::Preset(Preset::Dsh),
        true,
    )
    .unwrap();
    let ran = tick(&m.layout, &m.config);
    assert_eq!(
        (ran[0].failure.clone(), ran[0].skipped.clone()),
        (None, vec![])
    );
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (2, 0),
        "with --backfill the past goes too; a lock is not a log, an empty log is nothing yet"
    );
    let placed = m
        .layout
        .store()
        .join("projects/Promed/agents")
        .join(AGENT)
        .join(format!("{session}.jsonl"));
    let text = fs::read_to_string(&placed).unwrap();
    assert_eq!(text.lines().count(), 2, "both frames, both messages");
    assert!(watched(&m.layout)[0].last_put.is_some());
    assert_eq!(tick(&m.layout, &m.config)[0].handed, 0, "once");
}

#[test]
fn a_dsh_session_whose_directory_is_gone_is_set_aside_and_the_rest_go_on() {
    let m = machine("agent-dsh-gone");
    fs::create_dir_all(m.layout.store()).unwrap();
    support::git_repo_with_commit(&m.layout.store());
    let work = fs::canonicalize(m.temp.dir("work/Promed")).unwrap();
    // The fixture's session names a directory that is not on this disk.
    let gone = write_dsh_session(&m.logs, "gone", &dsh_log("framesJoined"));
    set_modified(&gone, SystemTime::now() - Duration::from_mins(1));
    write_dsh_session(
        &m.logs,
        "here",
        &dsh_log_for(
            &work.display().to_string(),
            "session-22222222-2222-4222-8222-000000000010",
        ),
    );
    register(
        &m.layout,
        AGENT,
        &m.logs,
        Handler::Preset(Preset::Dsh),
        true,
    )
    .unwrap();

    let ran = tick(&m.layout, &m.config);
    assert_eq!(
        ran[0].failure, None,
        "one session is no fault of the integration"
    );
    assert_eq!(
        (ran[0].handed, ran[0].skipped.len()),
        (1, 1),
        "the other one went"
    );
    let seen = &watched(&m.layout)[0];
    assert!(!seen.is_wrong());
    assert_eq!(seen.skipped.len(), 1, "doctor names what did not go");
    assert_eq!(
        tick(&m.layout, &m.config)[0].skipped.len(),
        0,
        "not tried every run"
    );
    set_modified(&gone, SystemTime::now() + Duration::from_secs(5));
    assert_eq!(
        tick(&m.layout, &m.config)[0].skipped.len(),
        1,
        "tried again once it is written to"
    );
}

#[test]
fn the_dsh_preset_without_backfill_starts_from_now() {
    let m = machine("agent-dsh-now");
    write_dsh_session(&m.logs, "past", &dsh_log("framesJoined"));
    register(
        &m.layout,
        AGENT,
        &m.logs,
        Handler::Preset(Preset::Dsh),
        false,
    )
    .unwrap();
    assert_eq!(tick(&m.layout, &m.config)[0].handed, 0);
}

#[test]
fn a_dsh_log_of_an_unknown_version_fails_loudly() {
    let m = machine("agent-dsh-v5");
    fs::create_dir_all(m.layout.store()).unwrap();
    support::git_repo_with_commit(&m.layout.store());
    register(
        &m.layout,
        AGENT,
        &m.logs,
        Handler::Preset(Preset::Dsh),
        false,
    )
    .unwrap();
    write_dsh_session(&m.logs, "future", &dsh_log("unknownVersion"));
    let failure = tick(&m.layout, &m.config)[0].failure.clone().unwrap();
    assert!(
        failure.outcome.contains("v5") && failure.outcome.contains("vibememory update"),
        "{}",
        failure.outcome
    );
    assert!(watched(&m.layout)[0].is_wrong(), "doctor goes red");
}
