//! MCP over HTTP (Streamable HTTP, revision 2025-06-18), for agents that cannot start a process or
//! reach the host over ssh.
//!
//! Deliberately small: one JSON-RPC message per `POST /mcp`, answered with one JSON body; no
//! server-sent events, no sessions. It listens on a loopback port behind a TLS proxy, and every
//! request must carry a bearer token — the proxy is the door, the token is the key, and the
//! [`Door`] says whose key it is and which memory it opens.

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

use serde_json::Value;

use crate::memories::Memories;
use crate::tools::{Caller, Limits, Writes};

/// The one path that is served.
pub const ENDPOINT: &str = "/mcp";

/// Largest request head accepted: a few headers, not a document.
const MAX_HEAD: usize = 16 * 1024;

/// Largest body accepted. A memory is a paragraph; a megabyte is already generous.
pub const MAX_BODY: usize = 1024 * 1024;

/// How long a client may take to send its request. One slow client must not hold the door.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Connections served at once when `--connections` does not say. Each is a thread that holds a
/// connection only until its one request is answered.
pub const DEFAULT_CONNECTIONS: usize = 8;

/// What the HTTP layer needs from the host: whose a bearer token is, and the memory it opens.
pub trait Door: Sync {
    /// Who presented `token` — empty when the request carried none.
    fn admit(&self, token: &str) -> Admission<'_>;
}

/// Who a request is, as far as the door can tell.
pub enum Admission<'a> {
    /// Nobody gets in: the rights cannot be read. The host says why in its own journal line.
    Closed,
    /// No token matches: unknown, revoked, or none presented.
    Unknown,
    /// A token that matched and is past its time, by its public id.
    Expired(String),
    /// Somebody the rights let in.
    Granted(Visit<'a>),
}

/// An admitted request: who it is, and the memory it may use.
pub struct Visit<'a> {
    /// Who, for what the request writes and for the journal.
    pub grant: Grant,
    /// The memory the grant opens.
    pub memories: Box<dyn Memories + 'a>,
}

/// Who an admitted request is. Owned: it lives exactly as long as the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    /// The public id of what opened the door — a token `tk_…` over HTTPS, a machine key `mk_…`
    /// over ssh: it names the request in the journal.
    pub token: String,
    /// The team's slug.
    pub team: String,
    /// The member's handle.
    pub member: String,
    /// The agent the token was issued for.
    pub agent: String,
    /// Whether it may write.
    pub writes: Writes,
    /// Whether it may search past sessions.
    pub history: bool,
    /// The projects the token opens; `None` is all of the team's.
    pub scope: Option<Vec<String>>,
    /// The team's limits.
    pub limits: Limits,
    /// Where the cabinet is, for refusals only it can lift.
    pub cabinet: Option<String>,
}

impl Grant {
    /// The caller the tools see. Over HTTP nothing comes from the client itself: not the agent,
    /// which the token names, and not a default project, because there is no directory.
    #[must_use]
    pub fn caller(&self) -> Caller<'_> {
        Caller {
            agent: &self.agent,
            member: Some(&self.member),
            project: None,
            writes: self.writes,
            history: self.history,
            scope: self.scope.as_deref(),
            limits: self.limits,
            cabinet: self.cabinet.as_deref(),
        }
    }
}

/// What a request leaves in the journal. Some of these lines are read by fail2ban, so their text
/// is part of the contract with the jail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// Nothing worth a line.
    Quiet,
    /// A request without the right token: the line the jail counts.
    Refused,
    /// A token that was right once, by its public id: a known client is not an attack, and the
    /// jail does not count this line.
    Expired(String),
    /// A tool was called. Who, which tool, where, and whether it worked — never the token's
    /// secret and never what was written.
    Call {
        /// The token's public id.
        token: String,
        /// The team's slug.
        team: String,
        /// The member's handle.
        member: String,
        /// The agent the token names.
        agent: String,
        /// The tool.
        tool: String,
        /// The project the call named, when it named one.
        project: Option<String>,
        /// Whether the tool answered rather than refused.
        ok: bool,
    },
}

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

    /// The one answer for every token that does not open the door: unknown, revoked and expired
    /// alike, so the answer tells nothing about which it was.
    fn unauthorized() -> Self {
        let mut refused = Self::plain(401, "a bearer token is required");
        refused
            .headers
            .push(("WWW-Authenticate", "Bearer realm=\"vibememory\"".to_owned()));
        refused
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

/// Answers one request, and says what it leaves in the journal.
#[must_use]
pub fn answer(request: &HttpRequest, door: &dyn Door) -> (HttpResponse, Note) {
    // A browser page is the one client that sends Origin, and it must not reach this: a page open
    // on a member's machine would otherwise use their token-bearing proxy, or rebind DNS to it.
    if request.header("origin").is_some() {
        return (
            HttpResponse::plain(403, "requests from web pages are not served"),
            Note::Quiet,
        );
    }
    if request.path != ENDPOINT {
        return (HttpResponse::plain(404, "not found"), Note::Quiet);
    }
    let presented = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let visit = match door.admit(presented) {
        Admission::Closed => {
            return (
                HttpResponse::plain(
                    503,
                    "the server cannot read who may use it; nobody is let in until it can",
                ),
                Note::Quiet,
            );
        }
        Admission::Unknown => return (HttpResponse::unauthorized(), Note::Refused),
        Admission::Expired(token) => return (HttpResponse::unauthorized(), Note::Expired(token)),
        Admission::Granted(visit) => visit,
    };
    if request.method != "POST" {
        // No server-sent events and no sessions: there is nothing to GET and nothing to DELETE.
        let mut refused = HttpResponse::plain(405, "only POST is served");
        refused.headers.push(("Allow", "POST".to_owned()));
        return (refused, Note::Quiet);
    }
    let text = String::from_utf8_lossy(&request.body);
    let caller = visit.grant.caller();
    let (response, note) = match crate::protocol::parse(&text) {
        Ok(message) => {
            let response = crate::protocol::handle(&message, &caller, &*visit.memories);
            let note = call_note(&message, response.as_ref(), &visit.grant);
            (response, note)
        }
        Err(detail) => (Some(crate::protocol::malformed(&detail)), Note::Quiet),
    };
    let response = match response {
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
    };
    (response, note)
}

/// The journal line of a tool call; nothing for the handshake and the catalogue.
#[must_use]
pub fn call_note(
    message: &crate::protocol::Request,
    response: Option<&crate::protocol::Response>,
    grant: &Grant,
) -> Note {
    if message.method != "tools/call" {
        return Note::Quiet;
    }
    let tool = message
        .params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let project = message
        .params
        .get("arguments")
        .and_then(|arguments| arguments.get("project"))
        .and_then(Value::as_str);
    Note::Call {
        token: grant.token.clone(),
        team: grant.team.clone(),
        member: grant.member.clone(),
        agent: grant.agent.clone(),
        tool: tool.to_owned(),
        project: project.map(str::to_owned),
        ok: response.is_some_and(|response| !response.is_refusal()),
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
        503 => "Service Unavailable",
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

/// The journal line a note becomes, if any. Where fail2ban or a person can see it; never a token.
#[must_use]
pub fn journal_line(note: &Note, address: &str) -> Option<String> {
    match note {
        Note::Quiet => None,
        Note::Refused => Some(format!(
            "vibememory-mcp: refused a request without the right token from {address}"
        )),
        Note::Expired(token) => Some(format!(
            "vibememory-mcp: token {token} has expired; its request from {address} was turned away"
        )),
        Note::Call {
            token,
            team,
            member,
            agent,
            tool,
            project,
            ok,
        } => Some(format!(
            "vibememory-mcp: {tool} in {team}/{} by {member} ({agent}, {token}): {}",
            project.as_deref().unwrap_or("-"),
            if *ok { "done" } else { "refused" }
        )),
    }
}

/// Serves one connection: read the request within the timeout, answer, close.
fn serve_connection(mut stream: TcpStream, door: &dyn Door) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let (response, note, request) = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => buffer.extend_from_slice(chunk.get(..read).unwrap_or_default()),
        }
        match parse(&buffer) {
            Ok(None) => {}
            Ok(Some(request)) => {
                let (response, note) = answer(&request, door);
                break (response, note, Some(request));
            }
            Err(refused) => break (refused, Note::Quiet, None),
        }
    };
    let peer = stream
        .peer_addr()
        .map_or_else(|_| "unknown".to_owned(), |address| address.ip().to_string());
    if let Some(line) = journal_line(&note, &client_address(request.as_ref(), &peer)) {
        eprintln!("{line}");
    }
    let _ = stream.write_all(&encode(&response));
}

/// How many connections may be served at once.
struct Gate {
    free: Mutex<usize>,
    freed: Condvar,
}

/// One place at the gate, given back when dropped — also when the connection's thread panics.
struct Permit<'a> {
    gate: &'a Gate,
}

impl Gate {
    fn new(places: usize) -> Self {
        Self {
            free: Mutex::new(places),
            freed: Condvar::new(),
        }
    }

    /// Waits for a place. A counter under a lock holds no state a panic could leave half-done.
    fn enter(&self) -> Permit<'_> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        while *free == 0 {
            free = self
                .freed
                .wait(free)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *free -= 1;
        Permit { gate: self }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut free = self
            .gate
            .free
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *free += 1;
        self.gate.freed.notify_one();
    }
}

/// Serves connections until the listener fails, each on its own thread, at most `connections` at
/// once; the next one waits in the listener's queue.
///
/// Threads, because one client that connects and sends nothing would otherwise hold every team's
/// door for the whole read timeout. A thread that panics loses its own connection and gives its
/// place back; the loop goes on.
pub fn serve(listener: &TcpListener, door: &dyn Door, connections: usize) {
    let gate = Gate::new(connections.max(1));
    std::thread::scope(|scope| {
        for stream in listener.incoming().flatten() {
            let permit = gate.enter();
            scope.spawn(move || {
                let _permit = permit;
                serve_connection(stream, door);
            });
        }
    });
}
