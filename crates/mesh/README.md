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

## Key API

- `RawMesh` — one importer-produced mesh: `positions`/`normals`/`uvs`/`indices` (per emitted, post-
  triangulation vertex), `control_point_of_vertex` (maps each emitted vertex back to its source
  control point — the bridge skin weights are reattached through), optional `skin`, and
  `materials`/`material_of_triangle`. Helpers: `vertex_count()`, `control_point_count()`,
  `is_skinned()`, `material_slot_count()`, `triangle_material(tri)`.
- `SkinData { clusters: Vec<SkinCluster> }` — a mesh's skin binding.
- `SkinCluster` — one bone's influence: `bone_id`, parallel `indexes`/`weights` (keyed by control
  point), and bind matrices `transform_link` (bone bind-world) + `transform` (mesh bind-world).
- `MeshMaterial` — `name`, optional `diffuse_color`, optional `texture: TextureRef`.
- `TextureRef` — an *unresolved* texture: `relative`/`absolute` paths and/or `embedded` bytes; the
  preview layer decodes it to pixels.
- `IDENTITY_16: [f64; 16]` — the row-major identity in this crate's matrix convention.

## Usage

```rust
use avatar_mesh::RawMesh;

// A static (unskinned) two-triangle quad with no material info.
let mesh = RawMesh {
    model_id: 0,
    positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
    normals: None,
    uvs: None,
    indices: vec![0, 1, 2, 0, 2, 3],
    control_point_of_vertex: vec![0, 1, 2, 3],
    skin: None,
    materials: Vec::new(),
    material_of_triangle: Vec::new(),
};
assert_eq!(mesh.vertex_count(), 4);
assert!(!mesh.is_skinned());
assert_eq!(mesh.material_slot_count(), 1); // floors at slot 0
```

## Status

Foundation for the runtime rig layer. See [`docs/reference/rig-runtime.md`](../../docs/reference/rig-runtime.md).
