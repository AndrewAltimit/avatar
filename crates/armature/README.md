# avatar-armature

Skeleton extraction, Unity humanoid bone inference, VRChat rig validation, and armature repair
planning. Package `avatar-armature` · library `avatar_armature`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Given a parsed [`avatar_fbx::FbxScene`](../fbx/README.md), it builds a bone hierarchy from the
`Model` objects, classifies bone names, then resolves a Unity humanoid mapping using the skeleton
hierarchy — depth ordering disambiguates the spine, arm, and leg chains, which names alone cannot do
reliably. It reports what is missing or mis-mapped against VRChat's rig requirements, and can plan
repairs to make a non-standard rig import cleanly.

## Key API

- `Skeleton::from_scene(&FbxScene)` — the bone hierarchy.
- `map_humanoid(&Skeleton) -> HumanoidMapping` — hierarchy-aware slot resolution
  (`slots`, `slot_ids`, `unique_id`).
- `analyze(&FbxScene) -> ArmatureReport` — the full validation report (`missing_required`, mapped
  bones, duplicates, `is_humanoid_ready()`).
- `humanoid::{HumanBone, BoneCategory, Side, Requirement, classify}` — the bone model + name
  classifier.
- `repair::{plan_repairs, apply_plan, RepairPlan, RepairEdit}` — diagnose a scene into discrete
  edits (canonical renames, applied; topology reparents + scale/orientation, flagged only) and apply
  the native ones (renames) to an `FbxDocument`.
- `blender::blender_script(&RepairPlan, input, output)` — render the *whole* plan (flagged geometry
  repairs included) as a headless-Blender Python script (`avatar armature fix --blender-script`).

## Status

Diagnosis + hierarchy-aware mapping: **M1**. Repair planning + apply: **M3** — canonical bone
**renames** are applied (the only native repair); topology reparents and scale/orientation are
**flagged, not applied** natively (they need a geometry transform, not a metadata relabel) and are
instead emitted as a **headless-Blender repair script** covering the full plan.

## Features

- `schema` (off by default) — derive [`schemars::JsonSchema`](https://crates.io/crates/schemars) on
  `ArmatureReport` so a consumer can emit a JSON Schema for the report. The `avatar` CLI enables it
  (on by default) to back `avatar schema armature`.

## See also

- [`docs/reference/humanoid-bones.md`](../../docs/reference/humanoid-bones.md) — the bone table and
  the rules that drive validation.
- [`docs/reference/armature-repair.md`](../../docs/reference/armature-repair.md) — what
  `armature fix` repairs and why scale/orientation are flagged, not applied.
