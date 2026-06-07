use avatar_render::{Camera, Light, RenderMesh, Scene};
use glam::{Mat4, Vec3};

fn main() -> anyhow::Result<()> {
    // Unit cube centered at origin.
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
        0, 1, 2, 0, 2, 3, // back
        4, 6, 5, 4, 7, 6, // front
        4, 5, 1, 4, 1, 0, // bottom
        3, 2, 6, 3, 6, 7, // top
        4, 0, 3, 4, 3, 7, // left
        1, 5, 6, 1, 6, 2, // right
    ];
    let mesh = RenderMesh::new(positions, indices).with_color([0.85, 0.55, 0.25, 1.0]);
    let mesh2 = mesh
        .clone()
        .with_color([0.3, 0.6, 0.85, 1.0])
        .with_transform(
            Mat4::from_translation(Vec3::new(2.5, 0.0, 0.0)) * Mat4::from_scale(Vec3::splat(0.6)),
        );

    let mut scene = Scene {
        meshes: vec![mesh, mesh2],
        camera: Camera {
            eye: Vec3::ZERO,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_deg: 45.0,
            znear: 0.1,
            zfar: 100.0,
        },
        light: Light::default(),
        background: [0.12, 0.13, 0.16, 1.0],
    };
    let (w, h) = (480u32, 360u32);
    let (min, max) = scene.world_bounds().unwrap();
    scene.camera = Camera::frame_bounds(min, max, w as f32 / h as f32, 35.0, 22.0);

    let rgba = avatar_render::render_to_rgba(&scene, w, h)?;
    avatar_render::save_png(std::path::Path::new("/tmp/render_cube.png"), w, h, &rgba)?;
    println!("wrote /tmp/render_cube.png");
    Ok(())
}
