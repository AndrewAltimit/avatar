//! Headless render smoke test. Requires a working GPU adapter (Vulkan/GL/Metal/DX); if none is
//! available (e.g. a CI box with no GPU), `render_to_rgba` returns an error and the test prints a
//! skip notice and passes, so the suite stays green everywhere.

use avatar_render::{Camera, Light, RenderMesh, Scene};
use glam::Vec3;

/// A unit cube centred at the origin.
fn cube() -> RenderMesh {
    let p = |x: f32, y: f32, z: f32| [x, y, z];
    let positions = vec![
        p(-1., -1., -1.),
        p(1., -1., -1.),
        p(1., 1., -1.),
        p(-1., 1., -1.),
        p(-1., -1., 1.),
        p(1., -1., 1.),
        p(1., 1., 1.),
        p(-1., 1., 1.),
    ];
    let indices = vec![
        0, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6, 4, 5, 1, 4, 1, 0, 3, 2, 6, 3, 6, 7, 4, 0, 3, 4, 3, 7,
        1, 5, 6, 1, 6, 2,
    ];
    RenderMesh::new(positions, indices).with_color([0.85, 0.4, 0.2, 1.0])
}

#[test]
fn renders_a_cube_to_pixels() {
    let (w, h) = (160u32, 120u32);
    let bg = [0.05f32, 0.06, 0.08, 1.0];
    let mut scene = Scene {
        meshes: vec![cube()],
        camera: Camera {
            eye: Vec3::splat(5.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_deg: 45.0,
            znear: 0.1,
            zfar: 100.0,
        },
        light: Light::default(),
        background: bg,
    };
    let (min, max) = scene.world_bounds().expect("cube has bounds");
    scene.camera = Camera::frame_bounds(min, max, w as f32 / h as f32, 30.0, 20.0);

    let rgba = match avatar_render::render_to_rgba(&scene, w, h) {
        Ok(px) => px,
        Err(e) => {
            eprintln!("skip: no GPU adapter for offscreen render ({e:#})");
            return;
        }
    };

    assert_eq!(rgba.len(), (w * h * 4) as usize, "RGBA8 buffer size");

    // Background clear colour as it lands in the sRGB framebuffer's bytes.
    let bg8 = [
        (bg[0] * 255.0).round() as u8,
        (bg[1] * 255.0).round() as u8,
        (bg[2] * 255.0).round() as u8,
    ];
    // Some pixels must differ from the background — i.e. the cube actually drew.
    let non_bg = rgba
        .chunks_exact(4)
        .filter(|px| {
            (px[0] as i32 - bg8[0] as i32).abs() > 12
                || (px[1] as i32 - bg8[1] as i32).abs() > 12
                || (px[2] as i32 - bg8[2] as i32).abs() > 12
        })
        .count();
    let total = (w * h) as usize;
    assert!(
        non_bg > total / 50,
        "expected the cube to cover a meaningful area, got {non_bg}/{total} non-background pixels"
    );

    // Lit shading must produce a range of brightnesses across the cube's faces (not a flat fill):
    // the top face catches more of the directional light than the sides.
    let mut lo = 255u8;
    let mut hi = 0u8;
    for px in rgba.chunks_exact(4) {
        let lum = px[0].max(px[1]).max(px[2]);
        if lum != bg8[0].max(bg8[1]).max(bg8[2]) {
            lo = lo.min(lum);
            hi = hi.max(lum);
        }
    }
    assert!(
        hi.saturating_sub(lo) > 20,
        "expected directional shading variation across faces (lo={lo}, hi={hi})"
    );
}
