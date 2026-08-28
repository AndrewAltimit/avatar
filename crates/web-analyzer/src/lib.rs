//! In-browser FBX analysis + 3D scene extraction for the docs site's Analyzer page.
//!
//! A thin wasm-bindgen shim over the same diagnose graph the CLI uses:
//! [`avatar_fbx`] (parse + mesh extraction), [`avatar_armature`] (humanoid rig
//! check), and [`avatar_stats`] (performance rank). Everything runs client-side
//! on the bytes of a dropped file — nothing is uploaded anywhere.
//!
//! Exported surface:
//! - `analyze_fbx(bytes, name)` — the diagnose report as a JSON string ([`analyze`] is
//!   the pure core so the report shape is testable off-wasm).
//! - `sample_fbx()` — the bytes of the avatar-testkit synthetic humanoid, for a
//!   "try a sample" button.
//! - [`SceneView`] — the geometry of every mesh (positions/normals/uvs/indices/skin),
//!   the bones' world positions, and embedded textures, uprighted the way
//!   `avatar render` draws an avatar, so the page can render it with WebGL.
//!
//! The upright/placement logic mirrors `crates/cli/src/render_scene.rs` but is
//! reimplemented with plain `f32` math ([`math`]) — this crate must not pull `glam`
//! or the wgpu render layer into the wasm bundle.
//!
//! Built for the site with `wasm-pack build crates/web-analyzer --target web`.

use std::collections::HashMap;

use anyhow::Result;
use avatar_armature::{HumanBone, Skeleton};
use avatar_fbx::FbxScene;
use avatar_mesh::RawMesh;
use serde::Serialize;
use wasm_bindgen::prelude::*;

pub mod math;

use math::{Mat4, Quat, Vec3};

// --- analyze_fbx ------------------------------------------------------------------------------

/// The full report the Analyzer page renders. `armature` and `stats` are the
/// same serde shapes the CLI's `--json` output uses.
#[derive(Serialize)]
pub struct Report {
    pub fbx: FbxSummary,
    pub global_settings: GlobalSettingsSummary,
    pub armature: avatar_armature::ArmatureReport,
    pub stats: avatar_stats::PerfReport,
    pub blendshapes: Vec<Blendshape>,
    pub meshes: Vec<MeshSummary>,
    pub materials: Vec<MaterialSummary>,
    pub bone_tree: Vec<BoneTreeEntry>,
}

/// Coarse object counts so the page can show what the file contains at a glance.
#[derive(Serialize)]
pub struct FbxSummary {
    /// FBX format version as reported by the parser, e.g. `7400`.
    pub version: u32,
    pub models: usize,
    pub geometries: usize,
    pub materials: usize,
    pub deformers: usize,
    pub bone_like: usize,
}

/// The file's `GlobalSettings` that bear on unit/orientation problems.
#[derive(Serialize)]
pub struct GlobalSettingsSummary {
    pub unit_scale_factor: Option<f64>,
    pub up_axis: Option<i32>,
    pub front_axis: Option<i32>,
}

/// One blendshape channel (`avatar_fbx::BlendshapeChannel` isn't serde-derived).
#[derive(Serialize)]
pub struct Blendshape {
    pub name: String,
    pub mesh: Option<String>,
    pub group: BlendshapeGroup,
}

/// What a blendshape is for, guessed from its name — lets the page group visemes,
/// blinks and expressions apart from the long tail.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BlendshapeGroup {
    Viseme,
    Blink,
    Expression,
    Other,
}

/// The 15 VRChat viseme names (without the `vrc.v_` prefix).
const VISEMES: [&str; 15] = [
    "sil", "pp", "ff", "th", "dd", "kk", "ch", "ss", "nn", "rr", "aa", "e", "ih", "oh", "ou",
];

const EXPRESSION_WORDS: [&str; 16] = [
    "smile",
    "angry",
    "anger",
    "sad",
    "sorrow",
    "joy",
    "fun",
    "surprised",
    "surprise",
    "happy",
    "laugh",
    "grin",
    "frown",
    "cry",
    "wink_smile",
    "kiss",
];

/// Classify a blendshape channel by name (case-insensitive).
pub fn classify_blendshape(name: &str) -> BlendshapeGroup {
    let lower = name.to_ascii_lowercase();
    let bare = lower.strip_prefix("vrc.v_").unwrap_or(&lower);
    if VISEMES.contains(&bare) {
        return BlendshapeGroup::Viseme;
    }
    if ["blink", "eyeclose", "wink"]
        .iter()
        .any(|w| lower.contains(w))
    {
        return BlendshapeGroup::Blink;
    }
    if EXPRESSION_WORDS.iter().any(|w| lower.contains(w)) {
        return BlendshapeGroup::Expression;
    }
    BlendshapeGroup::Other
}

/// One mesh geometry as `analyze_fbx` reports it (counts only; buffers come from [`SceneView`]).
#[derive(Serialize)]
pub struct MeshSummary {
    pub index: usize,
    pub name: String,
    pub model_id: i64,
    pub vertices: usize,
    pub control_points: usize,
    pub triangles: usize,
    pub skinned: bool,
    pub material_slots: usize,
    pub bones_influencing: usize,
}

/// One material, deduplicated across meshes (see [`MaterialTable`]).
#[derive(Serialize, Clone)]
pub struct MaterialSummary {
    pub index: usize,
    pub name: String,
    pub diffuse_color: Option<[f32; 4]>,
    pub texture: Option<TextureSummary>,
}

#[derive(Serialize, Clone)]
pub struct TextureSummary {
    pub relative: Option<String>,
    pub absolute: Option<String>,
    pub embedded: bool,
    pub embedded_bytes: usize,
}

/// One `Model` node of the hierarchy, for the bone tree view.
#[derive(Serialize)]
pub struct BoneTreeEntry {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
    pub humanoid: Option<&'static str>,
    pub bone_like: bool,
    pub depth: usize,
}

/// Humanoid slot per bone id (only unambiguous assignments count).
fn humanoid_slots(skeleton: &Skeleton) -> HashMap<i64, &'static str> {
    let mapping = avatar_armature::map_humanoid(skeleton);
    HumanBone::ALL
        .iter()
        .filter_map(|&slot| mapping.unique_id(slot).map(|id| (id, slot.name())))
        .collect()
}

/// The global material list: materials are attached per mesh, so the same material shows up on
/// every mesh that uses it. Deduplicate by name + texture path so the page gets one entry per
/// material and each mesh's slots index into it.
struct MaterialTable {
    entries: Vec<MaterialSummary>,
    textures: Vec<Vec<u8>>,
    key_to_index: HashMap<String, usize>,
}

impl MaterialTable {
    fn new() -> Self {
        MaterialTable {
            entries: Vec::new(),
            textures: Vec::new(),
            key_to_index: HashMap::new(),
        }
    }

    fn intern(&mut self, m: &avatar_mesh::MeshMaterial) -> usize {
        let key = format!(
            "{}\u{0}{}\u{0}{}",
            m.name,
            m.texture
                .as_ref()
                .and_then(|t| t.relative.as_deref())
                .unwrap_or(""),
            m.texture
                .as_ref()
                .and_then(|t| t.absolute.as_deref())
                .unwrap_or(""),
        );
        if let Some(&i) = self.key_to_index.get(&key) {
            return i;
        }
        let index = self.entries.len();
        self.entries.push(MaterialSummary {
            index,
            name: m.name.clone(),
            diffuse_color: m.diffuse_color,
            texture: m.texture.as_ref().map(|t| TextureSummary {
                relative: t.relative.clone(),
                absolute: t.absolute.clone(),
                embedded: t.embedded.is_some(),
                embedded_bytes: t.embedded.as_ref().map_or(0, Vec::len),
            }),
        });
        self.textures.push(
            m.texture
                .as_ref()
                .and_then(|t| t.embedded.clone())
                .unwrap_or_default(),
        );
        self.key_to_index.insert(key, index);
        index
    }
}

fn model_name(scene: &FbxScene, id: i64) -> String {
    scene
        .object(id)
        .map(|o| o.name.clone())
        .unwrap_or_else(|| format!("#{id}"))
}

fn bone_tree(scene: &FbxScene) -> Vec<BoneTreeEntry> {
    let skeleton = Skeleton::from_scene(scene);
    let slots = humanoid_slots(&skeleton);
    skeleton
        .bones
        .iter()
        .map(|b| BoneTreeEntry {
            id: b.id,
            name: b.name.clone(),
            parent: b.parent,
            humanoid: slots.get(&b.id).copied(),
            bone_like: scene.object(b.id).is_some_and(|o| o.is_bone_like()),
            depth: skeleton.depth(b.id),
        })
        .collect()
}

/// Parse + analyze one binary FBX. Pure (no fs, no JS types), so unit tests can
/// pin the report against the synthetic testkit corpus.
pub fn analyze(bytes: &[u8], name: &str) -> Result<Report> {
    let doc = avatar_fbx::FbxDocument::from_bytes(bytes)?;
    let scene = doc.scene();

    let count = |node: &str| scene.objects.iter().filter(|o| o.node_name == node).count();
    let fbx = FbxSummary {
        version: scene.version,
        models: count("Model"),
        geometries: count("Geometry"),
        materials: count("Material"),
        deformers: count("Deformer"),
        bone_like: scene.objects.iter().filter(|o| o.is_bone_like()).count(),
    };
    let gs = &scene.global_settings;
    let global_settings = GlobalSettingsSummary {
        unit_scale_factor: gs.unit_scale_factor,
        up_axis: gs.up_axis,
        front_axis: gs.front_axis,
    };

    let armature = avatar_armature::analyze(&scene);
    let stats = avatar_stats::analyze_fbx_bytes(bytes, name)?;
    let blendshapes = scene
        .blendshape_channels()
        .into_iter()
        .map(|c| Blendshape {
            group: classify_blendshape(&c.name),
            name: c.name,
            mesh: c.mesh_model_name,
        })
        .collect();

    let raw = doc.meshes()?;
    let mut table = MaterialTable::new();
    let meshes = raw
        .iter()
        .enumerate()
        .map(|(index, m)| {
            for mat in &m.materials {
                table.intern(mat);
            }
            MeshSummary {
                index,
                name: model_name(&scene, m.model_id),
                model_id: m.model_id,
                vertices: m.vertex_count(),
                control_points: m.control_point_count(),
                triangles: m.indices.len() / 3,
                skinned: m.is_skinned(),
                material_slots: m.material_slot_count(),
                bones_influencing: m.skin.as_ref().map_or(0, |s| s.clusters.len()),
            }
        })
        .collect();

    Ok(Report {
        fbx,
        global_settings,
        armature,
        stats,
        blendshapes,
        meshes,
        materials: table.entries,
        bone_tree: bone_tree(&scene),
    })
}

/// The wasm entry point: bytes + display name in, JSON out. Errors surface to
/// JS as a thrown exception carrying the anyhow message.
#[wasm_bindgen]
pub fn analyze_fbx(bytes: &[u8], name: &str) -> Result<String, JsError> {
    let report = analyze(bytes, name).map_err(|e| JsError::new(&format!("{e:#}")))?;
    serde_json::to_string(&report).map_err(|e| JsError::new(&e.to_string()))
}

/// The avatar-testkit synthetic humanoid skeleton (bones only, no geometry), as
/// binary FBX bytes — the "try a sample" file.
#[wasm_bindgen]
pub fn sample_fbx() -> Vec<u8> {
    avatar_testkit::fbx::humanoid_skinned()
}

// --- SceneView --------------------------------------------------------------------------------

/// Rotation bringing an FBX file's up-axis to Y-up (FBX `UpAxis`: 0=X, 1=Y, 2=Z). Z-up → −90°
/// about X so (x,y,z) ↦ (x, z, −y). Mirrors `render_scene::up_axis_correction`.
pub fn up_axis_correction(up_axis: Option<i32>) -> Quat {
    match up_axis {
        Some(2) => Quat::from_axis_angle(Vec3::X, -std::f32::consts::FRAC_PI_2),
        _ => Quat::IDENTITY,
    }
}

/// Weighted centroid, in raw control-point space, of the vertices a bone's skin cluster drives
/// (mirrors `render_scene::bone_centroid`).
pub fn bone_centroid(meshes: &[RawMesh], bone_id: i64) -> Option<Vec3> {
    for m in meshes {
        let Some(skin) = &m.skin else { continue };
        let Some(c) = skin.clusters.iter().find(|c| c.bone_id == bone_id) else {
            continue;
        };
        let mut cp_pos: HashMap<u32, [f32; 3]> = HashMap::new();
        for (k, &cp) in m.control_point_of_vertex.iter().enumerate() {
            if let Some(&p) = m.positions.get(k) {
                cp_pos.entry(cp).or_insert(p);
            }
        }
        let mut sum = Vec3::ZERO;
        let mut wsum = 0.0f32;
        for (&cp, &w) in c.indexes.iter().zip(&c.weights) {
            if let Some(p) = cp_pos.get(&cp) {
                sum = sum + Vec3::from(*p) * w;
                wsum += w;
            }
        }
        if wsum > 0.0 {
            return Some(sum / wsum);
        }
    }
    None
}

/// A rotation standing the avatar upright in control-point space: align the measured
/// hips→head axis (cluster centroids) to +Y; fall back to the declared up-axis when the rig
/// isn't a recognizable humanoid. Mirrors `render_scene::auto_upright`.
pub fn auto_upright(scene: &FbxScene, meshes: &[RawMesh]) -> Quat {
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
    Quat::from_rotation_arc(up.normalize(), Vec3::Y)
}

/// The global matrix of `Model` node `id`: its Lcl TRS composed with every `Model` ancestor's
/// (mirrors `avatar_pose::model_global_matrix`).
pub fn model_global_matrix(scene: &FbxScene, id: i64) -> Mat4 {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(i) = cur {
        let Some(o) = scene.object(i) else { break };
        if o.class != "Model" {
            break;
        }
        chain.push(math::lcl_to_mat4(&o.transform));
        cur = scene.parent_of(i);
        guard += 1;
        if guard > 1024 {
            break;
        }
    }
    chain.iter().rev().fold(Mat4::IDENTITY, |acc, m| acc.mul(m))
}

/// Smooth per-vertex normals: area-weighted face normals accumulated per vertex (the
/// `avatar_render::compute_normals` algorithm, glam-free).
pub fn compute_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= positions.len() || b >= positions.len() || c >= positions.len() {
            continue;
        }
        let (pa, pb, pc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        let face = (pb - pa).cross(pc - pa);
        normals[a] = normals[a] + face;
        normals[b] = normals[b] + face;
        normals[c] = normals[c] + face;
    }
    normals
        .into_iter()
        .map(|n| n.normalize_or_zero().into())
        .collect()
}

/// Image MIME from magic bytes; TGA has none, so guess it from the file extension.
pub fn texture_mime(bytes: &[u8], paths: [Option<&str>; 2]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        return Some("image/jpeg");
    }
    let is_tga = |p: &str| p.to_ascii_lowercase().ends_with(".tga");
    paths
        .iter()
        .flatten()
        .any(|p| is_tga(p))
        .then_some("image/x-tga")
}

#[derive(Serialize)]
struct Manifest {
    upright: [f32; 4],
    unit_scale_factor: Option<f64>,
    up_axis: Option<i32>,
    bounds: Bounds,
    meshes: Vec<ManifestMesh>,
    materials: Vec<ManifestMaterial>,
    bones: Vec<ManifestBone>,
    blendshapes: Vec<ManifestBlendshape>,
}

#[derive(Serialize, Default)]
struct Bounds {
    min: [f32; 3],
    max: [f32; 3],
}

#[derive(Serialize)]
struct ManifestMesh {
    index: usize,
    name: String,
    model_id: i64,
    vertices: usize,
    triangles: usize,
    skinned: bool,
    material_slots: Vec<usize>,
}

#[derive(Serialize)]
struct ManifestMaterial {
    index: usize,
    name: String,
    diffuse_color: Option<[f32; 4]>,
    texture: Option<ManifestTexture>,
}

#[derive(Serialize)]
struct ManifestTexture {
    relative: Option<String>,
    absolute: Option<String>,
    embedded: bool,
    mime: Option<&'static str>,
}

#[derive(Serialize)]
struct ManifestBone {
    index: usize,
    id: i64,
    name: String,
    parent: Option<usize>,
    humanoid: Option<&'static str>,
    bone_like: bool,
    position: [f32; 3],
    influenced_vertices: usize,
}

#[derive(Serialize)]
struct ManifestBlendshape {
    name: String,
    mesh: Option<usize>,
    group: BlendshapeGroup,
}

/// Flat GPU-ready buffers for one mesh.
#[derive(Default)]
struct MeshBuffers {
    positions: Vec<f32>,
    normals: Vec<f32>,
    uvs: Vec<f32>,
    indices: Vec<u32>,
    triangle_materials: Vec<u32>,
    skin_indices: Vec<u32>,
    skin_weights: Vec<f32>,
}

/// The renderable view of one FBX: per-mesh vertex buffers in uprighted world space, bone
/// positions, embedded textures, and a JSON manifest tying them together.
#[wasm_bindgen]
pub struct SceneView {
    manifest: String,
    meshes: Vec<MeshBuffers>,
    textures: Vec<Vec<u8>>,
}

impl SceneView {
    /// Build the view from FBX bytes (pure; the wasm `load` wraps this).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let doc = avatar_fbx::FbxDocument::from_bytes(bytes)?;
        let scene = doc.scene();
        let raw = doc.meshes()?;

        // Placement, as `avatar render`: skinned meshes are uprighted via the skeleton; static
        // meshes get the file's declared up-axis correction. Geometry is the raw control points.
        let upright = auto_upright(&scene, &raw);
        let static_place = up_axis_correction(scene.global_settings.up_axis);

        // Bones: every Model, in skeleton order; id → index for the skin buffers.
        let skeleton = Skeleton::from_scene(&scene);
        let slots = humanoid_slots(&skeleton);
        let bone_index: HashMap<i64, usize> = skeleton
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| (b.id, i))
            .collect();
        let mut influenced = vec![0usize; skeleton.bones.len()];

        let mut table = MaterialTable::new();
        let mut meshes = Vec::with_capacity(raw.len());
        let mut manifest_meshes = Vec::with_capacity(raw.len());
        let mut bounds: Option<(Vec3, Vec3)> = None;
        let mut grow = |p: Vec3| {
            bounds = Some(match bounds {
                None => (p, p),
                Some((lo, hi)) => (lo.min(p), hi.max(p)),
            });
        };

        for (index, m) in raw.iter().enumerate() {
            let place = if m.is_skinned() {
                upright
            } else {
                static_place
            };
            let world: Vec<[f32; 3]> = m
                .positions
                .iter()
                .map(|&p| place.rotate(Vec3::from(p)).into())
                .collect();
            for &p in &world {
                grow(Vec3::from(p));
            }
            let normals: Vec<[f32; 3]> = match &m.normals {
                Some(n) if n.len() == m.positions.len() => n
                    .iter()
                    .map(|&n| place.rotate(Vec3::from(n)).normalize_or_zero().into())
                    .collect(),
                _ => compute_normals(&world, &m.indices),
            };
            let uvs: Vec<f32> = m
                .uvs
                .as_ref()
                .filter(|u| u.len() == m.positions.len())
                .map(|u| u.iter().flat_map(|uv| [uv[0], uv[1]]).collect())
                .unwrap_or_default();

            let (skin_indices, skin_weights) = vertex_skin(m, &bone_index, &mut influenced);

            let material_slots: Vec<usize> =
                m.materials.iter().map(|mat| table.intern(mat)).collect();

            meshes.push(MeshBuffers {
                positions: world.iter().flatten().copied().collect(),
                normals: normals.iter().flatten().copied().collect(),
                uvs,
                indices: m.indices.clone(),
                triangle_materials: m.material_of_triangle.clone(),
                skin_indices,
                skin_weights,
            });
            manifest_meshes.push(ManifestMesh {
                index,
                name: model_name(&scene, m.model_id),
                model_id: m.model_id,
                vertices: m.vertex_count(),
                triangles: m.indices.len() / 3,
                skinned: m.is_skinned(),
                material_slots,
            });
        }

        // Bone world positions: cluster centroid when the bone skins something (the same space
        // as the control points), else the composed Model transform chain — both uprighted.
        let bones: Vec<ManifestBone> = skeleton
            .bones
            .iter()
            .enumerate()
            .map(|(index, b)| {
                let raw_pos = bone_centroid(&raw, b.id)
                    .unwrap_or_else(|| model_global_matrix(&scene, b.id).translation());
                let position = upright.rotate(raw_pos);
                ManifestBone {
                    index,
                    id: b.id,
                    name: b.name.clone(),
                    parent: b.parent.and_then(|p| bone_index.get(&p).copied()),
                    humanoid: slots.get(&b.id).copied(),
                    bone_like: scene.object(b.id).is_some_and(|o| o.is_bone_like()),
                    position: position.into(),
                    influenced_vertices: influenced[index],
                }
            })
            .collect();
        if meshes.is_empty() {
            // Nothing but a skeleton: frame the camera on the bones instead.
            for b in &bones {
                grow(Vec3::from(b.position));
            }
        }

        // Blendshapes → mesh index via the channel's geometry's Model.
        let mesh_of_model: HashMap<i64, usize> = raw
            .iter()
            .enumerate()
            .map(|(i, m)| (m.model_id, i))
            .collect();
        let blendshapes = scene
            .blendshape_channels()
            .into_iter()
            .map(|c| ManifestBlendshape {
                group: classify_blendshape(&c.name),
                mesh: c
                    .geometry_id
                    .and_then(|g| scene.parent_of(g))
                    .and_then(|m| mesh_of_model.get(&m).copied()),
                name: c.name,
            })
            .collect();

        let materials = table
            .entries
            .iter()
            .zip(&table.textures)
            .map(|(e, bytes)| ManifestMaterial {
                index: e.index,
                name: e.name.clone(),
                diffuse_color: e.diffuse_color,
                texture: e.texture.as_ref().map(|t| ManifestTexture {
                    relative: t.relative.clone(),
                    absolute: t.absolute.clone(),
                    embedded: t.embedded,
                    mime: texture_mime(bytes, [t.relative.as_deref(), t.absolute.as_deref()]),
                }),
            })
            .collect();

        let manifest = Manifest {
            upright: upright.into(),
            unit_scale_factor: scene.global_settings.unit_scale_factor,
            up_axis: scene.global_settings.up_axis,
            bounds: bounds.map_or_else(Bounds::default, |(lo, hi)| Bounds {
                min: lo.into(),
                max: hi.into(),
            }),
            meshes: manifest_meshes,
            materials,
            bones,
            blendshapes,
        };
        Ok(SceneView {
            manifest: serde_json::to_string(&manifest)?,
            meshes,
            textures: table.textures,
        })
    }

    fn mesh(&self, i: u32) -> Option<&MeshBuffers> {
        self.meshes.get(i as usize)
    }
}

/// Per-emitted-vertex top-4 bone influences (indices into the manifest bone list + normalized
/// weights), from the control-point-keyed clusters. Unskinned meshes get all-zero weights.
/// Also counts, per bone, the emitted vertices it influences.
fn vertex_skin(
    m: &RawMesh,
    bone_index: &HashMap<i64, usize>,
    influenced: &mut [usize],
) -> (Vec<u32>, Vec<f32>) {
    let n = m.positions.len();
    let mut indices = vec![0u32; n * 4];
    let mut weights = vec![0f32; n * 4];
    let Some(skin) = &m.skin else {
        return (indices, weights);
    };
    // control point → [(bone index, weight)]
    let mut per_cp: HashMap<u32, Vec<(usize, f32)>> = HashMap::new();
    for c in &skin.clusters {
        let Some(&bi) = bone_index.get(&c.bone_id) else {
            continue;
        };
        for (&cp, &w) in c.indexes.iter().zip(&c.weights) {
            if w > 0.0 {
                per_cp.entry(cp).or_default().push((bi, w));
            }
        }
    }
    for (k, &cp) in m.control_point_of_vertex.iter().enumerate().take(n) {
        let Some(list) = per_cp.get(&cp) else {
            continue;
        };
        let mut top: Vec<(usize, f32)> = list.clone();
        top.sort_by(|a, b| b.1.total_cmp(&a.1));
        top.truncate(4);
        let sum: f32 = top.iter().map(|t| t.1).sum();
        if sum <= 0.0 {
            continue;
        }
        for (j, (bi, w)) in top.iter().enumerate() {
            indices[k * 4 + j] = *bi as u32;
            weights[k * 4 + j] = w / sum;
            influenced[*bi] += 1;
        }
    }
    (indices, weights)
}

#[wasm_bindgen]
impl SceneView {
    /// Parse FBX bytes into a renderable scene. Throws on a malformed file.
    #[wasm_bindgen]
    pub fn load(bytes: &[u8]) -> Result<SceneView, JsError> {
        SceneView::from_bytes(bytes).map_err(|e| JsError::new(&format!("{e:#}")))
    }

    /// The JSON manifest: upright quaternion, bounds, meshes, materials, bones, blendshapes.
    pub fn manifest(&self) -> String {
        self.manifest.clone()
    }

    /// Flat xyz per emitted vertex, in uprighted world space.
    pub fn positions(&self, mesh: u32) -> Vec<f32> {
        self.mesh(mesh)
            .map(|m| m.positions.clone())
            .unwrap_or_default()
    }

    /// Flat xyz normals per vertex (computed when the file has none).
    pub fn normals(&self, mesh: u32) -> Vec<f32> {
        self.mesh(mesh)
            .map(|m| m.normals.clone())
            .unwrap_or_default()
    }

    /// Flat uv per vertex; empty if the mesh has no UV layer.
    pub fn uvs(&self, mesh: u32) -> Vec<f32> {
        self.mesh(mesh).map(|m| m.uvs.clone()).unwrap_or_default()
    }

    /// Triangle index buffer.
    pub fn indices(&self, mesh: u32) -> Vec<u32> {
        self.mesh(mesh)
            .map(|m| m.indices.clone())
            .unwrap_or_default()
    }

    /// Per-triangle material slot (index into the mesh's `material_slots`); empty ⇒ all slot 0.
    pub fn triangle_materials(&self, mesh: u32) -> Vec<u32> {
        self.mesh(mesh)
            .map(|m| m.triangle_materials.clone())
            .unwrap_or_default()
    }

    /// Four bone indices per vertex (into `manifest.bones`), padded with 0.
    pub fn skin_indices(&self, mesh: u32) -> Vec<u32> {
        self.mesh(mesh)
            .map(|m| m.skin_indices.clone())
            .unwrap_or_default()
    }

    /// Four normalized weights per vertex (top-4 influences), padded with 0.
    pub fn skin_weights(&self, mesh: u32) -> Vec<f32> {
        self.mesh(mesh)
            .map(|m| m.skin_weights.clone())
            .unwrap_or_default()
    }

    /// Embedded image bytes of a material's texture, or empty.
    pub fn texture(&self, material: u32) -> Vec<u8> {
        self.textures
            .get(material as usize)
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
