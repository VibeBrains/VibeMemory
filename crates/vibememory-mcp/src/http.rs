//! MCP over HTTP (Streamable HTTP, revision 2025-06-18), for agents that cannot start a process or
//! reach the host over ssh.
//!
//! Deliberately small: one JSON-RPC message per `POST /mcp`, answered with one JSON body; no
//! server-sent events, no sessions. It listens on a loopback port behind a TLS proxy, and every
//! request must carry the bearer token — the proxy is the door, the token is the key.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use crate::memories::Memories;
use crate::tools::Caller;

/// The one path that is served.
pub const ENDPOINT: &str = "/mcp";

/// Largest request head accepted: a few headers, not a document.
const MAX_HEAD: usize = 16 * 1024;

/// Largest body accepted. A memory is a paragraph; a megabyte is already generous.
pub const MAX_BODY: usize = 1024 * 1024;

/// How long a client may take to send its request. One slow client must not hold the door.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The header a client names itself with, for the `agent` field of what it writes.
pub const AGENT_HEADER: &str = "vibememory-agent";

/// Who a record says wrote it when the client does not introduce itself.
const DEFAULT_AGENT: &str = "http";

/// A request as far as this server cares.
#[derive(Debug, PartialEq, Eq)]
pub struct HttpRequest {
    /// `POST`, `GET`…
    pub method: String,
    /// The path without the query.
    pub path: String,
    /// Headers with lower-case names, in order.
    pub headers: Vec<(String, String)>,
    /// The body.
    pub body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An answer: status, extra headers, body.
#[derive(Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// `200`, `401`…
    pub status: u16,
    /// Headers beyond the ones every response gets.
    pub headers: Vec<(&'static str, String)>,
    /// The body, JSON or empty.
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn plain(status: u16, message: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: serde_json::json!({ "error": message })
                .to_string()
                .into_bytes(),
        }
    }
}

/// Splits a request head and body out of bytes read so far. `Ok(None)` means "not all here yet".
///
/// # Errors
///
/// The status to refuse with: a head too large, no length, a body over the limit.
pub fn parse(bytes: &[u8]) -> Result<Option<HttpRequest>, HttpResponse> {
    let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        return if bytes.len() > MAX_HEAD {
            Err(HttpResponse::plain(431, "request head too large"))
        } else {
            Ok(None)
        };
    };
    if end > MAX_HEAD {
        return Err(HttpResponse::plain(431, "request head too large"));
    }
    let head = String::from_utf8_lossy(bytes.get(..end).unwrap_or_default());
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default().to_owned();
    let target = request_line.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let find = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, v)| v.clone())
    };
    if find("transfer-encoding").is_some() {
        return Err(HttpResponse::plain(411, "send Content-Length, not chunks"));
    }
    let length = match find("content-length") {
        None => 0,
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| HttpResponse::plain(400, "Content-Length is not a number"))?,
    };
    if length > MAX_BODY {
        return Err(HttpResponse::plain(413, "request body too large"));
    }
    let body_start = end + 4;
    let Some(body) = bytes.get(body_start..body_start + length) else {
        return Ok(None);
    };
    Ok(Some(HttpRequest {
        method,
        path,
        headers,
        body: body.to_vec(),
    }))
}

/// Compares in time that does not depend on where the first difference is: a token is a secret,
/// and an early exit would tell a patient caller how much of it they already have.
fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    if given.len() != expected.len() {
        return false;
    }
    given
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

/// Answers one request.
#[must_use]
pub fn answer(request: &HttpRequest, token: &str, memories: &dyn Memories) -> HttpResponse {
    // A browser page is the one client that sends Origin, and it must not reach this: a page open
    // on the owner's machine would otherwise use their token-bearing proxy, or rebind DNS to it.
    if request.header("origin").is_some() {
        return HttpResponse::plain(403, "requests from web pages are not served");
    }
    if request.path != ENDPOINT {
        return HttpResponse::plain(404, "not found");
    }
    let presented = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if token.is_empty() || !same_secret(presented.as_bytes(), token.as_bytes()) {
        let mut refused = HttpResponse::plain(401, "a bearer token is required");
        refused
            .headers
            .push(("WWW-Authenticate", "Bearer realm=\"vibememory\"".to_owned()));
        return refused;
    }
    if request.method != "POST" {
        // No server-sent events and no sessions: there is nothing to GET and nothing to DELETE.
        let mut refused = HttpResponse::plain(405, "only POST is served");
        refused.headers.push(("Allow", "POST".to_owned()));
        return refused;
    }
    let text = String::from_utf8_lossy(&request.body);
    let caller = Caller {
        agent: request.header(AGENT_HEADER).unwrap_or(DEFAULT_AGENT),
        project: None,
    };
    let response = match crate::protocol::parse(&text) {
        Ok(message) => crate::protocol::handle(&message, &caller, memories),
        Err(detail) => Some(crate::protocol::malformed(&detail)),
    };
    match response {
        // A notification: accepted, and nothing to say.
        None => HttpResponse {
            status: 202,
            headers: Vec::new(),
            body: Vec::new(),
        },
        Some(response) => HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: serde_json::to_vec(&response).unwrap_or_default(),
        },
    }
}

/// The bytes of a response, closing the connection after it.
#[must_use]
pub fn encode(response: &HttpResponse) -> Vec<u8> {
    let reason = match response.status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    if !response.body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    for (name, value) in &response.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    bytes.extend_from_slice(&response.body);
    bytes
}

/// Who asked, for the journal line fail2ban reads. Behind the proxy every connection comes from
/// `127.0.0.1`, so the address is the last one the proxy appended to `X-Forwarded-For`: the
/// entries before it were written by the client and prove nothing. Without the header — a
/// connection straight to the port — the peer itself.
#[must_use]
pub fn client_address(request: Option<&HttpRequest>, peer: &str) -> String {
    request
        .and_then(|request| request.header("x-forwarded-for"))
        .and_then(|forwarded| forwarded.rsplit(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or(peer)
        .to_owned()
}

/// Serves one connection: read the request within the timeout, answer, close.
fn serve_connection(mut stream: TcpStream, token: &str, memories: &dyn Memories) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let response = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(chunk.get(..read).unwrap_or_default()),
        }
        match parse(&buffer) {
            Ok(None) => {}
            Ok(Some(request)) => {
                let response = answer(&request, token, memories);
                break (response, Some(request));
            }
            Err(refused) => break (refused, None),
        }
    };
    let (response, request) = response;
    if response.status == 401 {
        // Where fail2ban or a person can see it. Never the token that was presented.
        let peer = stream
            .peer_addr()
            .map_or_else(|_| "unknown".to_owned(), |address| address.ip().to_string());
        eprintln!(
            "vibememory-mcp: refused a request without the right token from {}",
            client_address(request.as_ref(), &peer)
        );
    }
    let _ = stream.write_all(&encode(&response));
}

/// Serves connections one after another until the listener fails.
///
/// One at a time on purpose: every write is a commit on the store, and a personal memory server
/// has no traffic that would justify interleaving them.
pub fn serve(listener: &TcpListener, token: &str, memories: &dyn Memories) {
    for stream in listener.incoming().flatten() {
        serve_connection(stream, token, memories);
    }
}
