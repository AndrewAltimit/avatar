# In-engine preview: `avatar render`

`avatar-render` (lib `avatar_render`, CLI `avatar render`) is the renderer layer of the toolchain:
an **offscreen GPU pipeline** that turns an avatar — and a Unity world scene — into a PNG, headless.
It builds on the runtime rig layer (FBX/glTF import → `RawMesh`) and the Unity-YAML reader.

## The renderer (`avatar-render`)

A renderer-agnostic wgpu pipeline. Input: a `Scene` of `RenderMesh`es (world-space `transform` per
mesh), a look-at `Camera`, a directional+ambient `Light`, a clear colour. Output: RGBA8 pixels →
PNG. No window/surface — it renders into a texture and reads it back, so it works over SSH and in CI
wherever a GPU adapter (Vulkan/GL/Metal/DX) exists.

- One merged vertex buffer (mesh transforms baked CPU-side) + a single uniform (view-projection +
  light). The index buffer is grouped into per-texture **batches** (one draw per distinct texture),
  so a textured scene still uploads one vertex buffer and binds each texture once.
- **Textures.** A [`Scene`] carries a `textures` pool; each [`RenderMesh`] has UVs and an optional
  index into it. Textured batches bind a `texture_2d` + sampler (group 1); untextured ones bind a
  1×1 white texel so the shader path is uniform (base = texture × vertex tint). UVs are V-flipped
  (FBX/glTF are bottom-left origin, wgpu samples top-left). **Alpha cutout** at 0.5 discards
  transparent texels so foliage / hair / decal cards (transparent PNGs) don't render as opaque
  squares; opaque textures are unaffected.
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
4. **Textures** the avatar from its FBX-embedded materials: each mesh is split per material slot, and
   each slot's diffuse texture (an embedded `Content` blob, or a file resolved relative to the FBX)
   and diffuse colour are applied. Meshes without materials fall back to a per-submesh palette colour
   so parts stay visually distinct.
5. Auto-frames the camera (`--yaw`/`--pitch` orbit) to the geometry's bounds.

This renders the avatar in its rest/bind pose. (Full *posed* skinning would need the bind matrices,
which this class of asset can't be trusted to provide; see the note above.)

## Rendering a world — `avatar render --world <scene.unity | project-dir>`

The world loader (`crates/cli/src/world.rs`) parses a Unity `.unity` scene and emulates enough of
Unity's FBX import pipeline to place geometry at the right scale and assemble multi-mesh /
prefab-instanced models:

1. Reads the `Transform` (4), `MeshFilter` (33), `MeshRenderer` (23) and `PrefabInstance` (1001)
   objects with the Unity-YAML reader's **lossy** parse (scenes contain MonoBehaviours whose
   serialized scalars `yaml-rust2` rejects; only those object types are needed, so the rest is
   skipped).
2. Composes each scene transform's world matrix up the `m_Father` chain (memoized, cycle-guarded).
3. **FBX node-world transforms.** `avatar_fbx::meshes()` returns each mesh in its own geometry space;
   a multi-mesh FBX only assembles once each mesh is placed by its `Model` node's transform, composed
   up the FBX `OO` parent chain (Euler `XYZ`, degrees). This is what turns the cabin's 115 meshes from
   a pile at the origin into a building.
4. **Import scale.** Unity bakes the model import scale (`useFileScale`/`globalScale` ×
   `UnitScaleFactor`/100, read from the FBX + its `.meta`) into the imported mesh, so we apply it to
   the raw FBX geometry — fixing props from cm-unit FBX files that would otherwise render 100× too big.
5. **Prefab instances.** An instanced model's visible meshes are *not* serialized into the scene —
   only a stripped placeholder plus the instance's root override (`m_Modification`). We resolve
   `m_SourcePrefab` → FBX and re-instantiate every mesh at
   `world(m_TransformParent) · root_local · import_scale · node_world` (the root scale is the import
   scale, a prefab default that never appears in the scene). This is how the cabin shell renders.
6. **Directly-placed MeshFilters** keep using the **raw** mesh geometry (× import scale) at their
   GameObject's transform — what Unity does when a shared sub-mesh is assigned to a plain GameObject.
7. **Materials + textures.** Each renderer's `m_Materials` slots are resolved (per material GUID) to
   their `.mat`'s base colour (`_Color`) and base texture (`_MainTex` → asset → decoded pixels), and
   the mesh is split per slot so each slot draws with its own texture/tint. **Prefab-instanced models**
   (the cabin) have no scene-side material, so their materials are resolved instead through the model
   importer's **material remap** (`World.fbx.meta`'s `externalObjects`: FBX material *name* → project
   `.mat`), falling back to the FBX-embedded material's texture/colour. Unresolved materials fall back
   to neutral grey.
8. Converts Unity's left-handed Y-up space to the renderer's right-handed space (negating Z).

**Validated against the Cozy Cabin world** (PC export): the cabin assembles at correct scale (~6 m)
with its surrounding low-poly trees, props (clocks, iPad, pens) at correct real-world sizes inside
it, and the trees / cabin / props render **textured** (the alpha-cut foliage as proper pine, the
cabin's remapped materials, the props' `_MainTex`).

**Remaining limitations (still a static preview, not a pixel-accurate Unity render):**
- **Shading is flat-lit, not shader-accurate.** A single base-colour texture × tint under one
  Lambert light — no normal/metallic/emission maps, transparency blending (only a 0.5 alpha cutout),
  lightmaps, or custom shaders. Texture formats are limited to what `image` decodes (PNG/JPEG/TGA/
  BMP/GIF — no DDS/PSD/EXR); undecodable textures fall back to the material's flat colour.
- **FBX transform fidelity.** We compose `Lcl` Translation/Rotation/Scaling only — no per-node
  `RotationOrder`, pre/post-rotation, rotation/scale pivots, geometric transforms, or `InheritType`
  scale-inheritance modes. Uncommon on static world props; rare cases can be mis-oriented/scaled.
- **No prefab nesting or per-platform import overrides**, and only the first material per renderer is
  read for colour.

## Verifying

The output is a PNG you can open. The avatar path renders the real SDK2 avatar upright, undistorted,
and **textured** from its embedded materials. The world path assembles real Unity scenes at correct
scale **and texture** (validated against the Cozy Cabin world) — geometrically faithful, shaded with
base-colour textures (flat-lit, not shader-accurate) per the limitations above. Combined
`--avatar --world` composes both into one frame at consistent scale.
