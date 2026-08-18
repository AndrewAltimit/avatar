//! Glue between the importers (FBX/glTF) and the `avatar-render` GPU layer: load an avatar's
//! geometry at rest pose into a renderable [`Scene`]. World-scene loading is added alongside this.

use std::path::Path;

use anyhow::{Context, Result, bail};
use avatar_armature::{HumanBone, Skeleton};
use avatar_mesh::RawMesh;
use avatar_render::{Camera, Light, RenderMesh, Scene, Texture};
use glam::{Mat4, Quat, Vec3};

use crate::texture::{SlotStyle, TextureSet, split_by_material};

/// A chain-length preview: scale the offsets of every bone *below* the bones matching `hinge`
/// (a name, `*` wildcards allowed — `Skirt_0_*`) by `factor`, and let the skinned mesh follow.
/// This is exactly the edit `avatar physbone stretch` makes to a prefab, previewed on the FBX
/// before any Unity round trip.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneStretch {
    pub hinge: String,
    pub factor: f32,
}

impl BoneStretch {
    /// Parse `HINGE:FACTOR` (e.g. `Skirt_0_*:1.5`).
    pub fn parse(s: &str) -> Result<Self> {
        let Some((h, f)) = s.rsplit_once(':') else {
            bail!("--stretch '{s}' is not HINGE:FACTOR (e.g. 'Skirt_0_*:1.5')");
        };
        let factor: f32 = f
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("--stretch factor '{f}' is not a number"))?;
        if !(factor.is_finite() && factor > 0.0) {
            bail!("--stretch factor must be > 0 (got {factor})");
        }
        let hinge = h.trim().to_string();
        if hinge.is_empty() {
            bail!("--stretch '{s}' names no bone");
        }
        Ok(BoneStretch { hinge, factor })
    }

    fn matches(&self, name: &str) -> bool {
        glob_match(&self.hinge, name)
    }
}

/// `*`-wildcard match (no other metacharacters).
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if part.is_empty() {
            continue;
        } else {
            let Some(pos) = rest.find(part) else {
                return false;
            };
            rest = &rest[pos + part.len()..];
        }
    }
    true
}

/// How to pose an FBX avatar for a preview render, instead of its rest/bind pose. Both are the
/// prefab-side edits (`avatar physbone stretch|flare|…`) seen from the FBX: `stretch` re-applies a
/// chain stretch by hinge name; `pose_prefab` takes every bone's local TRS from a Unity prefab
/// (matched by GameObject name), so *whatever* the prefab's transforms say — a stretched skirt,
/// re-angled chains, hand-posed bones — is what gets drawn.
#[derive(Debug, Clone, Default)]
pub struct AvatarPose {
    pub stretch: Vec<BoneStretch>,
    pub pose_prefab: Option<std::path::PathBuf>,
}

impl AvatarPose {
    fn is_rest(&self) -> bool {
        self.stretch.is_empty() && self.pose_prefab.is_none()
    }
}

/// Bone-name → local TRS (Unity space) read from a prefab, for [`apply_pose`].
fn prefab_locals(
    path: &Path,
) -> Result<std::collections::HashMap<String, avatar_migrate::math::Trs>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading --pose prefab {}", path.display()))?;
    let rw = avatar_migrate::rewrite::PrefabRewriter::new(&text)
        .with_context(|| format!("parsing --pose prefab {}", path.display()))?;
    let scene = rw.scene();
    let mut by_name: std::collections::HashMap<String, Vec<avatar_migrate::math::Trs>> =
        std::collections::HashMap::new();
    for tr in scene.transforms.values() {
        let name = scene.name_of_transform(tr.file_id).to_string();
        by_name.entry(name).or_default().push(tr.local);
    }
    // Ambiguous names (several objects) are skipped rather than guessed.
    Ok(by_name
        .into_iter()
        .filter_map(|(n, v)| (v.len() == 1).then(|| (n, v[0])))
        .collect())
}

/// Pose skinned FBX meshes per [`AvatarPose`] and CPU-skin each mesh's raw control points through
/// the resulting palette. Only the **delta** from rest is applied — per bone `G⁻¹ · world(pose) ·
/// world(rest)⁻¹ · G`, `G` the mesh node's global transform (the space its raw control points live
/// in) — so untouched geometry is byte-identical to the rest render and the untrusted per-cluster
/// bind `Transform`s are never used. Returns how many bones were changed.
///
/// A prefab pose is Unity's mirrored copy of the FBX hierarchy: Unity negates X on import, which
/// maps a local `(p, q)` to `((-p.x, p.y, p.z), (q.x, -q.y, -q.z, q.w))`; positions are divided by
/// the file's import scale (`UnitScaleFactor / 100`). Bones are matched by GameObject name.
fn apply_pose(
    scene: &avatar_fbx::FbxScene,
    meshes: &mut [RawMesh],
    how: &AvatarPose,
) -> Result<usize> {
    use avatar_pose::{PosedSkeleton, cpu_skin, model_global_matrix};
    let skeleton = Skeleton::from_scene(scene);
    // Bone id → stretch factor for every bone strictly below a matching hinge.
    let mut factor_of: std::collections::HashMap<i64, f32> = std::collections::HashMap::new();
    for st in &how.stretch {
        let hinges: Vec<i64> = skeleton
            .bones
            .iter()
            .filter(|b| st.matches(&b.name))
            .map(|b| b.id)
            .collect();
        for b in &skeleton.bones {
            let mut cur = b.parent;
            while let Some(p) = cur {
                if hinges.contains(&p) {
                    factor_of.insert(b.id, st.factor);
                    break;
                }
                cur = skeleton.bone(p).and_then(|pb| pb.parent);
            }
        }
    }
    if !how.stretch.is_empty() && factor_of.is_empty() {
        bail!(
            "--stretch matched no bone below {}",
            how.stretch
                .iter()
                .map(|s| format!("'{}'", s.hinge))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    // Bone id → local TRS from the prefab (FBX convention).
    let mut prefab_local: std::collections::HashMap<i64, (Vec3, Quat, Vec3)> =
        std::collections::HashMap::new();
    if let Some(p) = &how.pose_prefab {
        let locals = prefab_locals(p)?;
        // Unity import scale: UnitScaleFactor/100 (FBX in cm → 0.01). FBX-space = Unity / that.
        let import_scale = scene.global_settings.unit_scale_factor.unwrap_or(1.0) / 100.0;
        for b in &skeleton.bones {
            if let Some(t) = locals.get(&b.name) {
                let pos = Vec3::new(
                    (-t.position.x / import_scale) as f32,
                    (t.position.y / import_scale) as f32,
                    (t.position.z / import_scale) as f32,
                );
                let rot = Quat::from_xyzw(
                    t.rotation.x as f32,
                    -t.rotation.y as f32,
                    -t.rotation.z as f32,
                    t.rotation.w as f32,
                )
                .normalize();
                let scl = Vec3::new(t.scale.x as f32, t.scale.y as f32, t.scale.z as f32);
                prefab_local.insert(b.id, (pos, rot, scl));
            }
        }
        if prefab_local.is_empty() {
            bail!(
                "--pose {}: no bone name matches an object in the prefab",
                p.display()
            );
        }
    }
    let changed = factor_of.len().max(prefab_local.len());
    for m in meshes.iter_mut().filter(|m| m.is_skinned()) {
        let posed = PosedSkeleton::from_fbx(&skeleton, scene, m);
        let mut pose = posed.rest_pose();
        for (id, (t, r, s)) in &prefab_local {
            if let Some(i) = posed.index_of(*id) {
                pose.set_local_trs(i, *t, *r, *s);
            }
        }
        for (id, f) in &factor_of {
            if let Some(i) = posed.index_of(*id) {
                let (s, r, t) = pose.local[i].to_scale_rotation_translation();
                pose.set_local_trs(i, t * *f, r, s);
            }
        }
        let skin = posed.build_vertex_skin(m);
        // The mesh is drawn from its raw control points, which live in the mesh node's own space
        // (`G`, e.g. Blender's -90° X on the mesh object). The pose is a world-space change per
        // bone (`posed · rest⁻¹`), so conjugate it into mesh space: `G⁻¹ · posed · rest⁻¹ · G`.
        // Identity everywhere the pose is untouched. (The FBX cluster `Transform`s — the bind
        // palette proper — are deliberately not used: converted avatars ship them inconsistent,
        // which is why the renderer draws raw control points in the first place.)
        let g = model_global_matrix(scene, m.model_id);
        let g_inv = g.inverse();
        let rest = posed.world_matrices(&posed.rest_pose());
        let palette: Vec<Mat4> = posed
            .world_matrices(&pose)
            .iter()
            .zip(&rest)
            .map(|(p, r)| g_inv * *p * r.inverse() * g)
            .collect();
        m.positions = cpu_skin(m, &skin, &palette);
    }
    Ok(changed)
}

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
            // `control_point_of_vertex` is parallel to `positions`; guard against a malformed mesh
            // where it isn't, rather than panicking on an out-of-range index.
            if let Some(&p) = m.positions.get(k) {
                cp_pos.entry(cp).or_insert(p);
            }
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
    how: &AvatarPose,
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
            let mut meshes = doc.meshes()?;
            if meshes.is_empty() {
                bail!("no meshes found in {}", path.display());
            }
            if !how.is_rest() {
                let changed = apply_pose(&scene, &mut meshes, how)?;
                println!("pose: {changed} bone(s) posed from the prefab / stretch");
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
            if !how.is_rest() {
                bail!("--stretch / --pose are supported for .fbx avatars only");
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
pub fn load_avatar(path: &Path, tex: &mut TextureSet, how: &AvatarPose) -> Result<Vec<RenderMesh>> {
    load_avatar_placed(path, Mat4::IDENTITY, tex, how)
}

/// Standing height (metres) an avatar is normalised to when dropped into a world, so a model
/// authored in any unit system (FBX cm, MMD metres × 8, …) stands at human scale beside the map.
const AVATAR_HEIGHT_M: f32 = 1.6;

/// World-space axis-aligned bounds of a placed mesh list (each vertex through its `transform`).
pub fn mesh_bounds(meshes: &[RenderMesh]) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for m in meshes {
        for p in &m.positions {
            let w = m.transform.transform_point3(Vec3::from(*p));
            min = min.min(w);
            max = max.max(w);
        }
    }
    min.is_finite().then_some((min, max))
}

/// Load an avatar and drop it into a world at `spawn` (a renderer-space point — see
/// [`crate::world::WorldLoad::spawn`]), the way VRChat materialises a player there. The avatar is
/// normalised to [`AVATAR_HEIGHT_M`] regardless of its authored units and stood with its feet on the
/// spawn point. Returns the placed meshes plus their world-space bounds, so the caller can frame the
/// camera on the avatar rather than on the whole map.
pub fn load_avatar_in_world(
    path: &Path,
    spawn: Vec3,
    tex: &mut TextureSet,
    how: &AvatarPose,
) -> Result<(Vec<RenderMesh>, (Vec3, Vec3))> {
    // Upright-local geometry at the origin; re-placed once we know its size.
    let mut meshes = load_avatar_placed(path, Mat4::IDENTITY, tex, how)?;
    let (min, max) =
        mesh_bounds(&meshes).ok_or_else(|| anyhow::anyhow!("avatar has no geometry"))?;
    let scale = AVATAR_HEIGHT_M / (max.y - min.y).max(1e-4);
    let foot = Vec3::new((min.x + max.x) * 0.5, min.y, (min.z + max.z) * 0.5);
    // feet-centre → spawn, uniformly scaled to human height.
    let place = Mat4::from_translation(spawn)
        * Mat4::from_scale(Vec3::splat(scale))
        * Mat4::from_translation(-foot);
    for m in &mut meshes {
        m.transform = place * m.transform;
    }
    let bounds = mesh_bounds(&meshes)
        .ok_or_else(|| anyhow::anyhow!("no geometry remained after placement"))?;
    Ok((meshes, bounds))
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

/// Assemble a [`Scene`] from meshes + the resolved texture pool, auto-framing the camera.
///
/// `focus`, when given, is the bounding box the camera frames on (e.g. an avatar dropped into a
/// world, so it fills the shot with the map visible around it); otherwise the camera frames every
/// mesh in the scene.
pub fn scene_from_meshes(
    meshes: Vec<RenderMesh>,
    textures: Vec<Texture>,
    width: u32,
    height: u32,
    yaw_deg: f32,
    pitch_deg: f32,
    focus: Option<(Vec3, Vec3)>,
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
    let (min, max) = focus
        .or_else(|| scene.world_bounds())
        .ok_or_else(|| anyhow::anyhow!("scene has no finite geometry bounds to frame"))?;
    scene.camera = Camera::frame_bounds(min, max, width as f32 / height as f32, yaw_deg, pitch_deg);
    Ok(scene)
}

/// Grow a bounding box about its centre by `factor` — used to pull the camera back from a focused
/// avatar so the surrounding map is visible in the frame.
pub fn expand_bounds((min, max): (Vec3, Vec3), factor: f32) -> (Vec3, Vec3) {
    let center = (min + max) * 0.5;
    let half = (max - min) * 0.5 * factor;
    (center - half, center + half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_spec_parses() {
        let s = BoneStretch::parse("Skirt_0_*:1.5").unwrap();
        assert_eq!(s.hinge, "Skirt_0_*");
        assert_eq!(s.factor, 1.5);
        assert!(BoneStretch::parse("Skirt").is_err());
        assert!(BoneStretch::parse("Skirt:0").is_err());
        assert!(BoneStretch::parse("Skirt:x").is_err());
        assert!(BoneStretch::parse(":2").is_err());
    }

    #[test]
    fn glob_matches_star_only() {
        assert!(glob_match("Skirt_0_*", "Skirt_0_7"));
        assert!(!glob_match("Skirt_0_*", "Skirt_1_7"));
        assert!(glob_match("Hair_1", "Hair_1"));
        assert!(!glob_match("Hair_1", "Hair_10"));
        assert!(glob_match("*Tail*", "ButtTail1"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("A*B*C", "AxxBxC"));
        assert!(!glob_match("A*B*C", "AxxBx"));
    }
}
