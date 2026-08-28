# avatar-web-analyzer

The WebAssembly bundle behind the docs site's [FBX analyzer](../../site/_content/analyzer.html)
page: the same diagnose graph the CLI uses — `avatar-fbx` (parse + mesh extraction) +
`avatar-armature` (humanoid rig check) + `avatar-stats` (performance rank) — compiled to wasm and
run entirely in the visitor's browser, plus a glam-free scene extractor so the page can draw the
dropped avatar in WebGL with bone/skin/blendshape overlays. The dropped file is read into memory
and analyzed client-side; nothing is uploaded anywhere.

## Key API

Pure Rust core (testable off-wasm):

- `analyze(bytes, name) -> Result<Report>` — parse one binary FBX, run the armature analysis and
  the geometry performance rank, and summarize: `fbx` object counts, `global_settings`
  (unit scale / up / front axis), `meshes` (per-geometry vertex/triangle/skin/slot counts),
  `materials` (deduplicated; diffuse colour + texture path/embedded-bytes info), `blendshapes`
  (each with a `group`: `viseme` / `blink` / `expression` / `other`, see
  `classify_blendshape`), and `bone_tree` (every `Model` with parent, depth, humanoid slot).
- `SceneView::from_bytes(bytes) -> Result<SceneView>` — the renderable view. Geometry is the raw
  control points of every `avatar_fbx::FbxDocument::meshes()` mesh, placed exactly as `avatar
  render` places them (`crates/cli/src/render_scene.rs`): skinned meshes through the hips→head
  `auto_upright` (cluster-centroid based), static meshes through the declared-up-axis correction.
  Bone positions are the cluster weighted centroid when the bone skins something, else the
  composed `Lcl` TRS chain (`model_global_matrix`), both through the same upright. Normals are
  computed (area-weighted smooth) when the file has none.
- `math` — the minimal `f32` `Vec3`/`Quat`/`Mat4` the above needs. Deliberately **not** `glam`:
  the workspace confines `glam` to the runtime-rig + render crates, and this bundle must not pull
  wgpu.

wasm-bindgen exports (the [site contract](../../site/README.md)):

- `analyze_fbx(bytes, name) -> string` — `analyze` as JSON; errors are thrown exceptions.
  `armature` and `stats` in the JSON are the exact serde shapes the CLI's `--json` output uses.
- `sample_fbx() -> Uint8Array` — `avatar_testkit::fbx::humanoid_skinned()`: a T-posed ~1.6 m
  synthetic humanoid (cm, Y-up) with a skinned low-poly mesh (~5k triangles, normals + UVs), a
  cluster per bone, `Body`/`Head` material slots, and `vrc.v_aa`/`vrc.v_oh`/`Blink`/`Smile`
  blendshape channels — the page's "try a sample" avatar (`avatar-testkit` is a normal dependency
  here, feature `fbx` only).
- `class SceneView` — `static load(bytes)`, `manifest()` (JSON: applied `upright` quaternion,
  `bounds`, `meshes` with material-slot → material index lists, `materials` with sniffed texture
  `mime`, `bones` with world positions / parent index / humanoid slot / influenced-vertex counts,
  `blendshapes` with mesh index + group), and per-mesh typed-array getters `positions`,
  `normals`, `uvs`, `indices`, `triangle_materials`, `skin_indices` / `skin_weights` (4 per
  vertex, top-4, normalized, indexing `manifest.bones`), plus `texture(material)` for embedded
  image bytes. A file with no geometry (e.g. the testkit `humanoid_skeleton`) loads fine with an
  empty mesh list and bounds taken from the bones.

## Build

```sh
wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
```

CI does this on every Pages deploy (`deploy-pages` in `.github/workflows/main-ci.yml`); the main
`ci` job additionally runs a plain `cargo build --target wasm32-unknown-unknown` as a cheap
regression gate. The bundle output under `site/wasm/` is never committed.

## Status

Built; the report shape, the manifest, the buffer sizes, the upright behaviour (a strip authored
along +Z ends up along +Y) and the blendshape grouping are pinned by in-crate tests against the
`avatar-testkit` corpus plus a synthetic skinned/textured/blendshaped mesh built in the test
module. `publish = false` — this crate exists for the site, not as a library. The whole graph it
pulls is pure Rust with no fs/network use on the analysis path, which is what makes the wasm build
work; keep it that way when extending it.
