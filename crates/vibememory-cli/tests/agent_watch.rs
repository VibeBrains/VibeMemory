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
    record_put, register, registry, registry_problem, tick, unregister, watched,
};
use vibememory_cli::install::Layout;

const AGENT: &str = "dsh-desktop";

struct Machine {
    temp: TempDir,
    layout: Layout,
    logs: PathBuf,
}

fn machine(label: &str) -> Machine {
    let temp = TempDir::new(label);
    let layout = Layout {
        config_dir: temp.dir("claude"),
        engine_dir: temp.dir("engine"),
    };
    let logs = temp.dir("logs");
    Machine { temp, layout, logs }
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

#[test]
fn registration_checks_what_it_is_given() {
    let m = machine("agent-register");
    let file = m.temp.path().join("wrapper");
    fs::write(&file, "").unwrap();
    assert!(
        register(&m.layout, "Not A Slug", &m.logs, &file).is_err(),
        "the agent becomes a file name"
    );
    assert!(
        register(&m.layout, AGENT, &m.temp.path().join("nowhere"), &file).is_err(),
        "a log directory that is not there watches nothing"
    );
    assert!(
        register(&m.layout, AGENT, &m.logs, &m.logs).is_err(),
        "the wrapper is a program, not a directory"
    );
    let registration = register(&m.layout, AGENT, &m.logs, &file).unwrap();
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
    register(&m.layout, AGENT, &m.logs, &script).unwrap();

    let ran = tick(&m.layout);
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (0, 0),
        "history is not handed over"
    );
    assert!(!noted.exists());

    let fresh = write_log(&m.logs, "fresh");
    let ran = tick(&m.layout);
    assert_eq!(ran[0].handed, 1);
    assert_eq!(
        fs::read_to_string(&noted).unwrap(),
        format!("{AGENT} {}\n", fs::canonicalize(&fresh).unwrap().display()),
        "the wrapper gets the log's path, and the agent's name in its environment"
    );

    assert_eq!(tick(&m.layout)[0].handed, 0, "a log is handed over once");
    set_modified(&before, SystemTime::now() + Duration::from_secs(5));
    assert_eq!(
        tick(&m.layout)[0].handed,
        1,
        "and again once it is written to"
    );
}

#[cfg(unix)]
#[test]
fn a_failing_wrapper_is_written_down_and_tried_again() {
    let m = machine("agent-fail");
    let (script, _) = wrapper(&m.temp, "fails", true);
    register(&m.layout, AGENT, &m.logs, &script).unwrap();
    let log = write_log(&m.logs, "one");

    let ran = tick(&m.layout);
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
        tick(&m.layout)[0].handed,
        0,
        "the log it failed on stays new and is tried again"
    );
    assert!(tick(&m.layout)[0].failure.is_some());

    // The log it failed on is gone: nothing is failing any more, and doctor must not stay red.
    fs::remove_file(&log).unwrap();
    assert_eq!(tick(&m.layout)[0].failure, None);
    assert_eq!(watched(&m.layout)[0].failure, None);
}

#[cfg(unix)]
#[test]
fn a_tick_hands_over_a_bounded_number_of_logs() {
    let m = machine("agent-bound");
    let (script, _) = wrapper(&m.temp, "ok", false);
    register(&m.layout, AGENT, &m.logs, &script).unwrap();
    for index in 0..=vibememory_cli::agents::LOGS_PER_TICK {
        write_log(&m.logs, &format!("log-{index}"));
    }
    let ran = tick(&m.layout);
    assert_eq!(
        (ran[0].handed, ran[0].waiting),
        (vibememory_cli::agents::LOGS_PER_TICK, 1)
    );
    let ran = tick(&m.layout);
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
    register(&m.layout, AGENT, &m.logs, &file).unwrap();
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
        tick(&m.layout).is_empty(),
        "the tick goes on without the agents"
    );
}

#[test]
fn a_missing_log_directory_is_said() {
    let m = machine("agent-missing");
    let file = m.temp.path().join("wrapper");
    fs::write(&file, "").unwrap();
    register(&m.layout, AGENT, &m.logs, &file).unwrap();
    fs::remove_dir_all(&m.logs).unwrap();
    assert!(tick(&m.layout)[0].missing);
    assert!(watched(&m.layout)[0].missing);
}
