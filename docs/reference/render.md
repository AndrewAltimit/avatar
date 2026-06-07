# In-engine preview: `avatar render`

`avatar-render` (lib `avatar_render`, CLI `avatar render`) is the renderer layer of the toolchain:
an **offscreen GPU pipeline** that turns an avatar — and, experimentally, a Unity world scene — into
a PNG, headless. It builds on the runtime rig layer (FBX/glTF import → `RawMesh`) and the Unity-YAML
reader.

## The renderer (`avatar-render`)

A renderer-agnostic wgpu pipeline. Input: a `Scene` of `RenderMesh`es (world-space `transform` per
mesh), a look-at `Camera`, a directional+ambient `Light`, a clear colour. Output: RGBA8 pixels →
PNG. No window/surface — it renders into a texture and reads it back, so it works over SSH and in CI
wherever a GPU adapter (Vulkan/GL/Metal/DX) exists.

- One merged vertex/index buffer (mesh transforms baked CPU-side), a single uniform (view-projection
  + light), one draw call.
- 4× MSAA + depth buffer; right-handed camera with `0..1` clip depth (`glam::Mat4::perspective_rh`).
- **Two-sided** Lambert + ambient: imported meshes have inconsistent triangle winding, so both faces
  are lit rather than going black.
- `render_to_rgba` returns `Err` when no GPU adapter is available; the CLI surfaces that and the
  crate's smoke test skips.

`glam` enters the CLI dependency graph for the first time here (the renderer is built on it),
confined to the CLI's `render_scene`/`world` modules.

## Rendering an avatar — `avatar render --avatar <fbx|gltf|glb> -o out.png`

The avatar loader (`crates/cli/src/render_scene.rs`):

1. Imports the mesh(es) via `FbxDocument::meshes()` / `GltfDocument::meshes()`.
2. Uses the **raw control points** as the bind geometry. It deliberately does **not** apply the FBX
   skin-bind matrices: ripped/converted avatars (notably MMD→FBX) ship inconsistent per-cluster
   `Transform` matrices, so linear-blend skinning blends opposing rotations and the mesh collapses
   into spikes. The control points are always a clean, undeformed bind.
3. **Auto-uprights** the model by aligning its hips→head axis to +Y. That axis is measured from the
   *cluster centroids in control-point space* (weights + control points are reliable; the bind
   matrices are not), so a model authored lying down, sideways, or upside down comes out standing.
   Non-humanoid rigs fall back to the file's declared `UpAxis`.
4. Auto-frames the camera (`--yaw`/`--pitch` orbit) to the geometry's bounds.

This renders the avatar in its rest/bind pose. (Full *posed* skinning would need the bind matrices,
which this class of asset can't be trusted to provide; see the note above.)

## Rendering a world — `avatar render --world <scene.unity | project-dir>` (experimental)

The world loader (`crates/cli/src/world.rs`) parses a Unity `.unity` scene:

1. Reads the `Transform` (class 4) hierarchy and `MeshFilter` (class 33) components with the
   Unity-YAML reader's **lossy** parse (scenes contain MonoBehaviours whose serialized scalars
   `yaml-rust2` rejects; only Transforms/MeshFilters are needed, so the rest is skipped).
2. Composes each mesh's world matrix up the `m_Father` chain (memoized, cycle-guarded).
3. Resolves each `m_Mesh` GUID to a source FBX via a `guid → asset` index built from the project's
   `.meta` files, loads it (cached per GUID), and places every submesh at the transform.
4. Converts Unity's left-handed Y-up space to the renderer's right-handed space (negating Z).

**Limitations (best-effort static preview, not a Unity-accurate render):**
- Only directly-placed MeshFilters are drawn; **prefab instances are not expanded** (the Cozy Cabin
  main scene is almost entirely flat, so this still covers it).
- **No materials/textures** — everything is shaded flat grey.
- **No Unity import-pipeline emulation.** Unity applies each FBX's own scale/axis/node transforms on
  import; we don't, so props from FBX files with different authoring units render at inconsistent
  sizes, and large background meshes (skydomes, ground planes, the building shell) can dominate the
  frame. Combined `--avatar --world` therefore composes correctly but the avatar can be dwarfed by
  mis-scaled world geometry. A faithful world render needs the importer math (per-file scale factor,
  materials, prefab expansion) — future work.

## Verifying

The output is a PNG you can open. The avatar path is the validated one: it renders the real SDK2
avatar upright and undistorted. The world/combined paths parse and render real Unity scenes
end-to-end but are visually approximate per the limitations above.
