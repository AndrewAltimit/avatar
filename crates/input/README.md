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

## Features

- `osc` — pulls `rosc`, enables `osc::OscSource` and the (unit-tested) message decoder.

## Status

Implements §9 #3 of the VR PRD. Behaviour: [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
