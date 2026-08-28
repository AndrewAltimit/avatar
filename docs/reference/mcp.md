# `avatar-mcp` / `avatar mcp serve` — Model Context Protocol server

`avatar mcp serve` exposes the toolchain's **read/diagnose** surface to an agent host over the Model
Context Protocol (MCP), so an agent can discover and call capabilities as typed tools rather than
shelling out to the CLI and parsing stdout. The protocol plumbing is the domain-agnostic
[`avatar-mcp`](../../crates/mcp/README.md) crate; the avatar tool registry is the cli's
`crates/cli/src/cmd/mcp.rs`.

## Running

```sh
avatar mcp serve        # speaks newline-delimited JSON-RPC 2.0 on stdin/stdout
```

It is a long-lived stdio server: it reads one JSON-RPC message per line from stdin and writes one
response per line to stdout until stdin reaches EOF. **All diagnostics go to stderr** — stdout is the
protocol channel. Configure it in an MCP host as a stdio server with command `avatar` and args
`["mcp", "serve"]`.

## Handshake

Standard MCP: `initialize` → (`notifications/initialized`) → `tools/list` → `tools/call`. The server
echoes the client's requested `protocolVersion` (default `2024-11-05`), advertises a `tools`
capability, and answers `ping` with an empty result.

## Tools

All tools are **non-writing** — nothing here touches the filesystem for output. The diagnose tools
take a single `path` string and return the same JSON report the corresponding `--json` CLI flag
emits; the generation tools (`avatar_gen_*`) return the generated YAML (and `.meta` sidecars) as
*text inside the result* — the agent host decides what, if anything, lands on disk.

| Tool | Input | Returns |
|------|-------|---------|
| `avatar_describe` | `path` (FBX **or** project) | Consolidated `DescribeReport` — best first call |
| `avatar_lint` | `path` (project) | `LintReport` (VRC001–VRC052 diagnostics) |
| `avatar_stats` | `path` (FBX **or** project) | `PerfReport` (FBX) or `PerfReport[]` (project avatars) |
| `avatar_armature_check` | `path` (FBX) | `ArmatureReport` (humanoid-bone mapping + flags) |
| `avatar_fbx_inspect` | `path` (FBX) | `InspectSummary` (structure counts + unit/orientation flags) |
| `avatar_physbone_list` | `path` (`.prefab`) | `PhysBoneInfo[]` — every VRCPhysBone's root, chains, colliders, tuning + curves ([`physbone.md`](physbone.md)) |
| `avatar_unitypackage_info` | `path` (`.unitypackage`) | Package summary (counts, SDK, avatar/world traits) |
| `avatar_schema` | `name?` | JSON Schema for a report type (omit `name` to list; `all` for every) — only under the `schema` feature |
| `avatar_gen_clip` | `name`, `blendshapes[]?`, `toggles[]?` | A `.anim` AnimationClip as YAML text (+ suggested file name, fileID) |
| `avatar_gen_controller` | `clips[]`, `name?`, `layer?`, `parameter?` | A complete FX `.controller` (analog-gesture blend tree) as YAML text |
| `avatar_gen_params` | `params[]`, `name?` | A `VRCExpressionParameters` `.asset` as YAML text + its sync-bit cost |
| `avatar_gen_menu` | `toggles[]?`, `buttons[]?`, `radials[]?`, `submenus[]?`, `name?` | A `VRCExpressionsMenu` `.asset` as YAML text (≤ 8 controls enforced) |
| `avatar_gen_toggle` | `name`, `toggles[]?`, `blendshapes[]?`, `parameter?`, `menu_label?`, `default_on?`, `saved?` | The full ten-file toggle bundle (every file name + content, pinned guids, wiring note) |

The output shapes are published as JSON Schemas (`avatar_schema` / `avatar schema`), so a consumer can
introspect the contract instead of inferring it.

### Why non-writing

Every MCP tool can be called freely with no risk of mutating assets: the diagnose tools only read,
and the generation tools run the same generators as `avatar anim-gen …` / `avatar toggle` but hand
the output back as text instead of writing it. Spec-string formats (`PATH:SHAPE:VALUE`,
`GUID@THRESHOLD`, `NAME:TYPE[:DEFAULT][:unsaved][:local]`, `LABEL:PARAM[:VALUE]`) match the CLI
flags exactly. Actual disk writes stay on the explicit CLI behind the `WriteGuard`
(`--dry-run`/`--force`), as do the repairs (`armature fix`).

## Errors are two-layered

- A **protocol** error (malformed JSON, unknown method) comes back as a JSON-RPC `error` object.
- A **tool** failure (a bad path, a parse error, an unknown tool name) comes back as a *successful*
  `tools/call` result with `isError: true` and the error text as content — because that text is for
  the model to read and act on. Handlers attach actionable context up front (e.g. `path does not
  exist: … — paths are resolved relative to the server's working directory`; `expected a single FBX
  file but … is a directory — for a Unity project use avatar_lint`), and the server renders the full
  context chain, so the agent gets guidance it can recover from rather than a deep parser error.
