//! `avatar-mcp` — a tiny, domain-agnostic Model Context Protocol (MCP) server over stdio.
//!
//! MCP lets an agent host discover and call tools by name with typed JSON arguments, instead of
//! shelling out to a CLI and parsing free-form stdout. This crate is the **protocol half only** — it
//! knows nothing about avatars. It speaks JSON-RPC 2.0 over newline-delimited stdio, implements the
//! MCP `initialize` / `tools/list` / `tools/call` / `ping` handshake, and dispatches `tools/call` to
//! a registry of [`Tool`]s the caller supplies. The avatar tools themselves are wired in by the cli
//! (`avatar mcp serve`), mirroring the way [`avatar-osc`](../avatar_osc/index.html) keeps a pure
//! `codec` apart from its thin UDP transport.
//!
//! The dispatch core — [`Server::handle`] — is a pure `&Value -> Option<Value>` function with no I/O,
//! so it is exhaustively unit-testable; [`Server::serve_stdio`] is the thin loop that reads stdin
//! lines, feeds each to `handle`, and writes the response to stdout. Diagnostics must go to **stderr**
//! (stdout is the protocol channel).
//!
//! ## Errors are two-layered, on purpose
//!
//! A *protocol* error (malformed JSON, unknown method) comes back as a JSON-RPC `error` object. A
//! *tool* failure (a handler returning `Err`) instead comes back as a successful `tools/call` result
//! with `isError: true` and the error text as content — because that text is meant for the **model**
//! to read and act on, not for the transport to choke on. Handlers return [`anyhow::Result`], and the
//! server renders the failure with the full `{:#}` context chain, so a handler that attaches
//! actionable context (`"describing foo.fbx: path does not exist: …"`) surfaces all of it to the agent.

use std::io::{BufRead, Write};

use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The MCP protocol revision this server implements and advertises by default. When a client sends a
/// different `protocolVersion` in `initialize`, we echo theirs back (both revisions in the wild are
/// wire-compatible for the tools subset we implement), maximizing client compatibility.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

// JSON-RPC 2.0 reserved error codes (see https://www.jsonrpc.org/specification#error_object).
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// A tool handler: given the call's `arguments` object, produce the textual result, or an error whose
/// context chain is shown to the model. Must be `Send + Sync` so a `Server` can be shared.
pub type ToolFn = Box<dyn Fn(&Value) -> Result<String> + Send + Sync>;

/// One callable tool: a name, a human/agent-readable description, a JSON Schema for its arguments
/// (published verbatim in `tools/list` so the host can validate calls), and the handler.
pub struct Tool {
    /// Unique tool name the host calls by (`tools/call` `name`).
    pub name: String,
    /// What the tool does — shown to the agent when it chooses a tool.
    pub description: String,
    /// JSON Schema for the `arguments` object.
    pub input_schema: Value,
    handler: ToolFn,
}

impl Tool {
    /// Define a tool. `input_schema` should be a JSON Schema `object` describing the arguments.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        handler: ToolFn,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            handler,
        }
    }
}

/// An MCP server: server identity plus a registry of [`Tool`]s. Build it with [`Server::new`] +
/// [`Server::tool`], unit-test it through [`Server::handle`], and run it with [`Server::serve_stdio`].
pub struct Server {
    name: String,
    version: String,
    tools: Vec<Tool>,
}

impl Server {
    /// Create a server advertising the given name/version in its `initialize` reply.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            tools: Vec::new(),
        }
    }

    /// Register a tool (builder style).
    pub fn tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }

    /// The registered tools, in registration order.
    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Dispatch one parsed JSON-RPC message. Returns `Some(response)` for a request (a message with
    /// an `id`) and `None` for a notification (no `id` — these get no reply, per JSON-RPC). This is
    /// pure: no I/O, no global state. [`serve_stdio`](Self::serve_stdio) is the only caller that does
    /// I/O, so every dispatch path is testable by passing a `Value` and inspecting the result.
    pub fn handle(&self, msg: &Value) -> Option<Value> {
        // Absent `id` ⇒ notification ⇒ no response is ever sent (even on error).
        let id = msg.get("id").cloned();

        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            return id.map(|id| error(id, INVALID_REQUEST, "request is missing a string `method`"));
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => id.map(|id| success(id, self.initialize_result(&params))),
            "tools/list" => id.map(|id| success(id, self.tools_list_result())),
            "tools/call" => id.map(|id| self.tools_call_result(id, &params)),
            // Liveness ping (MCP/JSON-RPC): empty result.
            "ping" => id.map(|id| success(id, json!({}))),
            // Client→server notifications (`notifications/initialized`, `.../cancelled`, …) are
            // accepted and ignored; they carry no `id` and so warrant no reply regardless.
            m if m.starts_with("notifications/") => None,
            other => id.map(|id| error(id, METHOD_NOT_FOUND, &format!("unknown method `{other}`"))),
        }
    }

    /// Serve the protocol over stdio until stdin reaches EOF. Reads newline-delimited JSON-RPC from
    /// stdin, writes newline-delimited responses to stdout (flushing each), and never writes
    /// diagnostics to stdout. A line that is not valid JSON yields a JSON-RPC parse error with a
    /// null id; a blank line is skipped.
    pub fn serve_stdio(&self) -> Result<()> {
        let stdin = std::io::stdin();
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for line in stdin.lock().lines() {
            let line = line.context("reading a line from stdin")?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(error(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("message was not valid JSON: {e}"),
                )),
            };
            if let Some(resp) = response {
                write_message(&mut out, &resp)?;
            }
        }
        Ok(())
    }

    fn initialize_result(&self, params: &Value) -> Value {
        // Echo the client's requested protocol version when it sends one (wire-compatible for the
        // tools subset), else advertise our default.
        let version = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": self.name, "version": self.version },
        })
    }

    fn tools_list_result(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect();
        json!({ "tools": tools })
    }

    fn tools_call_result(&self, id: Value, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error(id, INVALID_PARAMS, "`tools/call` requires a string `name`");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        let Some(tool) = self.tools.iter().find(|t| t.name == name) else {
            // Unknown tool is a *tool-level* error the model should see and recover from (e.g. by
            // calling `tools/list`), not a transport error.
            let known = self
                .tools
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return success(
                id,
                tool_error(&format!("unknown tool `{name}`; available tools: {known}")),
            );
        };

        match (tool.handler)(&args) {
            Ok(text) => success(
                id,
                json!({
                    "content": [ { "type": "text", "text": text } ],
                    "isError": false,
                }),
            ),
            // `{:#}` renders the whole anyhow context chain — this is where a handler's actionable
            // context (path that was wrong, what was expected) reaches the agent.
            Err(e) => success(id, tool_error(&format!("{e:#}"))),
        }
    }
}

/// A successful JSON-RPC response.
fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC *protocol* error response (malformed request / unknown method).
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A `tools/call` *result* that reports a tool failure: `isError: true` with the text the model reads.
fn tool_error(text: &str) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": true })
}

/// Write one response as a single line of JSON followed by `\n`, then flush.
fn write_message(out: &mut impl Write, msg: &Value) -> Result<()> {
    serde_json::to_writer(&mut *out, msg).context("serializing response")?;
    out.write_all(b"\n").context("writing response newline")?;
    out.flush().context("flushing stdout")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server with one tool that echoes its `text` argument and one that always fails with a
    /// context-rich error (to exercise the `isError` path and chain rendering).
    fn test_server() -> Server {
        Server::new("test", "9.9.9")
            .tool(Tool::new(
                "echo",
                "echo the text argument back",
                json!({"type": "object", "properties": {"text": {"type": "string"}}}),
                Box::new(|args| {
                    Ok(args
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string())
                }),
            ))
            .tool(Tool::new(
                "boom",
                "always fails",
                json!({"type": "object"}),
                Box::new(|_| {
                    anyhow::bail!("inner cause");
                }),
            ))
    }

    fn req(id: i64, method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    #[test]
    fn initialize_echoes_protocol_and_reports_identity() {
        let s = test_server();
        let resp = s
            .handle(&req(
                1,
                "initialize",
                json!({"protocolVersion": "2025-06-18"}),
            ))
            .expect("request gets a response");
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], "2025-06-18");
        assert_eq!(result["serverInfo"]["name"], "test");
        assert_eq!(result["serverInfo"]["version"], "9.9.9");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn initialize_defaults_protocol_when_absent() {
        let s = test_server();
        let resp = s.handle(&req(1, "initialize", json!({}))).unwrap();
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn tools_list_publishes_names_and_schemas() {
        let s = test_server();
        let resp = s.handle(&req(2, "tools/list", json!({}))).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["name"], "echo");
        assert_eq!(tools[0]["inputSchema"]["type"], "object");
    }

    #[test]
    fn tools_call_success_returns_text_content() {
        let s = test_server();
        let resp = s
            .handle(&req(
                3,
                "tools/call",
                json!({"name": "echo", "arguments": {"text": "hi"}}),
            ))
            .unwrap();
        let result = &resp["result"];
        assert_eq!(result["isError"], false);
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "hi");
    }

    #[test]
    fn tools_call_failure_is_iserror_with_chain() {
        let s = test_server();
        let resp = s
            .handle(&req(4, "tools/call", json!({"name": "boom"})))
            .unwrap();
        let result = &resp["result"];
        // A tool failure is a *successful* JSON-RPC response carrying isError, not a protocol error.
        assert!(resp.get("error").is_none());
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("inner cause"), "got: {text}");
    }

    #[test]
    fn unknown_tool_is_iserror_not_protocol_error() {
        let s = test_server();
        let resp = s
            .handle(&req(5, "tools/call", json!({"name": "nope"})))
            .unwrap();
        assert!(resp.get("error").is_none());
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("echo") && text.contains("boom"),
            "lists known tools: {text}"
        );
    }

    #[test]
    fn unknown_method_is_protocol_error() {
        let s = test_server();
        let resp = s.handle(&req(6, "frobnicate", json!({}))).unwrap();
        assert_eq!(resp["error"]["code"], METHOD_NOT_FOUND);
    }

    #[test]
    fn notification_gets_no_response() {
        let s = test_server();
        let msg = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        assert!(s.handle(&msg).is_none());
    }

    #[test]
    fn ping_returns_empty_result() {
        let s = test_server();
        let resp = s.handle(&req(7, "ping", json!({}))).unwrap();
        assert!(resp["result"].is_object());
        assert_eq!(resp["result"].as_object().unwrap().len(), 0);
    }

    #[test]
    fn missing_method_on_request_is_invalid_request() {
        let s = test_server();
        let resp = s.handle(&json!({"jsonrpc": "2.0", "id": 8})).unwrap();
        assert_eq!(resp["error"]["code"], INVALID_REQUEST);
    }
}
