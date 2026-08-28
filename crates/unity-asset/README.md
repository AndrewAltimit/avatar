# avatar-unity-asset

Typed Unity asset graphs over [`avatar-unity-yaml`](../unity-yaml/README.md). Package
`avatar-unity-asset` · library `avatar_unity_asset`. Part of the [avatar](../../README.md) monorepo.

## What it does

Currently covers the **AnimatorController** (`.controller`) — the structure VRChat avatars drive
through their playable layers (FX, Gesture, Action, …) — and the **AnimationClip** (`.anim`) those
controllers play (`AnimationClip::from_file`). A `.controller` is a multi-document Unity
YAML stream: one `AnimatorController` (class 91) plus the state machines (1107), states (1102),
transitions (1101/1109), and blend trees (206) it owns, linked by local `fileID`s.

For the lint rules — which parameters are referenced, write-defaults consistency, missing default
states — it doesn't rebuild the full graph; it aggregates the relevant fields across the file's
documents by Unity class id. This is robust to SDK version drift, since the field names are stable
Unity serialization, not VRChat specifics.

## Key API

- `AnimatorController::from_file(&UnityFile) -> Option<Self>` — aggregate a parsed `.controller`.
- Exposes `parameters`, `conditions`, `blend_trees` (with `referenced_parameters()` respecting blend
  type), `state_machines` (default-state presence), `write_defaults` per state, `states` (name +
  `MotionRef`), and `blend_tree_motion_guids`.
- `AnimationClip::from_file(&UnityFile) -> Option<Self>` — a `.anim`'s curve *bindings*
  (`float_curves`, `transform_curves`, `pptr_curves`; `is_empty()` / `animates_transforms()` /
  `animates_muscles()`).

## Status

Built: **AnimatorController (M2)** + **AnimationClip** (curve bindings: float/transform/PPtr
counts, muscle detection, per-state `MotionRef`s on the controller side). Material / scene typing
in this crate is still open.

## See also

- [`docs/reference/sdk3-lint-rules.md`](../../docs/reference/sdk3-lint-rules.md) — the
  `VRC040`–`VRC049` controller/clip rules this crate feeds, plus `VRC062` (blendshape drift),
  which also consumes it.
