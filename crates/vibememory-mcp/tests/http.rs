//! MCP over HTTP: who is let in, what is answered, and a real round trip over a socket.

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

use serde_json::Value;
use vibememory_mcp::http::{HttpRequest, MAX_BODY, answer, parse, serve};
use vibememory_mcp::memories::FakeMemories;

const TOKEN: &str = "test-token-0123456789abcdef";

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

#[test]
fn only_the_token_opens_the_door() {
    let fake = FakeMemories::new("2026-09-13T12:00:00Z");
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    assert_eq!(answer(&post(ping, &[]), TOKEN, &fake).status, 200);

    let mut wrong = post(ping, &[]);
    wrong.headers[0].1 = "Bearer test-token-0123456789abcdeX".into();
    let refused = answer(&wrong, TOKEN, &fake);
    assert_eq!(refused.status, 401);
    assert!(
        refused
            .headers
            .iter()
            .any(|(name, _)| *name == "WWW-Authenticate")
    );

    let mut none = post(ping, &[]);
    none.headers.clear();
    assert_eq!(answer(&none, TOKEN, &fake).status, 401);

    // A server started without a token refuses everyone rather than admitting everyone — including
    // a request that brings no token either, which an empty comparison would call a match.
    assert_eq!(answer(&post(ping, &[]), "", &fake).status, 401);
    let mut bare = post(ping, &[]);
    bare.headers[0].1 = "Bearer ".into();
    assert_eq!(answer(&bare, "", &fake).status, 401);
}

#[test]
fn a_web_page_is_turned_away_before_anything_else() {
    let fake = FakeMemories::new("2026-09-13T12:00:00Z");
    let ping = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let from_page = post(ping, &[("origin", "https://example.invalid")]);
    assert_eq!(answer(&from_page, TOKEN, &fake).status, 403);
}

#[test]
fn only_post_to_the_endpoint_is_served_and_a_notification_gets_no_body() {
    let fake = FakeMemories::new("2026-09-13T12:00:00Z");
    let mut get = post("", &[]);
    get.method = "GET".into();
    assert_eq!(answer(&get, TOKEN, &fake).status, 405);

    let mut elsewhere = post("{}", &[]);
    elsewhere.path = "/admin".into();
    assert_eq!(answer(&elsewhere, TOKEN, &fake).status, 404);

    let note = answer(
        &post(
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            &[],
        ),
        TOKEN,
        &fake,
    );
    assert_eq!((note.status, note.body.len()), (202, 0));
}

#[test]
fn the_agent_header_names_who_wrote_the_record() {
    let fake = FakeMemories::new("2026-09-13T12:00:00Z");
    let save = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"memory_save","arguments":{"project":"VibeIDE","id":"over-http","kind":"project","description":"d","body":"b"}}}"#;
    let answered = answer(
        &post(save, &[("vibememory-agent", "work-laptop")]),
        TOKEN,
        &fake,
    );
    assert_eq!(answered.status, 200);
    let events = fake.events.borrow();
    let written = serde_json::to_value(&events["VibeIDE"][0]).unwrap();
    assert_eq!(written["record"]["agent"], "work-laptop", "{written}");
}

#[test]
fn a_request_is_read_whole_and_refused_when_it_is_too_big() {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
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

#[test]
fn a_real_socket_gets_a_real_answer() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let fake = FakeMemories::new("2026-09-13T12:00:00Z");
        serve(&listener, TOKEN, &fake);
    });
    let body = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#;
    let mut stream = TcpStream::connect(address).expect("connect");
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
    let json: Value = serde_json::from_str(reply.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(
        json["result"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() == 6),
        "{json}"
    );
}
