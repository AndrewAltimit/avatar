# avatar-input

Backend-agnostic VR tracker/controller input. Package `avatar-input` · lib `avatar_input`. Part of
the [avatar](../../README.md) monorepo.

## What it does

Exposes one frame of tracking — `TrackerState` (HMD + two controllers with analog trigger/grip/stick
+ extra trackers, all 6-DoF `glam` transforms) — behind a `TrackerSource::poll()` trait so a viewport
can drive an avatar pose without caring about the source:

- `MockSource` — deterministic scripted frames (tests, headless dev). Always available.
- `osc::OscSource` — real UDP/OSC backend behind the `osc` feature; transform-oriented addresses
  (`/tracking/hmd`, `/tracking/controller/{left,right}`, `/tracking/tracker/<n>`), **not** VRChat
  `/avatar/parameters/*`.
- **OpenXR** — the intended on-device backend; implements the same trait, dropped in later (needs the
  loader + a headset).

`body_ik_targets` maps a frame to arm IK targets for `avatar_pose::ik`.

## Key API

- `TrackerState` — one frame: `hmd: Pose6dof`, `left`/`right: Controller`, extra `trackers:
  Vec<Pose6dof>`, `time: f64`.
- `Pose6dof { position: Vec3, orientation: Quat }` and `Controller { pose, trigger, grip, stick,
  buttons }` (analog axes + button bitmask).
- `trait TrackerSource { fn poll(&mut self) -> TrackerState; }` — the backend interface, polled once
  per frame (non-blocking).
- `MockSource::new(frames) / MockSource::fixed(state)` — deterministic scripted source (holds on the
  last frame); always available.
- `osc::OscSource` — real UDP/OSC backend, behind the `osc` feature.
- `body_ik_targets(&TrackerState) -> BodyIkTargets` — arm IK targets (`IkTarget { position, pole }`
  for each hand, plus `head: Vec3`); elbow pole sits below each controller in its own frame.

## Usage

```rust
use avatar_input::{MockSource, TrackerSource, TrackerState, body_ik_targets};
use glam::Vec3;

// Drive the rig from a scripted source (a real viewport polls an OSC/OpenXR source instead).
let mut state = TrackerState::default();
state.left.pose.position = Vec3::new(-0.3, 1.0, 0.2);
state.right.pose.position = Vec3::new(0.3, 1.0, 0.2);
state.hmd.position = Vec3::new(0.0, 1.6, 0.0);

let mut source = MockSource::fixed(state);
let frame = source.poll();
let targets = body_ik_targets(&frame); // feed to avatar_pose::ik::TwoBoneIk
assert_eq!(targets.head, Vec3::new(0.0, 1.6, 0.0));
```

## Features

- `osc` — pulls `rosc`, enables `osc::OscSource` and the (unit-tested) message decoder.

## Status

Implements §9 #3 of the VR PRD. Behaviour: [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
