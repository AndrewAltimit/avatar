# avatar-fbx

Load **and write** binary FBX files. Package `avatar-fbx` · library `avatar_fbx`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Reads the parts of a binary FBX that avatar tooling needs — the object table (`Model`, `Geometry`,
…) and the `Connections` graph that wires objects into a hierarchy — and exposes them as a flat,
typed scene. It also retains `fbxcel`'s mutable node tree so a loaded document can be edited and
serialized back to binary FBX (the basis of `avatar armature fix`).

The crate stays close to the raw FBX structure: it does not compute world transforms or interpret
skinning. Higher-level interpretation lives in [`avatar-armature`](../armature/README.md).

**Scope:** binary FBX 7.x only (the Autodesk/Unity/Blender default). ASCII FBX is rejected with a
clear error — re-export as binary.

## Key API

- `FbxScene` — the read view: `load(path)`, `models()`, `object(id)`, `children_of(id)`,
  `parent_of(id)`, `blendshape_channels()` (the morph-channel names Unity imports as blendshapes,
  each traced to its mesh), plus `global_settings`, `objects`, `connections`.
- `FbxDocument` — the writable document: `load(path)` / `from_bytes(&[u8])`, `scene()` (recomputed
  read view), `blendshape_target_indexes(channel)` (the control points a morph channel deforms —
  join with `meshes()` for "which material slots does this blendshape touch"), and id-addressed
  mutators `rename_object`, `reparent_object`, `set_global_setting_f64/i32`, `scale_object`, then
  `to_bytes()` / `write(path)`.
- `FbxObject`, `Connection`, `GlobalSettings`, `LocalTransform` — the typed pieces of the scene.

Mutators address objects by FBX **object id** — the stable identifier skin clusters and animation
curves reference — so renaming a bone never breaks skinning or animation.

## Status

Reading: **M0/M1**. Writing (`FbxDocument`): **M3**. Built on [`fbxcel`](https://github.com/lo48576/fbxcel)
0.9 (`tree` + `writer` features).

## See also

- [`docs/reference/armature-repair.md`](../../docs/reference/armature-repair.md) — how the writer is
  used and its known characteristics.
- [`PLAN.md`](../../PLAN.md) §1, §8 — the two-layer model and the (now resolved) FBX-write risk.
