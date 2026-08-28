# avatar-osc

VRChat OSC **avatar-parameter** runtime layer. Package `avatar-osc` · lib `avatar_osc`. Part of the
[avatar](../../README.md) monorepo. Milestone M5.

## What it does

Speaks VRChat's OSC *parameter* protocol — the address spaces a running VRChat client sends and
accepts — as a renderer-agnostic, pure-Rust library (just `rosc` + UDP):

- **`capture` module** — record the broadcast parameter stream, reduce to per-parameter
  counts/ranges + the gesture/weight cross-tab (`avatar osc capture`; also the standalone
  `avatar-gesture-capture` bin, cross-compilable to a Windows `.exe` for on-rig sessions).
- **`oscquery` module** — minimal OSCQuery *advertisement* (hand-rolled mDNS responder +
  `?HOST_INFO`/tree HTTP): modern VRChat discovers the service and routes its parameter output
  to the advertised port, so capture works without fixed-port assumptions.
- **`/avatar/parameters/<Name>`** — typed avatar parameters (`bool`/`int`/`float`), modeled by
  `ParamMessage` + `ParamValue`.
- **`/avatar/change`** — the avatar-switch event (blueprint id + config path), `AvatarChange`.
- **`/input/<Axis|Button>`** — the input controller: `InputAxis` (float `-1..=1`) and `InputButton`
  (momentary, reset-to-zero) enums covering VRChat's canonical input names, via `InputMessage`.

This is **not** the VMC tracker protocol (`/tracking/*` transforms) — that's `avatar_input::osc`,
which drives the local rig. Parameters drive a VRChat avatar.

## Key API

- `codec` — **pure** wire format. `ParamMessage` / `AvatarChange` / `InputMessage` each
  `to_osc()` / `from_osc()` against `rosc::OscMessage`, with no I/O. Fully unit-tested round-trips.
- `query::AvatarConfig` — parses an avatar's OSCQuery config JSON
  (`…/OSC/<user>/Avatars/<avtr>.json`) into a flat parameter list with OSC type tags and read/write
  `Access`, **offline** (`from_json` / `from_path`). Lets a daemon know an avatar's schema without
  probing the live HTTP endpoint.
- `ParamClient` — thin non-blocking UDP transport over the codec. `connect_default()` (recv 9001,
  send 9000); `send_param` / `send_axis` / `send_button` / `send_avatar_change`; `poll()` drains
  queued datagrams and returns the `/avatar/parameters/*` updates. The codec never touches a socket.

```rust
use avatar_osc::{ParamClient, ParamValue, InputButton};
let mut client = ParamClient::connect_default()?;
client.send_param("VRCEmote", ParamValue::Int(3))?;
client.send_button(InputButton::Jump, true)?;   // then send `false` to release
for update in client.poll()? { /* update.name, update.value */ }
# anyhow::Ok(())
```

## Status

Built and green: the codec, OSCQuery config parser, and `ParamClient` transport, with round-trip
unit tests for every parameter type and an embedded-JSON OSCQuery parse test. The analog-gesture
daemon that drives this (`avatar-osc-gestures`, PLAN §4) is the next M5 piece. Behaviour:
[`docs/reference/osc-runtime.md`](../../docs/reference/osc-runtime.md).
