//! Glue between the importers (FBX/glTF) and the `avatar-render` GPU layer: load an avatar's
//! geometry at rest pose into a renderable [`Scene`]. World-scene loading is added alongside this.

use std::path::Path;

use anyhow::{Result, bail};
use avatar_armature::{HumanBone, Skeleton};
use avatar_mesh::RawMesh;
use avatar_render::{Camera, Light, RenderMesh, Scene};
use glam::{Mat4, Quat, Vec3};

/// A soft, distinguishable colour per submesh so body/hair/clothes don't merge into one blob.
const PALETTE: &[[f32; 4]] = &[
    [0.82, 0.80, 0.78, 1.0],
    [0.70, 0.78, 0.88, 1.0],
    [0.88, 0.74, 0.66, 1.0],
    [0.74, 0.84, 0.72, 1.0],
    [0.86, 0.80, 0.66, 1.0],
    [0.80, 0.72, 0.84, 1.0],
];

fn color_for(i: usize) -> [f32; 4] {
    PALETTE[i % PALETTE.len()]
}

/// Rotation bringing an FBX file's up-axis to the renderer's Y-up. FBX `UpAxis`: 0=X, 1=Y, 2=Z.
/// Z-up (the common Maya/3ds-Max export) → rotate −90° about X so a Z-up point (x,y,z) maps to
/// (x, z, −y). Y-up (Unity's convention) needs nothing.
fn up_axis_correction(up_axis: Option<i32>) -> Mat4 {
    match up_axis {
        Some(2) => Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        _ => Mat4::IDENTITY,
    }
}

/// Weighted centroid, in raw control-point space, of the vertices a bone's skin cluster drives.
/// This is where the bone's geometry physically sits in the mesh's authored bind — robust because
/// it reads only the (reliable) weights and control points, never the (often-broken) bind matrices.
fn bone_centroid(meshes: &[RawMesh], bone_id: i64) -> Option<Vec3> {
    for m in meshes {
        let Some(skin) = &m.skin else { continue };
        let Some(c) = skin.clusters.iter().find(|c| c.bone_id == bone_id) else {
            continue;
        };
        // control-point index -> a representative emitted-vertex position.
        let mut cp_pos: std::collections::HashMap<u32, [f32; 3]> = std::collections::HashMap::new();
        for (k, &cp) in m.control_point_of_vertex.iter().enumerate() {
            cp_pos.entry(cp).or_insert(m.positions[k]);
        }
        let mut sum = Vec3::ZERO;
        let mut wsum = 0.0f32;
        for (&cp, &w) in c.indexes.iter().zip(&c.weights) {
            if let Some(p) = cp_pos.get(&cp) {
                sum += Vec3::from(*p) * w;
                wsum += w;
            }
        }
        if wsum > 0.0 {
            return Some(sum / wsum);
        }
    }
    None
}

/// A rotation that stands an avatar upright in control-point space, regardless of authoring axes.
///
/// We measure the avatar's own **hips → head** direction from where those bones' geometry sits
/// (see [`bone_centroid`]) and align it to +Y with one shortest-arc rotation — uprighting a model
/// that was authored lying down, sideways, or upside down. Reading cluster centroids (weights +
/// control points) sidesteps the unreliable per-cluster bind matrices some converted avatars ship.
/// Falls back to the file's declared up-axis when the rig isn't a recognizable humanoid.
fn auto_upright(scene: &avatar_fbx::FbxScene, meshes: &[RawMesh]) -> Mat4 {
    let fallback = up_axis_correction(scene.global_settings.up_axis);
    let skeleton = Skeleton::from_scene(scene);
    let mapping = avatar_armature::map_humanoid(&skeleton);
    let (Some(head_id), Some(hips_id)) = (
        mapping.unique_id(HumanBone::Head),
        mapping.unique_id(HumanBone::Hips),
    ) else {
        return fallback;
    };
    let (Some(head), Some(hips)) = (
        bone_centroid(meshes, head_id),
        bone_centroid(meshes, hips_id),
    ) else {
        return fallback;
    };
    let up = head - hips;
    if up.length() < 1e-4 {
        return fallback;
    }
    Mat4::from_quat(Quat::from_rotation_arc(up.normalize(), Vec3::Y))
}

/// Build a [`RenderMesh`] from world-space `positions` (normals are recomputed by the renderer,
/// since skinning re-orients geometry and any source normals would be stale).
fn render_mesh(
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    color: [f32; 4],
    place: Mat4,
) -> RenderMesh {
    RenderMesh {
        positions,
        normals: Vec::new(),
        indices,
        color,
        transform: place,
    }
}

/// Bind-pose positions for one mesh: the raw control points (the mesh's authored bind geometry).
///
/// We deliberately do **not** apply the FBX skin-bind matrices. In principle the rest pose is
/// `Σ wᵦ·Tᵦ·v`, but ripped/converted avatars (notably MMD→FBX) routinely ship inconsistent per-
/// cluster `Transform` matrices — e.g. one bone's bind rotates +Z-up while another's is flipped —
/// so linear-blend skinning blends opposing rotations and the mesh collapses into spikes. The raw
/// control points are always a clean, undeformed bind; orientation is recovered separately by
/// [`auto_upright`], which measures the avatar's own hips→head axis in this same space.
fn bind_pose_positions(m: &RawMesh) -> Vec<[f32; 3]> {
    m.positions.clone()
}

/// Load an avatar (FBX or glTF/GLB) into render meshes at rest/bind pose, placed by `extra`
/// (identity for a standalone avatar; a spawn transform when dropped into a world).
pub fn load_avatar_placed(path: &Path, extra: Mat4) -> Result<Vec<RenderMesh>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let out: Vec<RenderMesh> = match ext.as_str() {
        "fbx" => {
            let doc = avatar_fbx::FbxDocument::load(path)?;
            let scene = doc.scene();
            let meshes = doc.meshes()?;
            if meshes.is_empty() {
                bail!("no meshes found in {}", path.display());
            }
            // Skinned meshes are placed at their bind transform then uprighted via the skeleton;
            // static meshes get the file's declared up-axis correction.
            let skinned_place = extra * auto_upright(&scene, &meshes);
            let static_place = extra * up_axis_correction(scene.global_settings.up_axis);
            meshes
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.positions.is_empty() && !m.indices.is_empty())
                .map(|(i, m)| {
                    let place = if m.is_skinned() {
                        skinned_place
                    } else {
                        static_place
                    };
                    render_mesh(
                        bind_pose_positions(m),
                        m.indices.clone(),
                        color_for(i),
                        place,
                    )
                })
                .collect()
        }
        // glTF is defined as Y-up; meshes come back in usable space.
        "gltf" | "glb" => {
            let raw = avatar_gltf::GltfDocument::import(path)?.meshes();
            if raw.is_empty() {
                bail!("no meshes found in {}", path.display());
            }
            raw.iter()
                .enumerate()
                .filter(|(_, m)| !m.positions.is_empty() && !m.indices.is_empty())
                .map(|(i, m)| {
                    render_mesh(m.positions.clone(), m.indices.clone(), color_for(i), extra)
                })
                .collect()
        }
        other => bail!("unsupported avatar format '.{other}' (expected .fbx, .gltf, or .glb)"),
    };
    if out.is_empty() {
        bail!("no renderable meshes in {}", path.display());
    }
    Ok(out)
}

/// Load an avatar at the origin (convenience for the standalone-avatar case).
pub fn load_avatar(path: &Path) -> Result<Vec<RenderMesh>> {
    load_avatar_placed(path, Mat4::IDENTITY)
}

/// Load a world/map scene into placed render meshes. (Implemented in the world-scene step.)
pub fn load_world(_path: &Path) -> Result<Vec<RenderMesh>> {
    bail!("world rendering is not implemented yet; use --avatar for now")
}

/// Assemble a [`Scene`] from meshes, auto-framing the camera to the geometry's bounds.
pub fn scene_from_meshes(
    meshes: Vec<RenderMesh>,
    width: u32,
    height: u32,
    yaw_deg: f32,
    pitch_deg: f32,
) -> Result<Scene> {
    if meshes.is_empty() {
        bail!("nothing to render (no meshes)");
    }
    let mut scene = Scene {
        meshes,
        camera: Camera {
            eye: Vec3::ONE,
            target: Vec3::ZERO,
            up: Vec3::Y,
            fov_y_deg: 45.0,
            znear: 0.1,
            zfar: 1000.0,
        },
        light: Light::default(),
        background: [0.10, 0.11, 0.13, 1.0],
    };
    let (min, max) = scene.world_bounds().expect("non-empty meshes have bounds");
    scene.camera = Camera::frame_bounds(min, max, width as f32 / height as f32, yaw_deg, pitch_deg);
    Ok(scene)
}
