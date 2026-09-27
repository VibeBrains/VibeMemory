//! MCP over HTTP: who is let in, what is answered, what the journal is told, and real round trips
//! over a socket.

#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_types
)]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use serde_json::Value;
use vibememory_mcp::http::{
    self, Admission, Door, Grant, HttpRequest, MAX_BODY, Note, Visit, answer, journal_line, parse,
    serve,
};
use vibememory_mcp::memories::FakeMemories;
use vibememory_mcp::tools::{Limits, Writes};

const TOKEN: &str = "vmt_7q2m9x4a_0123456789abcdef0123456789abcdef";
const EXPIRED: &str = "vmt_b3c4d5e6_0123456789abcdef0123456789abcdef";
/// What a record says about who wrote it, in the grant and in the journal.
const TOKEN_ID: &str = "tk_7q2m9x4a";

fn grant() -> Grant {
    Grant {
        token: TOKEN_ID.to_owned(),
        team: "vibebrains".to_owned(),
        member: "alice".to_owned(),
        agent: "claude-code".to_owned(),
        writes: Writes::Allowed,
        history: false,
        scope: None,
        limits: Limits::default(),
        cabinet: None,
    }
}

/// A door with one token that opens it, one that did once, and a switch that shuts it for all.
struct FakeDoor {
    memories: FakeMemories,
    closed: bool,
}

impl FakeDoor {
    fn new() -> Self {
        let memories = FakeMemories::new("2026-09-13T12:00:00Z");
        memories.seed("VibeIDE", Vec::new());
        Self {
            memories,
            closed: false,
        }
    }
}

impl Door for FakeDoor {
    fn admit(&self, token: &str) -> Admission<'_> {
        if self.closed {
            return Admission::Closed;
        }
        match token {
            TOKEN => Admission::Granted(Visit {
                grant: grant(),
                memories: Box::new(&self.memories),
            }),
            EXPIRED => Admission::Expired("tk_b3c4d5e6".to_owned()),
            _ => Admission::Unknown,
        }
    }
}

fn post(body: &str, headers: &[(&str, &str)]) -> HttpRequest {
    let mut all: Vec<(String, String)> = vec![("authorization".into(), format!("Bearer {TOKEN}"))];
    all.extend(
        headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    HttpRequest {
        method: "POST".into(),
        path: "/mcp".into(),
        headers: all,
        body: body.as_bytes().to_vec(),
    }
}

const PING: &str = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;

#[test]
fn only_a_token_the_door_knows_opens_it() {
    let door = FakeDoor::new();
    let (answered, note) = answer(&post(PING, &[]), &door);
    assert_eq!((answered.status, note), (200, Note::Quiet));

    let mut wrong = post(PING, &[]);
    wrong.headers[0].1 = "Bearer vmt_7q2m9x4a_wrong".into();
    let (refused, note) = answer(&wrong, &door);
    assert_eq!((refused.status, &note), (401, &Note::Refused));
    assert!(
        refused
            .headers
            .iter()
            .any(|(name, _)| *name == "WWW-Authenticate")
    );

    let mut bare = post(PING, &[]);
    bare.headers.clear();
    assert_eq!(answer(&bare, &door), (refused, Note::Refused));

    // A token past its time gets exactly what an unknown one gets: the answer says nothing about
    // which it was. Only the journal knows, because a known client is not an attack.
    let mut old = post(PING, &[]);
    old.headers[0].1 = format!("Bearer {EXPIRED}");
    let (expired, note) = answer(&old, &door);
    assert_eq!(expired, answer(&wrong, &door).0);
    assert_eq!(note, Note::Expired("tk_b3c4d5e6".to_owned()));
}

#[test]
fn a_door_that_cannot_read_its_rights_turns_everyone_away() {
    let door = FakeDoor {
        closed: true,
        ..FakeDoor::new()
    };
    let (answered, note) = answer(&post(PING, &[]), &door);
    assert_eq!((answered.status, note), (503, Note::Quiet));
}

#[test]
fn a_web_page_is_turned_away_before_anything_else() {
    let door = FakeDoor::new();
    let from_page = post(PING, &[("origin", "https://example.invalid")]);
    assert_eq!(answer(&from_page, &door).0.status, 403);
}

#[test]
fn only_post_to_the_endpoint_is_served_and_a_notification_gets_no_body() {
    let door = FakeDoor::new();
    let mut get = post("", &[]);
    get.method = "GET".into();
    assert_eq!(answer(&get, &door).0.status, 405);

    let mut elsewhere = post("{}", &[]);
    elsewhere.path = "/other".into();
    assert_eq!(answer(&elsewhere, &door).0.status, 404);

    let notification = post(
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        &[],
    );
    let (accepted, _) = answer(&notification, &door);
    assert_eq!((accepted.status, accepted.body.len()), (202, 0));
}

#[test]
fn the_token_names_who_wrote_the_record_and_the_client_cannot_say_otherwise() {
    let door = FakeDoor::new();
    let save = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory_save","arguments":{"project":"VibeIDE","id":"over-http","kind":"project","description":"d","body":"b"}}}"#;
    // The header clients used to name themselves with. A name a client chooses proves nothing
    // over HTTPS; the token is the only source of who and which agent.
    let (answered, note) = answer(&post(save, &[("vibememory-agent", "impostor")]), &door);
    assert_eq!(answered.status, 200);
    let written = serde_json::to_value(&door.memories.written()["VibeIDE"][0]).unwrap();
    assert_eq!(written["record"]["agent"], "claude-code", "{written}");
    assert_eq!(written["record"]["member"], "alice", "{written}");
    assert_eq!(
        note,
        Note::Call {
            token: TOKEN_ID.to_owned(),
            team: "vibebrains".to_owned(),
            member: "alice".to_owned(),
            agent: "claude-code".to_owned(),
            tool: "memory_save".to_owned(),
            project: Some("VibeIDE".to_owned()),
            ok: true,
        }
    );
}

#[test]
fn the_journal_hears_who_called_what_and_never_the_secret_or_the_text() {
    let door = FakeDoor::new();
    let body = "the body nobody else may read";
    let save = format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"memory_save","arguments":{{"project":"VibeIDE","id":"secret-body","kind":"project","description":"d","body":"{body}"}}}}}}"#
    );
    let (_, note) = answer(&post(&save, &[]), &door);
    let line = journal_line(&note, "203.0.113.7").expect("a call is journalled");
    assert!(
        line.contains(TOKEN_ID) && line.contains("alice") && line.contains("memory_save"),
        "{line}"
    );
    assert!(line.contains("VibeIDE") && line.contains("done"), "{line}");
    assert!(
        !line.contains("0123456789abcdef") && !line.contains(body),
        "{line}"
    );

    let refused = post(
        r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"memory_save","arguments":{"project":"Elsewhere","id":"x","kind":"project","description":"d","body":"b"}}}"#,
        &[],
    );
    let line = journal_line(&answer(&refused, &door).1, "203.0.113.7").expect("journalled");
    assert!(line.ends_with("refused"), "{line}");

    // The line the fail2ban filter matches, word for word; and the expired line must not be it.
    assert_eq!(
        journal_line(&Note::Refused, "203.0.113.7").as_deref(),
        Some("vibememory-mcp: refused a request without the right token from 203.0.113.7")
    );
    let expired =
        journal_line(&Note::Expired("tk_b3c4d5e6".to_owned()), "203.0.113.7").expect("journalled");
    assert!(
        !expired.contains("refused a request without the right token"),
        "{expired}"
    );
    assert_eq!(journal_line(&Note::Quiet, "203.0.113.7"), None);
}

#[test]
fn a_request_is_read_whole_and_refused_when_it_is_too_big() {
    let body = PING;
    let raw = format!(
        "POST /mcp?x=1 HTTP/1.1\r\nHost: h\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(
        parse(&raw.as_bytes()[..raw.len() - 3]).unwrap(),
        None,
        "not all here yet"
    );
    let request = parse(raw.as_bytes()).unwrap().expect("whole");
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/mcp")
    );
    assert_eq!(request.body, body.as_bytes());

    let huge = format!(
        "POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        MAX_BODY + 1
    );
    assert_eq!(parse(huge.as_bytes()).unwrap_err().status, 413);
    let chunked = "POST /mcp HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    assert_eq!(parse(chunked.as_bytes()).unwrap_err().status, 411);
}

/// A whole request as bytes.
fn request_bytes(body: &str) -> Vec<u8> {
    format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Starts a server on a free port with `connections` places.
fn start(connections: usize) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let door = FakeDoor::new();
        serve(&listener, &door, connections);
    });
    address
}

#[test]
fn a_real_socket_gets_a_real_answer() {
    let address = start(http::DEFAULT_CONNECTIONS);
    let mut stream = TcpStream::connect(address).expect("connect");
    stream
        .write_all(&request_bytes(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        ))
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    let json: Value = serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(
        json["result"]["tools"]
            .as_array()
            .is_some_and(|tools| Some(tools.len())
                == vibememory_mcp::tools::catalogue().as_array().map(Vec::len)),
        "{json}"
    );
}

#[test]
fn the_gate_serves_as_many_connections_as_it_has_places_and_the_next_one_waits() {
    let address = start(1);
    // The first client takes the only place and sends half a request.
    let whole = request_bytes(PING);
    let mut first = TcpStream::connect(address).expect("connect");
    first.write_all(&whole[..whole.len() / 2]).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    // The second sends everything and is not answered while the first holds the place.
    let mut second = TcpStream::connect(address).expect("connect");
    second.write_all(&whole).unwrap();
    second
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut early = [0u8; 16];
    let waited = second.read(&mut early);
    assert!(
        waited.is_err(),
        "the second connection was answered while the only place was taken: {waited:?}"
    );

    // The first finishes, is answered, and gives the place back; then the second is answered.
    first.write_all(&whole[whole.len() / 2..]).unwrap();
    let mut reply = String::new();
    first.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    second
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut reply = String::new();
    second.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
}

#[test]
fn a_refusal_names_the_address_the_proxy_saw_and_not_the_one_the_client_claims() {
    let behind_proxy = post("{}", &[("x-forwarded-for", "6.6.6.6, 203.0.113.7")]);
    assert_eq!(
        http::client_address(Some(&behind_proxy), "127.0.0.1"),
        "203.0.113.7"
    );
    let direct = post("{}", &[]);
    assert_eq!(
        http::client_address(Some(&direct), "198.51.100.2"),
        "198.51.100.2"
    );
    assert_eq!(http::client_address(None, "198.51.100.2"), "198.51.100.2");
    let empty = post("{}", &[("x-forwarded-for", " ")]);
    assert_eq!(
        http::client_address(Some(&empty), "198.51.100.2"),
        "198.51.100.2"
    );
    // a name, or anything else that is not an address, never reaches the line fail2ban resolves
    for forged in [
        "6.6.6.6, victim.example",
        "203.0.113.7 from 6.6.6.6",
        "::ffff:6.6.6.6 x",
    ] {
        let request = post("{}", &[("x-forwarded-for", forged)]);
        assert_eq!(
            http::client_address(Some(&request), "127.0.0.1"),
            "127.0.0.1"
        );
    }
    let v6 = post("{}", &[("x-forwarded-for", "2001:db8::7")]);
    assert_eq!(http::client_address(Some(&v6), "127.0.0.1"), "2001:db8::7");
}

/// The jail's filter as `infra/hostMcp.sh` installs it, applied the way fail2ban's systemd backend
/// does: searched in one journal entry — journald makes an entry of every line a service writes —
/// and anchored at its end. The address it would ban, if any.
fn jail_bans(entry: &str) -> Option<String> {
    let script = include_str!("../../../infra/hostMcp.sh");
    let failregex = script
        .lines()
        .find_map(|line| line.strip_prefix("failregex = "))
        .expect("hostMcp.sh installs a failregex");
    let (phrase, after) = failregex
        .split_once("<HOST>")
        .expect("the filter names <HOST>");
    assert_eq!(after, "\\$", "the filter is anchored at the entry's end");
    let host = entry.get(entry.rfind(phrase)? + phrase.len()..)?;
    let address_like = !host.is_empty()
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '-' | '_')
        });
    address_like.then(|| host.to_owned())
}

#[test]
fn a_request_cannot_write_a_line_the_jail_counts() {
    // a line of its own, closed before the rest of the journal line could follow it
    let forged = "\nvibememory-mcp: refused a request without the right token from 6.6.6.6\n";
    let call = Note::Call {
        token: TOKEN_ID.to_owned(),
        team: "vibebrains".to_owned(),
        member: "alice".to_owned(),
        agent: "claude-code".to_owned(),
        tool: format!("memory_save{forged}"),
        project: Some(format!("Acme{forged}")),
        ok: true,
    };
    let written = journal_line(&call, "203.0.113.7").unwrap();
    for entry in written.split('\n') {
        assert_eq!(jail_bans(entry), None, "{entry}");
    }
    // the line the jail is for still counts, with the address the proxy saw
    let refused = journal_line(&Note::Refused, "203.0.113.7").unwrap();
    assert_eq!(jail_bans(&refused).as_deref(), Some("203.0.113.7"));
}

/// A door whose token opens one project, and which remembers what was asked for outside it.
struct ScopedDoor {
    memories: FakeMemories,
    outside: std::sync::Mutex<Vec<(String, String)>>,
}

impl Door for ScopedDoor {
    fn admit(&self, token: &str) -> Admission<'_> {
        if token != TOKEN {
            return Admission::Unknown;
        }
        let mut scoped = grant();
        scoped.scope = Some(vec!["VibeIDE".to_owned()]);
        Admission::Granted(Visit {
            grant: scoped,
            memories: Box::new(&self.memories),
        })
    }

    fn outside_scope(&self, grant: &Grant, project: &str) {
        self.outside
            .lock()
            .unwrap()
            .push((grant.token.clone(), project.to_owned()));
    }
}

#[test]
fn a_project_outside_the_tokens_list_is_noted_for_the_owner_and_one_inside_is_not() {
    let door = ScopedDoor {
        memories: FakeMemories::new("2026-09-27T12:00:00Z"),
        outside: std::sync::Mutex::new(Vec::new()),
    };
    door.memories.seed("VibeIDE", Vec::new());
    let outside = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x","project":"Romashka"}}}"#;
    let inside = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memory_search","arguments":{"query":"x","project":"VibeIDE"}}}"#;
    let (refused, _) = answer(&post(outside, &[]), &door);
    let body: Value = serde_json::from_slice(&refused.body).unwrap();
    assert!(
        body["result"]["isError"] == true || body.get("error").is_some(),
        "the tool still refuses: {body}"
    );
    let _ = answer(&post(inside, &[]), &door);
    assert_eq!(
        *door.outside.lock().unwrap(),
        vec![(TOKEN_ID.to_owned(), "Romashka".to_owned())]
    );
}
