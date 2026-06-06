# avatar-mesh

POD mesh + skin **interchange** types. Package `avatar-mesh` · lib `avatar_mesh`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Defines the format-agnostic data both importers produce and the pose layer consumes: `RawMesh`
(triangulated positions/normals/UVs/indices + `control_point_of_vertex`), `SkinData`, and
`SkinCluster` (per-bone influence indices/weights + bind matrices). **No `glam`, no format
dependency** — that isolation is the point: importers (`avatar-fbx`, `avatar-gltf`) stay math-free
and `avatar-pose` owns all matrix work.

Matrices are 16 `f64` in FBX row-major convention (`IDENTITY_16` provided); `avatar-pose` converts.

## Status

Foundation for the runtime rig layer. See [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
