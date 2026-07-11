# avatar-mcp

A tiny, **domain-agnostic** Model Context Protocol (MCP) server over stdio. This crate is the
protocol half only — it knows nothing about avatars. The avatar tools are wired in by the cli
(`avatar mcp serve`), mirroring how [`avatar-osc`](../osc/README.md) keeps a pure `codec` apart from
its thin transport.

## Purpose

Let an agent host **discover** (`tools/list`) and **call** (`tools/call`) this binary's capabilities
as typed JSON tools, instead of spawning a subprocess per question and parsing free-form stdout. The
input/output shapes are the same JSON the `--json` CLI flags already emit (introspectable via the
`avatar_schema` tool / `avatar schema`).

## Key API

- `Server::new(name, version).tool(Tool::new(name, description, input_schema, handler))` — build a
  registry. `handler: Box<dyn Fn(&Value) -> anyhow::Result<String> + Send + Sync>` returns the tool's
  text result, or an `Err` whose full `{:#}` context chain is shown to the model.
- `Server::handle(&Value) -> Option<Value>` — the **pure** JSON-RPC dispatch core (no I/O):
  `Some(response)` for a request, `None` for a notification. Exhaustively unit-tested.
- `Server::serve_stdio()` — the thin loop: read newline-delimited JSON-RPC from stdin, write
  responses to stdout (diagnostics go to **stderr**, which is not the protocol channel).

## Protocol

JSON-RPC 2.0 over newline-delimited stdio. Implements `initialize` (echoes the client's
`protocolVersion`, advertises `tools`), `tools/list`, `tools/call`, and `ping`; accepts and ignores
`notifications/*`.

**Two error layers, deliberately:** a *protocol* error (malformed JSON → `-32700`, unknown method →
`-32601`) returns a JSON-RPC `error`. A *tool* failure (handler `Err`, or an unknown tool name)
returns a **successful** `tools/call` result with `isError: true` and the error text as content —
because that text is for the model to read and recover from, not for the transport to reject. This is
where a handler's actionable context (which path was wrong, what was expected) reaches the agent.

## Status

Built. Pure-dispatch core covered by unit tests; the avatar tool registry (read/diagnose +
text-returning `avatar_gen_*` generation tools) and a stdio handshake smoke test live in the cli
(`crates/cli/src/cmd/mcp.rs`). Deps: `anyhow` + `serde` + `serde_json`
only — no async runtime.
