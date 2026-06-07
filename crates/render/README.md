# avatar-render

Offscreen GPU renderer — geometry in, PNG out, headless. Package `avatar-render` ·
lib `avatar_render`. Part of the [avatar](../../README.md) monorepo.

## What it does

The "in-engine preview" draw layer. A [`Scene`] of [`RenderMesh`]es (each with a world `transform`),
a look-at [`Camera`], and a directional+ambient [`Light`] go in; [`render_to_rgba`] runs a **wgpu**
pipeline against an **offscreen texture** and returns RGBA8 pixels, which [`save_png`] writes to
disk. No window or surface is created, so it runs over SSH and in CI wherever a GPU adapter
(Vulkan/GL/Metal/DX) is reachable. (Validated on NVIDIA via Vulkan.)

Pure GPU + math (`wgpu`, `glam`, `bytemuck`, `png`): it knows nothing about FBX, Unity, or skinning.
The caller hands it world-space meshes. In this repo that caller is the `avatar render` CLI command
(see `crates/cli/src/render_scene.rs` + `world.rs`).

## Key API

- `Scene { meshes, camera, light, background }`, `RenderMesh { positions, normals, indices, color,
  transform }` (empty `normals` ⇒ smooth normals are computed), `Camera`, `Light`.
- `Camera::frame_bounds(min, max, aspect, yaw, pitch)` — orbit-frame an AABB to a 3/4 view.
- `Scene::world_bounds()` — AABB over all placed meshes.
- `render_to_rgba(&scene, w, h) -> Result<Vec<u8>>` — returns `Err` if no GPU adapter is available.
- `save_png(path, w, h, &rgba)`, `compute_normals(positions, indices)`.

```rust
use avatar_render::{Scene, RenderMesh, Camera, Light};
# use glam::Vec3;
let mesh = RenderMesh::new(positions, indices);
let mut scene = Scene { meshes: vec![mesh], camera: /* … */
#   Camera { eye: Vec3::ONE, target: Vec3::ZERO, up: Vec3::Y, fov_y_deg: 45.0, znear: 0.1, zfar: 100.0 },
    light: Light::default(), background: [0.1, 0.11, 0.13, 1.0] };
let (min, max) = scene.world_bounds().unwrap();
scene.camera = Camera::frame_bounds(min, max, 4.0/3.0, 35.0, 18.0);
let rgba = avatar_render::render_to_rgba(&scene, 960, 720)?;
avatar_render::save_png("out.png".as_ref(), 960, 720, &rgba)?;
# anyhow::Ok(())
```

## Pipeline notes

- Each mesh's world `transform` is baked into vertex positions/normals on the CPU; all meshes merge
  into one vertex/index buffer drawn in a single call, with per-mesh colour as a vertex attribute.
- 4× MSAA, a depth buffer, two-sided directional + ambient lighting (imported meshes have
  inconsistent winding, so faces are lit from both sides), right-handed `0..1`-depth camera.
- Each call creates its own device — the preview is one-shot, not a render loop.

## Status

Built and green. Headless smoke test (`tests/smoke.rs`) renders a cube and checks coverage +
shading variation; it skips cleanly when no GPU adapter is present. Behaviour:
[`docs/reference/render.md`](../../docs/reference/render.md).
