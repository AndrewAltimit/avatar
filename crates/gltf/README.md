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

## Key API

- `GltfDocument::from_slice(bytes) -> Result<GltfDocument>` — parse `.gltf`/`.glb` bytes (buffers must
  be embedded/resolvable from the bytes, e.g. a `.glb`).
- `GltfDocument::import<P: AsRef<Path>>(path) -> Result<GltfDocument>` — load a file, resolving any
  sidecar `.bin` buffers/images.
- `GltfDocument::meshes(&self) -> Vec<RawMesh>` — every mesh primitive as an `avatar_mesh::RawMesh`,
  with skin attached when the owning node is skinned.
- `GltfDocument::skeleton(&self) -> avatar_armature::Skeleton` — bones from the first skin's joints
  (node hierarchy → parents, names classified into humanoid categories).

Bind bridge: each cluster's `transform_link` is the joint bind-world (`inverse(inverseBindMatrix)`)
and `transform` is identity, so `avatar-pose`'s `transform_link⁻¹ · transform` recovers the glTF
inverse-bind. Material/texture import is not yet wired into the preview (`materials` is empty).

## Usage

```rust,no_run
use avatar_gltf::GltfDocument;

let doc = GltfDocument::import("avatar.glb")?;
let meshes = doc.meshes();      // Vec<avatar_mesh::RawMesh>
let skeleton = doc.skeleton();  // avatar_armature::Skeleton
// Pose + skin via avatar-pose, identical to the FBX path:
let posed = avatar_pose::PosedSkeleton::from_skinned_mesh(&skeleton, &meshes[0]);
# anyhow::Ok(())
```

## Status

Implements §9 #5 of the VR PRD. Behaviour: [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
