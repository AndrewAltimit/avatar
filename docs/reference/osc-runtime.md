# OSC runtime (avatar parameters · input · OSCQuery config)

The OSC runtime layer (`avatar-osc`) lets the tools **drive and observe a running VRChat avatar** at
render time over OSC — set avatar parameters, push input-controller axes/buttons, switch avatars, and
read parameter updates back — as a renderer-agnostic, pure-Rust library (only `rosc` and `std`'s UDP
socket; no system deps). It is the foundation under the analog-gesture daemon (`avatar-osc-gestures`,
PLAN §4): the "Vive advanced controls on any hardware" feature reads controller analog inputs and
sends `/avatar/parameters/<Gesture>Weight`.

This is a **different address space** from the rig layer's tracker input. `avatar_input::osc` carries
raw `/tracking/*` transforms that pose a *local* rig (rig-runtime.md); `avatar-osc` carries VRChat's
`/avatar/parameters/*`, `/avatar/change`, and `/input/*` that drive a *VRChat* avatar. They never
mix.

## Shape

The crate splits the same way the tracker backend does — a **pure codec** with no I/O, and a thin
transport that wraps it:

| Module | Role |
|--------|------|
| `codec` | Pure model of the wire format: `ParamMessage` / `AvatarChange` / `InputMessage` encode to / decode from `rosc::OscMessage`. No socket — unit-tested in isolation. |
| `query` | `AvatarConfig`: parses VRChat's per-avatar OSCQuery config JSON into a typed parameter schema, offline. |
| `lib` | `ParamClient`: a non-blocking UDP transport over the codec (send to VRChat, poll updates back). The only part that owns a socket. |

## The address space

### Avatar parameters — `/avatar/parameters/<Name>`

Each avatar parameter is one OSC message: the address `/avatar/parameters/<Name>` with a single typed
argument. VRChat exposes exactly three scalar types, matching the Avatars-3.0 parameter types:

| Type | OSC arg | `ParamValue` | tag |
|------|---------|--------------|-----|
| bool | `T`/`F` (or `i` `0`/`1`) | `Bool(bool)` | `b` |
| int  | `i` (`0..=255`) | `Int(i32)` | `i` |
| float| `f` (`-1.0..=1.0`) | `Float(f32)` | `f` |

`ParamMessage { name, value }` is the unit both directions speak in. `to_osc()` builds the message;
`from_osc()` returns `Ok(None)` for any non-parameter address (so a caller can fall through to other
parsers) and errors only when the address *is* a parameter address but the payload is malformed
(empty name, missing/unsupported argument). Decode is lenient on the bool-vs-int and float/double
ambiguity some senders introduce, since VRChat itself is.

### Avatar change — `/avatar/change`

`AvatarChange { id, config_path }` models the avatar-switch event VRChat broadcasts (blueprint id +
the on-disk path of the avatar's OSC config), and which we can also *send* to ask VRChat to load a
specific avatar. The config path is optional — present in VRChat's broadcast, omitted when we send a
plain load request.

### Input controller — `/input/<Axis|Button>`

The input space mirrors a game controller. Both kinds live under `/input/<Name>`, disambiguated only
by argument type:

- **Axes** (`InputAxis`) — continuous controls in `-1.0..=1.0`, one float argument. `Vertical`,
  `Horizontal`, `LookHorizontal`, `LookVertical`, and the GoGo/locomotion analogs (`MoveHold`,
  `SpinHold`, `UseAxisRight`, `GrabAxisRight`, …). The encoder **clamps** to `-1..=1`.
- **Buttons** (`InputButton`) — momentary controls, one int argument (`1` pressed, `0` released).
  `MoveForward`, `Jump`, `Run`, `Voice`, the grab/use/drop pairs, `PanicButton`, … VRChat treats a
  button like a held key: it stays active until it sees `0`, so a **tap is `1` then `0`** (the
  reset-to-zero / momentary semantics). `send_button(btn, true)` then `send_button(btn, false)`.

`InputMessage::{Axis, Button}` carries both; `from_osc` resolves the `/input/` suffix to a known
axis/button (returning `Ok(None)` for unknown suffixes) and uses the float-vs-int tag as
authoritative.

The canonical axis/button names are encoded as the `InputAxis` / `InputButton` enums (`name()` /
`from_name()` round-trip), so a daemon names a control by enum, not a stringly-typed address.

## The codec is pure and round-trip-tested

Every codec type is exercised by an encode→decode round-trip with no socket: each `ParamValue`
variant, `AvatarChange` with and without a config path, every `InputAxis` and `InputButton` name, the
axis clamp, and the button-state pair. Malformed/empty payloads assert the error path. This is the
same discipline as `avatar_input::osc::apply_message` — the wire format is verified independent of any
transport.

## The transport — `ParamClient`

`ParamClient` is the only piece that owns a UDP socket. It binds a **non-blocking** receive socket and
aims sends at VRChat's receive port:

- `connect_default()` — VRChat's defaults: receive on `127.0.0.1:9001` (VRChat's send port), send to
  `127.0.0.1:9000` (VRChat's listen port). `new(listen, target)` for custom wiring.
- `send_param`, `send_axis`, `send_button`, `send_avatar_change` — encode via the codec and fire one
  UDP datagram (fire-and-forget, as UDP is).
- `poll()` — drains every datagram queued since the last call and returns the `/avatar/parameters/*`
  updates it found. Non-parameter traffic (input echoes, `/avatar/change`, undecodable packets) is
  skipped rather than erroring, so a noisy socket never breaks the poll; it errors only on a genuine
  socket failure (never `WouldBlock`). Bundles are recursed. This mirrors
  `avatar_input::osc::OscSource::poll`.

`collect_params(packet, out)` is the pure receive-side helper (packet → parameter updates, recursing
bundles) so the receive path is unit-tested too, and a loopback round-trip test cross-wires two
clients on ephemeral ports to exercise real UDP.

## OSCQuery avatar config

When VRChat loads an avatar it writes a JSON file to
`…/VRChat/VRChat/OSC/<usr_id>/Avatars/<avtr_id>.json` describing every parameter the avatar exposes
over OSC. Parsing it gives a daemon the avatar's parameter **schema offline**, without probing the
live OSCQuery HTTP endpoint.

The file is an OSCQuery node tree: a root container whose `CONTENTS` recurses into nodes that may
carry `FULL_PATH`, `TYPE` (an OSC type-tag string like `"f"`), `ACCESS` (a bitmask — 1 = read,
2 = write, 3 = read/write), `VALUE`, and further `CONTENTS`. `AvatarConfig::from_json` /
`from_path` flatten this depth-first into:

- `AvatarConfig { name, params }` — the tree's display name and every value-bearing leaf (a node with
  a `TYPE`) in deterministic order.
- `AvatarParam { full_path, name, type_tag, access }` — `name` is the bit after
  `/avatar/parameters/` (so `/avatar/change` and containers have `name = None`); `Access` decodes the
  bitmask with `is_readable()` / `is_writable()` helpers (VRChat *sends* readable params, *accepts*
  writes to writable ones).

`config.param("VRCEmote")` looks one up by name; `config.avatar_parameters()` iterates just the
`/avatar/parameters/*` leaves (filtering containers and `/avatar/change`). The `VALUE` field is
deliberately ignored — the point of reading the file offline is the schema, not a stale snapshot.
Field renames (`FULL_PATH` → `full_path`, …) are `serde(rename)`; an embedded-JSON fixture asserts
the parse (param count, types, access, the `/avatar/change` exclusion, and the malformed/empty cases).

## Parameter capture (`avatar osc capture` / `avatar-gesture-capture.exe`)

`avatar_osc::capture` records the parameter stream VRChat broadcasts (OSC out, port 9001) and
reduces it to the report that answers *"what does my controller actually deliver?"*: per-parameter
update counts and value ranges, plus the **gesture cross-tab** — for every `GestureLeft`/
`GestureRight` value that was held, how many times it was entered and the range its
`…Weight` float covered while held. A row that never appears (e.g. `1 = Fist` missing after a
full Vive-touchpad sweep) or a weight range stuck at `0.000..0.000` localizes a gesture bug to
the input/binding side in one session; healthy rows mean the animator is receiving everything
and the FX layer is at fault. `avatar osc capture [--seconds N] [-o events.jsonl] [--json]`
echoes updates live, appends raw events as JSON lines as they arrive (a cut-short session keeps
its data), and prints the summary when the clock runs out.

Both capture front-ends **advertise themselves over OSCQuery** (`avatar_osc::oscquery`,
`--no-advertise` to opt out): a hand-built mDNS responder answers `PTR _oscjson._tcp.local`
(and announces periodically) while a tiny HTTP responder serves the `?HOST_INFO` handshake and a
parameter tree exposing `/avatar` — which is what tells modern VRChat to route its
avatar-parameter output to the advertised UDP port, *whatever* it is. That removes the
fixed-9001 assumption entirely (legacy output still lands when the port is free; if it is
taken, the capture falls back to an ephemeral port and lets discovery do the work). The mDNS
subset is hand-parsed/built like every format in this repo (12-byte header + labelled names,
compression-pointer-aware reader), the responder also reports VRChat's own `VRChat-Client-*`
announcements as a liveness diagnostic, and both the DNS round-trip and the handshake JSON are
unit-tested. Scope: same-host VRChat (the advertised address is `127.0.0.1`).

The same logic ships as a **standalone, dependency-slim binary** `avatar-gesture-capture`
(`crates/osc/src/bin/gesture_capture.rs`, `[[bin]]` in the crate) precisely so it cross-compiles
to a double-clickable Windows `.exe` (`cargo build -p avatar-osc --bins --release --target
x86_64-pc-windows-gnu`) for capture sessions on the machine that actually runs VRChat: it takes
`[seconds] [port]` positionally, writes `gesture-capture.jsonl` beside itself, and waits for
Enter before closing so the table survives a double-click launch.

## Offline controller replay (`avatar osc replay`)

The missing half of capture: `avatar osc replay <events.jsonl> --controller FX.controller
[--layer NAME] [--timeline] [--json]` runs the captured parameter log through the controller's
state machines **offline** — no Unity — and prints the state timeline each layer actually went
through: visits, dwell times, and the blend-parameter range covered inside each state (seeded
with the parameter's value at entry, since a constant weight sends no update events). Capture
proves what VRChat *delivered*; replay proves what the controller *did with it*. Simulated
semantics are the subset our generated controllers use: ordered Any-State + state transitions,
`m_CanTransitionToSelf`, condition modes If/IfNot/Greater/Less/Equals/NotEqual; crossfades are
treated as instantaneous and exit-time transitions are not modelled. On the mikunpc captures
this closed the centre-pad-blink investigation in one command: the wand delivers Fist with
analog weight, and the controller enters `Fist L`/`Fist R` and blends 0→1 exactly as designed.

## Analog-gesture daemon (`avatar-osc-gestures`)

The "Vive advanced controls on any hardware" feature (PLAN §4) lives in a separate crate built on
this one. VRChat blends its Fist gesture on an analog weight — `GestureLeftWeight` / `…Right` are
floats 0→1, and `GestureLeft` / `…Right` are ints 0–7 selecting the gesture. Vive wands drive those
from the trigger automatically; most hardware doesn't. The daemon reads a controller's analog
trigger (and optionally grip), maps it to a gesture + weight, and sends them via `ParamClient`.

The crate is deliberately **glam-free**: rather than depend on `avatar-input` (which pulls `glam`
into its 6-DoF pose types), it defines a minimal `AnalogSource` — per-hand `trigger`/`grip` floats
only — so the daemon and the `avatar` CLI stay out of the `glam` graph (a CLAUDE.md invariant). On
device, an OpenXR / `avatar-input` adapter implements `AnalogSource`; headless, `DemoSource` (a
deterministic triangle-wave sweep) and `ScriptedSource` cover demos and tests.

The mapping is pure and unit-tested: `HandMapping` applies a **deadzone** (resting trigger reads as
neutral, then rescales `(deadzone, 1]` → `(0, 1]` for a smooth response), defaults the trigger to the
Fist gesture, and optionally maps the grip to a separate static gesture. `GestureFrame::updates_since`
does **change detection** — only the `Gesture*` int(s) and `Gesture*Weight` float(s) that moved are
emitted, so a steady hand stops re-sending. `GestureDaemon` is the loop (`tick` / `run` / `run_for`),
sending through a `ParamSink` (implemented for `ParamClient`; tests use a recorder).

CLI: `avatar osc gestures` runs it. With no on-device backend headless, it drives the synthetic
`DemoSource` sweep (`--hz`, `--period`, `--seconds`) — pull it up against a running VRChat and watch
the Fist gesture blend, an end-to-end proof of the pipeline.

## Boundaries

- Send is UDP fire-and-forget; there is no delivery guarantee or retry (matching how VRChat's OSC
  works). The live VRChat socket path is not exercised in CI; the codec, the OSCQuery parser, and a
  loopback `ParamClient` round-trip are.
- The OSCQuery *HTTP/mDNS discovery* endpoint (advertising and querying over the network) is out of
  scope — only the on-disk config file is parsed. Live discovery is a future addition.
- The daemon's **production input backend (OpenXR)** is on-device work, not verifiable headless; the
  CLI ships a synthetic `DemoSource` until that adapter lands. The mapping, change detection, and OSC
  send are done and tested.
