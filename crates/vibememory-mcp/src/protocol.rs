//! JSON-RPC over stdio, which is all MCP is on the wire.
//!
//! Written by hand rather than taken from a library: the surface is three methods, and the
//! project's rule is zero runtime dependencies on the target machine. Answering is pure — a
//! request and a [`Memories`] go in, a response comes out — and [`serve_lines`] runs it over any
//! pair of streams, so the whole protocol is tested without a store or a pipe.

use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::memories::Memories;

/// The version of MCP this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// What the agent sent.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Absent on a notification, and a notification is never answered.
    #[serde(default)]
    pub id: Option<Value>,
    /// The method name.
    pub method: String,
    /// Its arguments.
    #[serde(default)]
    pub params: Value,
}

/// What the server sends back.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

impl Response {
    fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn failed(id: Value, code: i64, message: &str) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(json!({ "code": code, "message": message })),
        }
    }

    /// Whether this answers with a refusal: a protocol error, or a tool that said no.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        self.error.is_some()
            || self
                .result
                .as_ref()
                .and_then(|result| result.get("isError"))
                .and_then(Value::as_bool)
                == Some(true)
    }
}

/// Answers one request, or returns `None` when there is nothing to answer.
///
/// `caller.agent` is the name written into every record this call creates — memory is shared between
/// agents, so it has to say who wrote it, and "claude-code" for everyone would make that question
/// unanswerable.
#[must_use]
pub fn handle(
    request: &Request,
    caller: &crate::tools::Caller<'_>,
    memories: &dyn Memories,
) -> Option<Response> {
    // A notification carries no id and gets no answer — replying to one is a protocol error, and
    // `notifications/initialized` is sent by every client right after the handshake.
    let id = request.id.clone()?;
    Some(match request.method.as_str() {
        "initialize" => {
            let mut result = json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "vibememory", "version": env!("CARGO_PKG_VERSION") },
            });
            // The agent learns the name the store gave its folder: without it a model asked to
            // look up this project's memory has no way to know what to pass as `project`.
            let mut instructions = String::new();
            if let Some(project) = caller.project {
                instructions = format!(
                    "Started in the store project {project}: memory_save, memory_update and \
                     memory_delete use it when `project` is omitted, and memory_search and \
                     history_search take it as `project` to stay inside this project. "
                );
            }
            // every agent learns where a rule goes when a person asks for one: the level is theirs to name
            let _ = write!(
                instructions,
                "Rules and skills: rules_get gives the rules in force when your client does not load rule \
                 files itself; rule_save and skill_save write one at the level the person named. {}",
                crate::rule_tools::LEVELS
            );
            if let Some(fields) = result.as_object_mut() {
                fields.insert("instructions".to_owned(), json!(instructions));
            }
            Response::ok(id, result)
        }
        "tools/list" => Response::ok(id, json!({ "tools": crate::tools::catalogue() })),
        "tools/call" => {
            let name = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            match crate::tools::call(name, &arguments, caller, memories) {
                // A tool that refused is not a protocol failure: the agent asked something
                // sensible and the answer is "no, because…". Reporting it as a JSON-RPC error
                // would hide the reason from the model, which is the one that has to act on it.
                Ok(value) => Response::ok(id, tool_content(&value, false)),
                Err(problem) => Response::ok(id, tool_content(&json!({ "error": problem }), true)),
            }
        }
        "ping" => Response::ok(id, json!({})),
        other => Response::failed(id, -32601, &format!("unknown method: {other}")),
    })
}

/// The shape `tools/call` answers in: text content plus the machine-readable copy.
fn tool_content(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
        "structuredContent": value,
    })
}

/// Reads one line as a request. A line that is not a request is reported, never guessed at.
///
/// # Errors
///
/// What serde said, for the caller to answer with.
pub fn parse(line: &str) -> Result<Request, String> {
    serde_json::from_str(line).map_err(|error| error.to_string())
}

/// The error to send when a line could not be read at all.
#[must_use]
pub fn malformed(detail: &str) -> Response {
    Response::failed(Value::Null, -32700, detail)
}

/// The JSON-RPC error of a request the session's rights no longer allow: the session ends with it.
const RIGHTS_ENDED: i64 = -32001;

/// Serves MCP over a stream of lines until the input closes: one request per line, one answer per
/// line, nothing else on `output` — it *is* the protocol. `after` sees every request with its
/// answer; the host journals tool calls with it.
///
/// # Errors
///
/// Input that cannot be read. A client that goes away mid-answer is not an error of the server.
pub fn serve_lines(
    input: impl std::io::BufRead,
    output: impl std::io::Write,
    caller: &crate::tools::Caller<'_>,
    memories: &dyn Memories,
    after: &mut dyn FnMut(&Request, Option<&Response>),
) -> Result<(), String> {
    serve_checked_lines(
        input,
        output,
        &mut || Ok(()),
        &|(), request| handle(request, caller, memories),
        after,
    )
}

/// Serves MCP over a stream of lines like [`serve_lines`], with the session's rights taken anew
/// before every request: a session that lasts days must not outlive a revocation. `rights` answers
/// what the session may do now, or why it may do nothing any more — then that request is refused
/// with the reason and the session ends; the client's next connection meets the refusal of the
/// door.
///
/// # Errors
///
/// Input that cannot be read.
pub fn serve_checked_lines<Rights>(
    input: impl std::io::BufRead,
    mut output: impl std::io::Write,
    rights: &mut dyn FnMut() -> Result<Rights, String>,
    serve: &dyn Fn(&Rights, &Request) -> Option<Response>,
    after: &mut dyn FnMut(&Request, Option<&Response>),
) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|error| error.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let mut ended = false;
        let response = match parse(&line) {
            Ok(request) => {
                let response = match rights() {
                    Ok(now) => serve(&now, &request),
                    Err(why) => {
                        ended = true;
                        request
                            .id
                            .clone()
                            .map(|id| Response::failed(id, RIGHTS_ENDED, &why))
                    }
                };
                after(&request, response.as_ref());
                response
            }
            Err(detail) => Some(malformed(&detail)),
        };
        if ended {
            if let Some(response) = response
                && let Ok(text) = serde_json::to_string(&response)
            {
                let _ = writeln!(output, "{text}").and_then(|()| output.flush());
            }
            return Ok(());
        }
        let Some(response) = response else {
            continue; // a notification: answered by saying nothing
        };
        let Ok(text) = serde_json::to_string(&response) else {
            eprintln!("vibememory-mcp: a response could not be encoded");
            continue;
        };
        if writeln!(output, "{text}").is_err() || output.flush().is_err() {
            return Ok(());
        }
    }
    Ok(())
}
