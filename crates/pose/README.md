# avatar-pose

Runtime posing + skinning. Package `avatar-pose` · lib `avatar_pose`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Turns a skeleton + [`avatar_mesh::RawMesh`] bind data into renderer-agnostic pose data:

- `PosedSkeleton::from_fbx` / `::from_skinned_mesh` / `::from_parts` — build the rig (bind +
  topology) once.
- `Pose` (`rest_pose()`, `set_local`, `set_local_trs`) — mutable per-bone local transforms; this
  is what the CLI's `avatar render --pose <prefab>` / `--stretch` preview drives.
  `pose_humanoid` poses a mapped humanoid slot by a rotation relative to its bind frame.
- `world_matrices`, `palette` / `palette_cols` — FK and the GPU bone-matrix palette (identity at
  rest); `palette_cols` returns raw `[f32;16]` to avoid `glam`-version coupling with the renderer.
- `build_vertex_skin` + `cpu_skin` — ≤4 normalized influences per vertex and CPU linear-blend
  skinning (for tests/headless).
- `pose::ik::TwoBoneIk` — analytic, geometric two-bone IK (arms/legs follow targets).

The only `glam` in the runtime tier. The viewport owns the actual wgpu draw.

## Key invariant

At rest, CPU-skinning reproduces the input vertices exactly — the test that validates extraction +
bind math + FK + skinning without a renderer.

## Status

Implements the posing/skinning/IK pieces of the VR-spectator runtime-rig band
([`PLAN.md`](../../PLAN.md) §9, "Runtime rig layer"). Behaviour:
[`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
