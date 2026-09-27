//! The local server in a directory of a token-only team: requests go to the team's server over a
//! real socket through curl, and a write without a project gets the directory's.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::net::TcpListener;

use serde_json::{Value, json};
use vibememory_mcp::http::{Admission, Door, Grant, Visit, serve};
use vibememory_mcp::memories::FakeMemories;
use vibememory_mcp::proxy::{Remote, with_project};
use vibememory_mcp::tools::{Limits, Writes};

/// A private directory for one test, gone when the test ends.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vibememory-proxy-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TOKEN: &str = "vmt_7q2m9x4a_0123456789abcdef0123456789abcdef";

struct OneToken {
    memories: FakeMemories,
}

impl Door for OneToken {
    fn admit(&self, token: &str) -> Admission<'_> {
        if token != TOKEN {
            return Admission::Unknown;
        }
        Admission::Granted(Visit {
            grant: Grant {
                token: "tk_7q2m9x4a".to_owned(),
                team: "acme".to_owned(),
                member: "alice".to_owned(),
                agent: "claude-code".to_owned(),
                writes: Writes::Allowed,
                history: false,
                scope: None,
                limits: Limits::default(),
                cabinet: None,
            },
            memories: Box::new(&self.memories),
        })
    }
}

#[test]
fn a_write_without_a_project_gets_the_directorys_and_nothing_else_changes() {
    let save = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"memory_save","arguments":{"id":"a","kind":"user","description":"d","body":"b"}}})
    .to_string();
    let filled: Value = serde_json::from_str(&with_project(&save, Some("Acme"))).unwrap();
    assert_eq!(filled["params"]["arguments"]["project"], "Acme");

    let named = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":{"name":"memory_save","arguments":{"project":"Other"}}})
    .to_string();
    assert_eq!(
        with_project(&named, Some("Acme")),
        named,
        "a named project stays"
    );

    let search = json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"memory_search","arguments":{"query":"x"}}})
    .to_string();
    assert_eq!(
        with_project(&search, Some("Acme")),
        search,
        "a search still spans every project"
    );
    assert_eq!(with_project("not json", Some("Acme")), "not json");
}

#[test]
fn requests_reach_the_teams_server_and_its_answers_come_back_line_by_line() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let memories = FakeMemories::new("2026-09-27T12:00:00Z");
        memories.seed("Acme", Vec::new());
        serve(&listener, &OneToken { memories }, 4);
    });
    let temp = Scratch::new("proxy");
    let remote = Remote {
        url: format!("http://{address}/mcp"),
        authorization: format!("Authorization: Bearer {TOKEN}"),
        project: Some("Acme".to_owned()),
        scratch: temp.path().to_path_buf(),
    };
    let input = [
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_save",
            "arguments":{"id":"proxied","kind":"user","description":"through the proxy","body":"b"}}})
        .to_string(),
    ]
    .join("\n");
    let mut output = Vec::new();
    vibememory_mcp::proxy::serve(&remote, input.as_bytes(), &mut output).unwrap();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "a notification is not answered: {lines:?}");
    assert!(lines[0]["result"]["tools"].is_array(), "{}", lines[0]);
    assert_eq!(lines[1]["id"], 2);
    assert!(
        lines[1]["result"]["isError"] != true && lines[1].get("error").is_none(),
        "the write went into the directory's project: {}",
        lines[1]
    );
    assert!(
        std::fs::read_dir(temp.path()).unwrap().next().is_none(),
        "the file that carried the token is gone"
    );
}

#[test]
fn a_refused_token_is_said_to_the_agent_not_swallowed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let memories = FakeMemories::new("2026-09-27T12:00:00Z");
        serve(&listener, &OneToken { memories }, 4);
    });
    let temp = Scratch::new("proxy-refused");
    let remote = Remote {
        url: format!("http://{address}/mcp"),
        authorization: "Authorization: Bearer vmt_revoked".to_owned(),
        project: None,
        scratch: temp.path().to_path_buf(),
    };
    let input = json!({"jsonrpc":"2.0","id":7,"method":"tools/list"}).to_string();
    let mut output = Vec::new();
    vibememory_mcp::proxy::serve(&remote, input.as_bytes(), &mut output).unwrap();
    let answer: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(answer["id"], 7);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("revoked"),
        "{answer}"
    );
}
