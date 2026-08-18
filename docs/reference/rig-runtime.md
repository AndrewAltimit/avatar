# Rig runtime (skinning · posing · IK · input)

The runtime rig layer lets the tools **load and drive a rig at render time** — skin a mesh, pose a
skeleton each frame, and accept VR tracker input — entirely **renderer-agnostic**. It exists for
consumers like the Legaia VR spectator viewport, which owns the wgpu draw and only wants *data*
(vertices, weights, a bone-matrix palette) and *input* (tracker transforms). None of it touches the
VRChat path; it generalizes the layer *beneath* VRChat (rig + pose + input).

## Crates

| Crate | Role |
|-------|------|
| `avatar-mesh` | POD interchange: `RawMesh` / `SkinData` / `SkinCluster`. No `glam`, no format dep — both importers produce it, `avatar-pose` consumes it. |
| `avatar-fbx` (`FbxDocument::meshes`) | Extract geometry + skin/bind from FBX `Geometry`/`Deformer` nodes → `RawMesh`. |
| `avatar-gltf` (`GltfDocument`) | Second importer: glTF 2.0 → the same `RawMesh` + an `avatar-armature` `Skeleton`. |
| `avatar-pose` | `PosedSkeleton` + `Pose`: world matrices, GPU bone-matrix **palette**, CPU skinning, and `pose::ik` two-bone IK. The only `glam` in the runtime tier. |
| `avatar-input` | Backend-agnostic `TrackerState` + `TrackerSource` (HMD + controllers + trackers). `MockSource` always; `osc::OscSource` behind the `osc` feature; OpenXR is a documented future backend. |

`glam` is deliberately confined to `avatar-pose`, `avatar-input`, and `avatar-gltf` — the
lint/cli/descriptor crates never see it.

## Skin extraction (FBX)

`FbxDocument::meshes()` walks the connection graph `Geometry → Deformer(Skin) → SubDeformer(Cluster)
→ bone Model`, reading per-cluster `Indexes`/`Weights` and the two bind matrices `Transform` (mesh
bind) and `TransformLink` (bone bind world). Polygons are fan-triangulated into a triangle soup;
`RawMesh::control_point_of_vertex` maps each emitted vertex back to its FBX control point, which is
how control-point-keyed skin weights reattach after triangulation.

## The bind/posing math

- **Bind comes from the file, never re-derived.** Each bone's bind-world is `TransformLink` (FBX) or
  `inverse(inverseBindMatrix)` (glTF). We never reconstruct it from `Lcl Rotation` + `PreRotation`,
  so the FBX pivot/pre-rotation hazard (the same one `armature fix` flags for reparents) never
  arises for skinned bones.
- **Inverse-bind = `TransformLink⁻¹ · Transform`**, not `TransformLink⁻¹` alone — the latter only
  works when the mesh bind transform is identity (true on Mixamo, false in general).
- **Local-bind = `bind_world[parent]⁻¹ · bind_world[b]`**; FK is `world[b] = world[parent] ·
  local[b]`; the **palette** is `world[b] · inverse_bind[b]`.
- **Matrix convention:** FBX stores 16 row-major doubles; glam is column-major. The same 16 floats
  fed to `Mat4::from_cols_array` give glam's column-vector form of the identical transform — **no
  transpose** (`mat4_from_fbx`). glTF importers store glam's `to_cols_array()`, which round-trips
  through the same reader.

### The rest-pose invariant (the core test)

At rest (`pose == rest_pose()`), every world matrix equals its bind world, so every palette entry is
identity (when the mesh bind `Transform` is identity) and **CPU-skinning reproduces the input
vertices exactly**. `avatar-pose` and `avatar-gltf` both assert this — it validates extraction, bind
math, FK, and skinning end-to-end with no renderer or Unity.

## Posing API

```rust
let posed = PosedSkeleton::from_fbx(&skeleton, &scene, &mesh);   // or ::from_skinned_mesh (glTF)
let mut pose = posed.rest_pose();
posed.pose_humanoid(&mut pose, &mapping, HumanBone::LeftLowerArm, rot);  // by humanoid slot
let palette: Vec<[f32;16]> = posed.palette_cols(&pose);          // upload to a storage buffer
let skin = posed.build_vertex_skin(&mesh);                       // joints[4]+weights[4] per vertex
```

`palette_cols` returns raw `[f32;16]` so the consuming renderer isn't coupled to this crate's `glam`
version. GPU skinning does the weighted matrix-sum in a vertex shader; `cpu_skin` does the same on
the CPU for tests/headless.

**Posing raw control points (untrusted binds).** When the per-cluster bind `Transform`s can't be
trusted (converted avatars — see [render.md](render.md)), a pose can still be applied to the raw
control points as a *delta*: per bone `G⁻¹ · world(pose) · world(rest)⁻¹ · G`, where `G =
model_global_matrix(&scene, mesh.model_id)` is the mesh node's own global transform (the space the
control points live in). It is identity for every untouched bone and never involves the cluster
`Transform`s; `avatar render --stretch` is built on it. `lcl_to_mat4` (plain Lcl TRS) is public
for the same reason.

## Two-bone IK (`avatar_pose::ik`)

`TwoBoneIk { root, mid, end }.solve(&posed, &mut pose, target, pole)` is an analytic, **geometric**
solve: it places the elbow via the law of cosines and swings the joints onto it with
`Quat::from_rotation_arc` (robust to the axis/sign bugs of the interior-angle formulation).
Out-of-reach targets clamp to full extension; a target on the root is left untouched.

## Tracker input (`avatar-input`)

`TrackerState` is one frame of tracking (HMD + 2 controllers with analog trigger/grip/stick + extra
trackers). `TrackerSource::poll()` is the backend seam:

- `MockSource` — deterministic scripted frames (tests, headless dev).
- `osc::OscSource` (feature `osc`) — real UDP/OSC backend; transform-oriented addresses
  (`/tracking/hmd`, `/tracking/controller/{left,right}`, `/tracking/tracker/<n>`), **not** VRChat
  `/avatar/parameters/*`. The message→state decode (`apply_message`) is a pure, unit-tested function.
- **OpenXR** — the intended on-device backend; it implements the same `TrackerSource` and drops in
  without touching the viewport. Left for hardware work (needs the OpenXR loader + a headset, neither
  verifiable headless).

`body_ik_targets(&TrackerState)` is the pure-geometry glue that turns a frame into arm IK targets to
feed `TwoBoneIk`.

## Boundaries

- Normals/UVs cover the common FBX layouts (`ByControlPoint`/`ByPolygonVertex` × `Direct`/
  `IndexToDirect`); unsupported layouts degrade to `None` rather than emit garbage. Skinning needs
  neither.
- Non-skinned FBX bones' rest pose uses `Lcl` TRS and ignores `PreRotation` (not read) — it affects
  only bones that drive no vertices.
- The OpenXR backend and live OSC/hardware paths are not exercised in CI; the `MockSource` and the
  pure OSC decoder are.
