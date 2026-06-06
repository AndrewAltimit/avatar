# avatar-gltf

glTF 2.0 importer. Package `avatar-gltf` · lib `avatar_gltf`. Part of the
[avatar](../../README.md) monorepo.

## What it does

A second rig source alongside FBX: `GltfDocument::{from_slice, import}` →

- `meshes()` — every primitive as an [`avatar_mesh::RawMesh`] (positions/indices/normals/UVs +
  skin from `JOINTS_0`/`WEIGHTS_0`/`inverseBindMatrices`).
- `skeleton()` — an [`avatar_armature::Skeleton`] from the skin's joints (node hierarchy → parents,
  names classified into humanoid categories).

Output feeds `avatar_pose::PosedSkeleton::from_skinned_mesh` identically to the FBX path; glTF is
friendlier for non-VRChat rigs (already triangulated/indexed, inverse-bind given directly).

## Status

Implements §9 #5 of the VR PRD. Behaviour: [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
