# avatar-osc-gestures

The **analog-gesture daemon** — "Vive advanced controls on any hardware." Package
`avatar-osc-gestures` · lib `avatar_osc_gestures`. Part of the [avatar](../../README.md) monorepo;
the runtime half of the analog-gesture feature (PLAN §4, M5).

## What it does

VRChat blends its Fist gesture on an analog weight: `GestureLeftWeight` / `GestureRightWeight` are
floats 0→1, and `GestureLeft` / `GestureRight` are ints 0–7 picking *which* gesture. Vive wands drive
those from the trigger automatically; most other hardware doesn't. This daemon reads a controller's
analog **trigger** (and optionally **grip**), maps it to a gesture + weight, and sends them over OSC
via [`avatar-osc`](../osc/README.md) — so any headset gets the same fractional-gesture control.

## Key API

- **`AnalogSource`** — where input comes from: per-hand `trigger`/`grip` floats (`AnalogState`).
  Deliberately **glam-free and minimal** (no 6-DoF pose) so this crate and the `avatar` CLI stay out
  of the `glam` graph. On-device, an adapter from an OpenXR / `avatar-input` controller implements it.
  - `DemoSource` — a deterministic triangle-wave source (no hardware) for demos/tests.
  - `ScriptedSource` — replay precise frames, for tests.
- **`GestureConfig` / `HandMapping`** — the **pure** mapping: trigger → (`Gesture`, weight) per hand,
  with a deadzone (removes resting jitter, then rescales `(deadzone, 1]` → `(0, 1]`) and an optional
  grip gesture. Default: trigger → Fist, grip ignored.
- **`GestureFrame::updates_since`** — change detection: emits only the `Gesture*` int(s) and
  `Gesture*Weight` float(s) that moved, so a steady hand stops re-sending.
- **`ParamSink`** — the send target (implemented for `avatar_osc::ParamClient`; tests use a recorder).
- **`GestureDaemon`** — the loop: `tick` (poll → map → emit changed params), `run` (forever), and
  `run_for` (bounded, for demos/tests).

```rust
use avatar_osc_gestures::{DemoSource, GestureDaemon};
use avatar_osc::ParamClient;
use std::time::Duration;

let client = ParamClient::connect_default()?;
let mut daemon = GestureDaemon::new(DemoSource::new(120));
daemon.run(&client, Duration::from_millis(10))?; // 100 Hz, until Ctrl-C
# anyhow::Ok(())
```

## CLI

Driven by `avatar osc gestures` (a demo that sweeps a synthetic trigger so you can watch the Fist
gesture blend in a running VRChat):

```sh
avatar osc gestures --seconds 10        # 10s of synthetic trigger sweep at 100 Hz
avatar osc gestures --hz 60 --period 90 # 60 Hz ticks, ~1.5s sweep period
```

## Status

**M5** — library + CLI demo built and green. The production input backend is OpenXR (an on-device
`AnalogSource` adapter), left for hardware work; the mapping, change detection, and OSC send are
done and tested headless.

## See also

- [`../osc/README.md`](../osc/README.md) — the OSC parameter protocol + `ParamClient`.
- [`docs/reference/osc-runtime.md`](../../docs/reference/osc-runtime.md) — address space, codec, daemon.
