//! Glue between the importers (FBX/glTF) and the `avatar-render` GPU layer: load an avatar's
//! geometry at rest pose into a renderable [`Scene`]. World-scene loading is added alongside this.

use std::path::Path;

use anyhow::{Result, bail};
use avatar_armature::{HumanBone, Skeleton};
use avatar_mesh::RawMesh;
use avatar_render::{Camera, Light, RenderMesh, Scene, Texture};
use glam::{Mat4, Quat, Vec3};

use crate::texture::{SlotStyle, TextureSet, split_by_material};

/// Tint used for a textured slot when the material declares no diffuse colour — lets the texture's
/// own colours show through unmodulated.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

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

/// Load an avatar (FBX or glTF/GLB) into render meshes at rest/bind pose, placed by `extra`
/// (identity for a standalone avatar; a spawn transform when dropped into a world).
///
/// Geometry is the raw control points — deliberately **not** the FBX skin-bind transforms. Ripped/
/// converted avatars (notably MMD→FBX) routinely ship inconsistent per-cluster `Transform`s, so
/// linear-blend skinning blends opposing rotations and the mesh collapses into spikes. Raw control
/// points are always a clean, undeformed bind; orientation is recovered by [`auto_upright`], which
/// measures the hips→head axis in this same space. Each mesh is split by material so its texture and
/// diffuse tint (from the FBX-embedded materials) come through; meshes without materials fall back to
/// a per-submesh palette colour so parts stay visually distinct.
pub fn load_avatar_placed(
    path: &Path,
    extra: Mat4,
    tex: &mut TextureSet,
) -> Result<Vec<RenderMesh>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let fbx_dir = path.parent().unwrap_or_else(|| Path::new("."));

    // (mesh, world placement) pairs, importer-specific.
    let (meshes, places): (Vec<RawMesh>, Vec<Mat4>) = match ext.as_str() {
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
            let places = meshes
                .iter()
                .map(|m| {
                    if m.is_skinned() {
                        skinned_place
                    } else {
                        static_place
                    }
                })
                .collect();
            (meshes, places)
        }
        // glTF is defined as Y-up; meshes come back in usable space.
        "gltf" | "glb" => {
            let meshes = avatar_gltf::GltfDocument::import(path)?.meshes();
            if meshes.is_empty() {
                bail!("no meshes found in {}", path.display());
            }
            let places = vec![extra; meshes.len()];
            (meshes, places)
        }
        other => bail!("unsupported avatar format '.{other}' (expected .fbx, .gltf, or .glb)"),
    };

    let mut out = Vec::new();
    for (i, (m, place)) in meshes.iter().zip(places).enumerate() {
        if m.positions.is_empty() || m.indices.is_empty() {
            continue;
        }
        let fallback = color_for(i);
        let style = |slot: usize| -> SlotStyle {
            let mat = m.materials.get(slot);
            let texture = mat.and_then(|mm| tex.resolve_fbx_material(fbx_dir, mm));
            let color = mat
                .and_then(|mm| mm.diffuse_color)
                .unwrap_or(if texture.is_some() { WHITE } else { fallback });
            SlotStyle { texture, color }
        };
        out.extend(split_by_material(m, place, style));
    }
    if out.is_empty() {
        bail!("no renderable meshes in {}", path.display());
    }
    Ok(out)
}

/// Load an avatar at the origin (convenience for the standalone-avatar case).
pub fn load_avatar(path: &Path, tex: &mut TextureSet) -> Result<Vec<RenderMesh>> {
    load_avatar_placed(path, Mat4::IDENTITY, tex)
}

/// Unity is left-handed (X right, Y up, Z forward); the renderer is right-handed. Negating Z
/// converts world geometry (and a co-placed avatar) into the renderer's space.
pub fn unity_to_renderer() -> Mat4 {
    Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0))
}

/// Load a world/map scene (a `.unity` file or a project dir) into placed render meshes.
pub fn load_world(path: &Path, tex: &mut TextureSet) -> Result<crate::world::WorldLoad> {
    crate::world::load(path, unity_to_renderer(), tex)
}

/// Assemble a [`Scene`] from meshes + the resolved texture pool, auto-framing the camera to bounds.
pub fn scene_from_meshes(
    meshes: Vec<RenderMesh>,
    textures: Vec<Texture>,
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
        textures,
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
