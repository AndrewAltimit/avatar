//! Offscreen GPU renderer — turns posed geometry into a PNG, headless.
//!
//! This is the "in-engine preview" draw layer: a [`Scene`] of [`RenderMesh`]es (each already in
//! its own local space plus a world `transform`), a [`Camera`], and a [`Light`] go in;
//! [`render_to_rgba`] runs a wgpu pipeline against an offscreen texture and returns RGBA8 pixels,
//! which [`save_png`] writes to disk. No window or surface is involved, so it runs over SSH / in CI
//! wherever a GPU adapter (Vulkan, GL, …) is reachable.
//!
//! Design notes:
//! - Each mesh's world `transform` is baked into vertex positions/normals on the CPU and every mesh
//!   is merged into a single vertex/index buffer drawn in one call. Per-mesh base colour rides along
//!   as a per-vertex attribute. This keeps the GPU side trivial (one uniform, one draw) at the cost
//!   of a CPU transform pass — fine for a one-shot preview.
//! - 4× MSAA, a depth buffer, a right-handed camera with reverse-free `0..1` clip depth (wgpu
//!   convention, matching `glam::Mat4::perspective_rh`).
//! - Each call spins up its own device; the preview is one-shot, not a render loop.

use anyhow::{Context, Result};
use glam::{Mat4, Vec3};

mod gpu;

/// An RGBA8 texture image (row-major, top-to-bottom, 4 bytes/pixel), referenced by index from a
/// [`RenderMesh`] via [`Scene::textures`].
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A drawable mesh in its own local space, placed by `transform`.
#[derive(Debug, Clone)]
pub struct RenderMesh {
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals; if empty, smooth normals are computed from the geometry.
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex texture coordinates, parallel to `positions`. Empty when untextured.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Linear RGBA base colour (the tint), modulated by any texture and shaded by the light.
    pub color: [f32; 4],
    /// Index into [`Scene::textures`] of the base-colour texture, if this mesh is textured.
    pub texture: Option<usize>,
    /// Local-to-world transform.
    pub transform: Mat4,
}

impl RenderMesh {
    /// A mesh at the origin with a default mid-grey colour and computed normals.
    pub fn new(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        RenderMesh {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            color: [0.75, 0.75, 0.78, 1.0],
            texture: None,
            transform: Mat4::IDENTITY,
        }
    }

    pub fn with_color(mut self, color: [f32; 4]) -> Self {
        self.color = color;
        self
    }

    pub fn with_transform(mut self, transform: Mat4) -> Self {
        self.transform = transform;
        self
    }
}

/// A look-at camera.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_deg: f32,
    pub znear: f32,
    pub zfar: f32,
}

impl Camera {
    /// View-projection for the given framebuffer aspect (width / height).
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective_rh(
            self.fov_y_deg.to_radians(),
            aspect.max(1e-4),
            self.znear,
            self.zfar,
        );
        let view = Mat4::look_at_rh(self.eye, self.target, self.up);
        proj * view
    }

    /// Frame an axis-aligned bounding box: orbit the camera to a 3/4 view that fits the box, with
    /// near/far planes sized to the scene. `yaw`/`pitch` are in degrees around the box centre.
    pub fn frame_bounds(min: Vec3, max: Vec3, aspect: f32, yaw_deg: f32, pitch_deg: f32) -> Camera {
        let center = (min + max) * 0.5;
        let radius = ((max - min) * 0.5).length().max(1e-3);
        let fov_y = 45.0_f32;
        // Distance so the bounding sphere fits vertically (and horizontally via aspect).
        let fit_v = radius / (fov_y.to_radians() * 0.5).tan();
        let fov_x = 2.0 * ((fov_y.to_radians() * 0.5).tan() * aspect.max(1.0)).atan();
        let fit_h = radius / (fov_x * 0.5).tan();
        let dist = fit_v.max(fit_h) * 1.25 + radius;

        let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
        let dir = Vec3::new(
            yaw.cos() * pitch.cos(),
            pitch.sin(),
            yaw.sin() * pitch.cos(),
        );
        Camera {
            eye: center + dir * dist,
            target: center,
            up: Vec3::Y,
            fov_y_deg: fov_y,
            znear: (dist - radius * 2.0).max(radius * 0.01),
            zfar: dist + radius * 4.0,
        }
    }
}

/// A single directional light plus ambient term.
#[derive(Debug, Clone, Copy)]
pub struct Light {
    /// Direction the light travels (world space); normalized internally.
    pub direction: Vec3,
    pub color: [f32; 3],
    /// Ambient fraction in `0..1` added below the diffuse term.
    pub ambient: f32,
}

impl Default for Light {
    fn default() -> Self {
        Light {
            direction: Vec3::new(-0.4, -1.0, -0.6),
            color: [1.0, 1.0, 1.0],
            ambient: 0.28,
        }
    }
}

/// Everything needed to render one frame.
#[derive(Debug, Clone)]
pub struct Scene {
    pub meshes: Vec<RenderMesh>,
    /// Texture pool; a [`RenderMesh::texture`] is an index into this. Empty for an untextured scene.
    pub textures: Vec<Texture>,
    pub camera: Camera,
    pub light: Light,
    /// Linear RGBA clear colour.
    pub background: [f32; 4],
}

impl Scene {
    /// Axis-aligned bounds over all meshes in world space (after their transforms). `None` if empty.
    pub fn world_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut any = false;
        for m in &self.meshes {
            for p in &m.positions {
                let w = m.transform.transform_point3(Vec3::from(*p));
                min = min.min(w);
                max = max.max(w);
                any = true;
            }
        }
        any.then_some((min, max))
    }
}

/// Render a scene to RGBA8 pixels (row-major, top-to-bottom, 4 bytes/pixel) at `width`×`height`.
pub fn render_to_rgba(scene: &Scene, width: u32, height: u32) -> Result<Vec<u8>> {
    pollster::block_on(gpu::render(scene, width.max(1), height.max(1)))
}

/// Write RGBA8 pixels (as returned by [`render_to_rgba`]) to a PNG file.
pub fn save_png(path: &std::path::Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .context("png header")?
        .write_image_data(rgba)
        .context("png data")?;
    Ok(())
}

/// Compute smooth (area-weighted) vertex normals from a triangle mesh.
pub fn compute_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let (pa, pb, pc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        // Cross product magnitude is proportional to triangle area → area-weighted accumulation.
        let face = (pb - pa).cross(pc - pa);
        normals[a] += face;
        normals[b] += face;
        normals[c] += face;
    }
    normals
        .into_iter()
        .map(|n| n.normalize_or_zero().into())
        .collect()
}
