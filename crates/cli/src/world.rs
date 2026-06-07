//! Parse a Unity `.unity` scene into placed render meshes, emulating enough of Unity's FBX import
//! pipeline to place geometry at the right scale and assemble multi-mesh / prefab-instanced models.
//!
//! What this resolves (the parts of Unity's importer that matter for a static preview):
//! - **FBX node-world transforms.** `avatar_fbx::meshes()` returns each mesh in its own geometry
//!   space; a multi-mesh FBX (e.g. a whole building) only assembles once each mesh is placed by its
//!   `Model` node's transform, composed up the FBX `OO` parent chain. See [`fbx_node_world`].
//! - **Prefab instances** (class 1001). The visible meshes of an instanced model (the cabin shell
//!   here) are *not* serialized into the scene — only a stripped placeholder plus the instance's
//!   root override (`m_Modification`). We resolve `m_SourcePrefab` → FBX and re-instantiate every
//!   mesh at `world(m_TransformParent) · root_local · import_scale · node_world`. The model-prefab
//!   root scale (the **import scale**, Unity's `useFileScale` × `globalScale` × `UnitScaleFactor`/100)
//!   is a prefab *default* and never appears in the scene, so we derive it from the FBX's unit scale.
//! - **Directly-placed `MeshFilter`s** (class 33) keep using the **raw** mesh geometry at their
//!   GameObject's transform — that is exactly what Unity does when a shared sub-mesh is assigned to a
//!   plain GameObject (the source node's transform is *not* reapplied).
//! - **Materials.** Each renderer's `m_Materials` slots are resolved to their `.mat`'s base colour
//!   (`_Color`) and base **texture** (`_MainTex` → asset → decoded pixels); meshes are split per
//!   material slot so each slot draws with its own texture/tint. Prefab-instanced models have no
//!   scene-side material, so their FBX-embedded materials/textures are used instead.
//!
//! Unity-space (left-handed, Y-up) is converted to the renderer's right-handed space by the caller's
//! `extra` transform (a Z-negate). Remaining fidelity gaps are documented in `docs/reference/render.md`
//! (FBX pivots/pre-rotation, geometric transforms, per-platform import overrides).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_mesh::RawMesh;
use avatar_render::RenderMesh;
use avatar_unity_yaml::{UnityFile, Yaml, field_f64, field_i64, field_str};
use glam::{EulerRot, Mat4, Quat, Vec3};

use crate::texture::{SlotStyle, TextureSet, split_by_material};

/// Tint for a textured slot with no `_Color`/diffuse colour — texture colours show unmodulated.
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// A transform node distilled from a Unity `Transform`.
struct Node {
    pos: Vec3,
    rot: Quat,
    scale: Vec3,
    father: i64,
}

/// Outcome counts for the caller to report.
pub struct WorldLoad {
    pub meshes: Vec<RenderMesh>,
    /// Directly-placed `MeshFilter`s rendered.
    pub placed: usize,
    /// Prefab instances expanded into geometry.
    pub placed_prefabs: usize,
    pub skipped_builtin: usize,
    pub skipped_unresolved: usize,
    /// Where VRChat would materialise a player, in **renderer space** (already × `extra`), if the
    /// scene declares one — the natural place to drop a test avatar. See [`find_spawn`].
    pub spawn: Option<Vec3>,
}

fn ref_fileid(node: &Yaml, key: &str) -> Option<i64> {
    field_i64(&node[key], "fileID")
}

fn ref_guid(node: &Yaml, key: &str) -> Option<String> {
    field_str(&node[key], "guid").map(|s| s.to_string())
}

fn read_vec3(node: &Yaml, default: f32) -> Vec3 {
    Vec3::new(
        field_f64(node, "x").map(|v| v as f32).unwrap_or(default),
        field_f64(node, "y").map(|v| v as f32).unwrap_or(default),
        field_f64(node, "z").map(|v| v as f32).unwrap_or(default),
    )
}

fn read_quat(node: &Yaml) -> Quat {
    let x = field_f64(node, "x").unwrap_or(0.0) as f32;
    let y = field_f64(node, "y").unwrap_or(0.0) as f32;
    let z = field_f64(node, "z").unwrap_or(0.0) as f32;
    let w = field_f64(node, "w").unwrap_or(1.0) as f32;
    let q = Quat::from_xyzw(x, y, z, w);
    if q.length() < 1e-6 {
        Quat::IDENTITY
    } else {
        q.normalize()
    }
}

/// Resolve the scene file to render and the project root containing it.
fn resolve_scene(path: &Path) -> Result<(PathBuf, PathBuf)> {
    let scene = if path.is_file() {
        path.to_path_buf()
    } else if path.is_dir() {
        // Prefer Assets/Scene.unity, else the shallowest .unity under Assets/.
        let preferred = path.join("Assets/Scene.unity");
        if preferred.is_file() {
            preferred
        } else {
            find_shallowest_scene(&path.join("Assets"))
                .or_else(|| find_shallowest_scene(path))
                .with_context(|| format!("no .unity scene found under {}", path.display()))?
        }
    } else {
        bail!("world path not found: {}", path.display());
    };
    let root = project_root_of(&scene)
        .with_context(|| format!("no Unity project (Assets/) above {}", scene.display()))?;
    Ok((scene, root))
}

/// The `.unity` file nearest the top of the tree (fewest path components), to favour a main scene.
fn find_shallowest_scene(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    let mut best_depth = usize::MAX;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "unity") {
                let depth = p.components().count();
                if depth < best_depth {
                    best_depth = depth;
                    best = Some(p);
                }
            }
        }
    }
    best
}

/// Walk upward to the directory that contains an `Assets/` folder.
fn project_root_of(scene: &Path) -> Option<PathBuf> {
    let mut dir = scene.parent();
    while let Some(d) = dir {
        if d.join("Assets").is_dir() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Build a `guid -> asset path` index from every `.meta` under the project's `Assets/`.
fn build_guid_index(root: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    let mut stack = vec![root.join("Assets")];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "meta")
                && let Ok(text) = std::fs::read_to_string(&p)
                && let Some(guid) = avatar_unity_yaml::meta_guid(&text)
            {
                // The asset is the .meta path without the trailing ".meta".
                index.insert(guid, p.with_extension(""));
            }
        }
    }
    index
}

/// World matrix for a scene transform, composing up the father chain (memoized, cycle-guarded).
fn world_matrix(id: i64, nodes: &HashMap<i64, Node>, cache: &mut HashMap<i64, Mat4>) -> Mat4 {
    if let Some(m) = cache.get(&id) {
        return *m;
    }
    // Insert identity first as a cycle breaker.
    cache.insert(id, Mat4::IDENTITY);
    let Some(n) = nodes.get(&id) else {
        return Mat4::IDENTITY;
    };
    let local = Mat4::from_scale_rotation_translation(n.scale, n.rot, n.pos);
    let m = if n.father != 0 && nodes.contains_key(&n.father) {
        world_matrix(n.father, nodes, cache) * local
    } else {
        local
    };
    cache.insert(id, m);
    m
}

/// Up-axis correction (Z-up → Y-up) for a static FBX, matching `render_scene`'s convention.
fn up_axis_correction(up_axis: Option<i32>) -> Mat4 {
    match up_axis {
        Some(2) => Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        _ => Mat4::IDENTITY,
    }
}

/// One FBX node's local transform (`Lcl` Translation/Rotation/Scaling) as a matrix. Rotation is FBX
/// Euler degrees; we assume the default `XYZ` rotation order (we don't read per-node `RotationOrder`,
/// pre/post-rotation, or pivots — uncommon on static world props; see the module docs' fidelity note).
fn fbx_local_matrix(t: &avatar_fbx::LocalTransform) -> Mat4 {
    let tr = t.translation.unwrap_or([0.0; 3]);
    let ro = t.rotation.unwrap_or([0.0; 3]);
    let sc = t.scaling.unwrap_or([1.0; 3]);
    let translation = Vec3::new(tr[0] as f32, tr[1] as f32, tr[2] as f32);
    let rotation = Quat::from_euler(
        EulerRot::XYZ,
        (ro[0] as f32).to_radians(),
        (ro[1] as f32).to_radians(),
        (ro[2] as f32).to_radians(),
    );
    let scale = Vec3::new(sc[0] as f32, sc[1] as f32, sc[2] as f32);
    Mat4::from_scale_rotation_translation(scale, rotation, translation)
}

/// FBX-internal world transform of a `Model` node: its local matrix composed up the `OO` parent
/// chain of `Model`s. Memoized and cycle-guarded. Geometry-only roots (parent not a `Model`) stop.
fn fbx_node_world(
    scene: &avatar_fbx::FbxScene,
    model_id: i64,
    cache: &mut HashMap<i64, Mat4>,
) -> Mat4 {
    if let Some(m) = cache.get(&model_id) {
        return *m;
    }
    cache.insert(model_id, Mat4::IDENTITY); // cycle breaker
    let Some(obj) = scene.object(model_id) else {
        return Mat4::IDENTITY;
    };
    let local = fbx_local_matrix(&obj.transform);
    let m = match scene.parent_of(model_id) {
        Some(p) if scene.object(p).is_some_and(|o| o.is_model()) => {
            fbx_node_world(scene, p, cache) * local
        }
        _ => local,
    };
    cache.insert(model_id, m);
    m
}

/// An imported FBX, ready to place: each mesh paired with its FBX-internal world transform (in
/// Unity space, i.e. with up-axis correction applied) and the FBX's import scale.
struct FbxAsset {
    meshes: Vec<RawMesh>,
    /// Parallel to `meshes`: the mesh's `node_world` (up-axis-corrected), used for prefab assembly.
    node_world: Vec<Mat4>,
    /// Unity's import scale for this file (`useFileScale`/`globalScale` × `UnitScaleFactor`/100).
    import_scale: f32,
}

/// Load and prepare an FBX asset (geometry + node-world transforms + import scale), or `None` if the
/// path isn't a usable FBX. `meta_scale` is the `(globalScale, useFileScale)` read from the `.meta`.
fn load_fbx(path: &Path, meta_scale: (f32, bool)) -> Option<FbxAsset> {
    if path
        .extension()
        .is_none_or(|x| !x.eq_ignore_ascii_case("fbx"))
    {
        return None;
    }
    let doc = avatar_fbx::FbxDocument::load(path).ok()?;
    let scene = doc.scene();
    let up = up_axis_correction(scene.global_settings.up_axis);
    let usf = scene.global_settings.unit_scale_factor.unwrap_or(100.0) as f32;
    let (global_scale, use_file_scale) = meta_scale;
    // Unity: with "Convert Units" on, file units (FBX cm base → UnitScaleFactor/100 metres) scale the
    // model; off, only the global scale factor applies.
    let import_scale = if use_file_scale {
        global_scale * (usf / 100.0)
    } else {
        global_scale
    };

    let meshes = doc.meshes().ok()?;
    let mut node_cache: HashMap<i64, Mat4> = HashMap::new();
    let node_world = meshes
        .iter()
        .map(|m| up * fbx_node_world(&scene, m.model_id, &mut node_cache))
        .collect();
    Some(FbxAsset {
        meshes,
        node_world,
        import_scale,
    })
}

/// Read a model importer's `(globalScale, useFileScale)` from the FBX's `.meta`, defaulting to
/// Unity's defaults `(1.0, true)` when absent.
fn read_model_import_scale(fbx_path: &Path) -> (f32, bool) {
    let meta = fbx_path.with_extension(
        fbx_path
            .extension()
            .map(|e| format!("{}.meta", e.to_string_lossy()))
            .unwrap_or_else(|| "meta".into()),
    );
    let Ok(text) = std::fs::read_to_string(&meta) else {
        return (1.0, true);
    };
    let Some(root) = avatar_unity_yaml::parse_meta(&text) else {
        return (1.0, true);
    };
    let meshes = &root["ModelImporter"]["meshes"];
    let global = field_f64(meshes, "globalScale")
        .map(|v| v as f32)
        .unwrap_or(1.0);
    let use_file = avatar_unity_yaml::field_bool(meshes, "useFileScale").unwrap_or(true);
    (global, use_file)
}

/// Read a model importer's **material remap** (`externalObjects`) from the FBX's `.meta`: FBX
/// material *name* → the project `.mat` asset GUID Unity remapped it to. This is where the real
/// textures of an imported model live (the FBX-embedded material is often a placeholder), so a
/// prefab-instanced model is textured by resolving its materials through this map.
fn read_material_remap(fbx_path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let meta = fbx_path.with_extension(
        fbx_path
            .extension()
            .map(|e| format!("{}.meta", e.to_string_lossy()))
            .unwrap_or_else(|| "meta".into()),
    );
    let Ok(text) = std::fs::read_to_string(&meta) else {
        return map;
    };
    let Some(root) = avatar_unity_yaml::parse_meta(&text) else {
        return map;
    };
    let Some(entries) = root["ModelImporter"]["externalObjects"].as_vec() else {
        return map;
    };
    for e in entries {
        let first = &e["first"];
        // Only material remaps; a model can also remap AvatarMask/Texture entries.
        if field_str(first, "type") != Some("UnityEngine:Material") {
            continue;
        }
        if let (Some(name), Some(guid)) = (field_str(first, "name"), ref_guid(e, "second")) {
            map.insert(name.to_string(), guid);
        }
    }
    map
}

/// The renderer-relevant fields parsed from a Unity `.mat`: base colour tint and base texture GUID.
#[derive(Default)]
struct MatDef {
    color: Option<[f32; 4]>,
    main_tex: Option<String>,
}

/// Parse a material `.mat` file's `_Color` and `_MainTex` texture GUID.
fn parse_material_def(path: &Path) -> MatDef {
    std::fs::read_to_string(path)
        .ok()
        .map(|t| material_def_from_text(&t))
        .unwrap_or_default()
}

/// Extract a Unity `Material`'s `_Color` and `_MainTex` GUID from its YAML text. Split out for tests.
fn material_def_from_text(text: &str) -> MatDef {
    let uf = UnityFile::parse_lossy(text);
    let Some(mat) = uf.documents.iter().find(|d| d.type_name == "Material") else {
        return MatDef::default();
    };
    let saved = &mat.body["m_SavedProperties"];
    // `m_Colors` / `m_TexEnvs` are sequences of single-key maps (`{ _Color: {...} }`).
    let color = find_in_keyed_list(&saved["m_Colors"], "_Color").map(|v| {
        [
            field_f64(v, "r").unwrap_or(1.0) as f32,
            field_f64(v, "g").unwrap_or(1.0) as f32,
            field_f64(v, "b").unwrap_or(1.0) as f32,
            field_f64(v, "a").unwrap_or(1.0) as f32,
        ]
    });
    let main_tex = find_in_keyed_list(&saved["m_TexEnvs"], "_MainTex")
        .and_then(|v| field_str(&v["m_Texture"], "guid").map(|s| s.to_string()));
    MatDef { color, main_tex }
}

/// Find the value for `key` in a YAML sequence of single-key maps (Unity's `m_Colors`/`m_TexEnvs`).
fn find_in_keyed_list<'a>(list: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    for entry in list.as_vec()? {
        if let Some(h) = entry.as_hash() {
            for (k, v) in h {
                if k.as_str() == Some(key) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// Resolve a scene material GUID to a render style (decoded `_MainTex` + `_Color` tint), cached.
fn resolve_scene_material(
    guid: &str,
    guid_index: &HashMap<String, PathBuf>,
    tex: &mut TextureSet,
    cache: &mut HashMap<String, SlotStyle>,
) -> SlotStyle {
    if let Some(s) = cache.get(guid) {
        return SlotStyle {
            texture: s.texture,
            color: s.color,
        };
    }
    let def = guid_index
        .get(guid)
        .map(|p| parse_material_def(p))
        .unwrap_or_default();
    let texture = def
        .main_tex
        .as_deref()
        .and_then(|tg| guid_index.get(tg))
        .and_then(|p| tex.resolve_file(p));
    let color = def.color.unwrap_or(if texture.is_some() {
        WHITE
    } else {
        DEFAULT_COLOR
    });
    cache.insert(guid.to_string(), SlotStyle { texture, color });
    SlotStyle { texture, color }
}

/// Default mesh colour when no material resolves.
const DEFAULT_COLOR: [f32; 4] = [0.72, 0.72, 0.74, 1.0];

/// Parse a `.unity` scene (or a project dir) into placed render meshes. `extra` is prepended to
/// every world transform (e.g. the Unity→renderer handedness flip, shared with a co-placed avatar).
/// `tex` accumulates the decoded texture pool (shared with a co-placed avatar).
pub fn load(path: &Path, extra: Mat4, tex: &mut TextureSet) -> Result<WorldLoad> {
    let (scene_path, root) = resolve_scene(path)?;
    let guid_index = build_guid_index(&root);
    let text = std::fs::read_to_string(&scene_path)
        .with_context(|| format!("reading {}", scene_path.display()))?;
    // Lossy: scenes contain MonoBehaviours whose serialized scalars `yaml-rust2` rejects; we only
    // need Transforms/MeshFilters/MeshRenderers/PrefabInstances, so skip the unparseable rest.
    let uf = UnityFile::parse_lossy(&text);

    let mut nodes: HashMap<i64, Node> = HashMap::new();
    let mut transform_of_go: HashMap<i64, i64> = HashMap::new();
    let mut filters: Vec<(i64, String)> = Vec::new(); // (gameobject fileID, mesh guid)
    let mut mats_by_go: HashMap<i64, Vec<String>> = HashMap::new(); // go → material guid per slot
    let mut prefabs: Vec<PrefabInstance> = Vec::new();

    for d in &uf.documents {
        match d.class_id {
            4 => {
                let father = ref_fileid(&d.body, "m_Father").unwrap_or(0);
                let node = Node {
                    pos: read_vec3(&d.body["m_LocalPosition"], 0.0),
                    rot: read_quat(&d.body["m_LocalRotation"]),
                    scale: read_vec3(&d.body["m_LocalScale"], 1.0),
                    father,
                };
                nodes.insert(d.file_id, node);
                if let Some(go) = ref_fileid(&d.body, "m_GameObject") {
                    transform_of_go.insert(go, d.file_id);
                }
            }
            33 => {
                if let (Some(go), Some(guid)) = (
                    ref_fileid(&d.body, "m_GameObject"),
                    ref_guid(&d.body, "m_Mesh"),
                ) {
                    filters.push((go, guid));
                }
            }
            23 => {
                // All material slots of a MeshRenderer (slot order matters: it aligns with the
                // mesh's per-triangle material index). A built-in/empty slot stores an empty guid.
                if let Some(go) = ref_fileid(&d.body, "m_GameObject")
                    && let Some(mats) = d.body["m_Materials"].as_vec()
                {
                    let guids = mats
                        .iter()
                        .map(|m| field_str(m, "guid").unwrap_or("").to_string())
                        .collect();
                    mats_by_go.insert(go, guids);
                }
            }
            1001 => {
                if let Some(p) = PrefabInstance::parse(&d.body) {
                    prefabs.push(p);
                }
            }
            _ => {}
        }
    }

    // Scene material styles (decoded `_MainTex` + `_Color`) are resolved lazily, cached per guid.
    let mut mat_style_cache: HashMap<String, SlotStyle> = HashMap::new();

    let mut tf_cache: HashMap<i64, Mat4> = HashMap::new();
    let mut fbx_cache: HashMap<String, Option<FbxAsset>> = HashMap::new();
    let mut out = Vec::new();
    let mut placed = 0;
    let mut placed_prefabs = 0;
    let mut skipped_builtin = 0;
    let mut skipped_unresolved = 0;

    // Helper to get a cached FBX asset for a guid.
    let load_asset = |guid: &str, fbx_cache: &mut HashMap<String, Option<FbxAsset>>| -> bool {
        if !fbx_cache.contains_key(guid) {
            let asset = guid_index.get(guid).and_then(|p| {
                let scale = read_model_import_scale(p);
                load_fbx(p, scale)
            });
            fbx_cache.insert(guid.to_string(), asset);
        }
        fbx_cache.get(guid).is_some_and(|a| a.is_some())
    };

    // 1) Directly-placed MeshFilters: raw geometry at the GameObject's transform (matches Unity's
    //    shared-mesh-on-plain-GameObject behaviour — the source node transform is not reapplied).
    for (go, guid) in &filters {
        let Some(&tfid) = transform_of_go.get(go) else {
            skipped_unresolved += 1;
            continue;
        };
        if !guid_index.contains_key(guid) {
            skipped_builtin += 1; // built-in mesh or asset outside the project
            continue;
        }
        if !load_asset(guid, &mut fbx_cache) {
            skipped_unresolved += 1;
            continue;
        }
        let world = world_matrix(tfid, &nodes, &mut tf_cache);
        // `load_asset` returned true, so the cache holds a `Some` for this guid; still skip rather
        // than panic if that ever fails to hold (e.g. future cache eviction).
        let Some(Some(asset)) = fbx_cache.get(guid) else {
            eprintln!("warning: FBX asset for mesh guid {guid} unexpectedly missing; skipping");
            skipped_unresolved += 1;
            continue;
        };
        let mats = mats_by_go.get(go);
        // Unity bakes the model import scale into the imported mesh's vertices, so a shared sub-mesh
        // assigned to a plain GameObject is already scaled. We apply it here to the raw FBX geometry.
        let place = extra * world * Mat4::from_scale(Vec3::splat(asset.import_scale));
        let mut any = false;
        for m in &asset.meshes {
            if m.positions.is_empty() || m.indices.is_empty() {
                continue;
            }
            // Scene-material per slot (Unity's remapped `.mat` is authoritative for a placed prop).
            let style = |slot: usize| -> SlotStyle {
                let guid = mats
                    .and_then(|v| v.get(slot).or_else(|| v.last()))
                    .filter(|g| !g.is_empty());
                match guid {
                    Some(g) => resolve_scene_material(g, &guid_index, tex, &mut mat_style_cache),
                    None => SlotStyle {
                        texture: None,
                        color: DEFAULT_COLOR,
                    },
                }
            };
            out.extend(split_by_material(m, place, style));
            any = true;
        }
        if any {
            placed += 1;
        }
    }

    // 2) Prefab instances of an FBX model: re-instantiate every mesh with its node-world transform,
    //    textured from the FBX's *embedded* materials (an instanced model has no scene-side material).
    for p in &prefabs {
        let Some(guid) = &p.source_guid else {
            continue;
        };
        if !guid_index.contains_key(guid) {
            skipped_builtin += 1;
            continue;
        }
        if !load_asset(guid, &mut fbx_cache) {
            skipped_unresolved += 1;
            continue;
        }
        let fbx_path = guid_index.get(guid).cloned();
        let fbx_dir = fbx_path
            .as_deref()
            .and_then(|p| p.parent())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        // Unity remaps the model's materials (by name) to project `.mat` assets — that's where the
        // real textures live; fall back to the FBX-embedded material when a name isn't remapped.
        let remap = fbx_path
            .as_deref()
            .map(read_material_remap)
            .unwrap_or_default();
        // `load_asset` returned true, so the cache holds a `Some` for this guid; still skip rather
        // than panic if that ever fails to hold.
        let Some(Some(asset)) = fbx_cache.get(guid) else {
            eprintln!("warning: FBX asset for prefab guid {guid} unexpectedly missing; skipping");
            skipped_unresolved += 1;
            continue;
        };
        let parent_world = p
            .transform_parent
            .map(|pid| world_matrix(pid, &nodes, &mut tf_cache))
            .unwrap_or(Mat4::IDENTITY);
        // Instance root: the model-prefab root carries the import scale as its default scale.
        let root_local = Mat4::from_scale_rotation_translation(
            Vec3::splat(asset.import_scale),
            p.root_rot,
            p.root_pos,
        );
        let place = extra * parent_world * root_local;
        let mut any = false;
        for (i, m) in asset.meshes.iter().enumerate() {
            if m.positions.is_empty() || m.indices.is_empty() {
                continue;
            }
            let transform = place * asset.node_world[i];
            let style = |slot: usize| -> SlotStyle {
                let mat = m.materials.get(slot);
                // 1) Unity material remap by name → project `.mat` (authoritative for textures).
                if let Some(g) = mat.and_then(|mm| remap.get(&mm.name)) {
                    return resolve_scene_material(g, &guid_index, tex, &mut mat_style_cache);
                }
                // 2) Fall back to the FBX-embedded material's texture/colour.
                let texture = mat.and_then(|mm| tex.resolve_fbx_material(&fbx_dir, mm));
                let color = mat
                    .and_then(|mm| mm.diffuse_color)
                    .unwrap_or(if texture.is_some() {
                        WHITE
                    } else {
                        DEFAULT_COLOR
                    });
                SlotStyle { texture, color }
            };
            out.extend(split_by_material(m, transform, style));
            any = true;
        }
        if any {
            placed_prefabs += 1;
        }
    }

    if out.is_empty() {
        bail!(
            "rendered no world geometry from {} ({} mesh filters, {} prefab instances, {} built-in/unresolved)",
            scene_path.display(),
            filters.len(),
            prefabs.len(),
            skipped_builtin + skipped_unresolved
        );
    }

    let spawn = find_spawn(&uf, &nodes, &transform_of_go, &mut tf_cache, extra);

    Ok(WorldLoad {
        meshes: out,
        placed,
        placed_prefabs,
        skipped_builtin,
        skipped_unresolved,
        spawn,
    })
}

/// Find where VRChat would materialise a player, returned in **renderer space** (× `extra`).
///
/// Preferred source is the scene's `VRC_SceneDescriptor` (a MonoBehaviour with a `spawns` list of
/// transform references) — its first spawn is the default. Failing that (e.g. the descriptor's body
/// didn't survive lossy parsing), we fall back to the transform of the GameObject conventionally
/// named `VRCWorld`, which the descriptor lives on. `None` if neither is present.
fn find_spawn(
    uf: &UnityFile,
    nodes: &HashMap<i64, Node>,
    transform_of_go: &HashMap<i64, i64>,
    tf_cache: &mut HashMap<i64, Mat4>,
    extra: Mat4,
) -> Option<Vec3> {
    let spawn_tfid = uf
        .documents
        .iter()
        .filter(|d| d.class_id == 114)
        .find_map(|d| d.body["spawns"].as_vec().and_then(|v| v.first()))
        .and_then(|s| field_i64(s, "fileID"))
        .filter(|id| nodes.contains_key(id))
        .or_else(|| {
            // Fallback: the GameObject named "VRCWorld" → its transform.
            uf.documents
                .iter()
                .filter(|d| d.class_id == 1 && d.name() == Some("VRCWorld"))
                .find_map(|d| transform_of_go.get(&d.file_id).copied())
        })?;
    let world = extra * world_matrix(spawn_tfid, nodes, tf_cache);
    Some(world.w_axis.truncate())
}

/// A Unity `PrefabInstance` (class 1001), distilled to what we need to place an instanced model.
struct PrefabInstance {
    source_guid: Option<String>,
    transform_parent: Option<i64>,
    root_pos: Vec3,
    root_rot: Quat,
}

impl PrefabInstance {
    /// Parse the instance's source prefab, parent transform, and root position/rotation overrides
    /// from `m_Modification.m_Modifications`. The root's *scale* is intentionally not read here — it
    /// defaults to the FBX import scale (a prefab default not serialized in the scene).
    fn parse(body: &Yaml) -> Option<Self> {
        let modification = &body["m_Modification"];
        let source_guid = ref_guid(body, "m_SourcePrefab");
        let transform_parent = ref_fileid(modification, "m_TransformParent").filter(|&v| v != 0);

        // The root transform is the modification target carrying m_LocalPosition/Rotation. There is
        // usually exactly one such target (the instance root); collect its position/rotation.
        let mut pos = Vec3::ZERO;
        let mut rot = [0.0f32, 0.0, 0.0, 1.0];
        if let Some(mods) = modification["m_Modifications"].as_vec() {
            for m in mods {
                let Some(path) = field_str(m, "propertyPath") else {
                    continue;
                };
                let Some(val) = field_f64(m, "value").map(|v| v as f32) else {
                    continue;
                };
                match path {
                    "m_LocalPosition.x" => pos.x = val,
                    "m_LocalPosition.y" => pos.y = val,
                    "m_LocalPosition.z" => pos.z = val,
                    "m_LocalRotation.x" => rot[0] = val,
                    "m_LocalRotation.y" => rot[1] = val,
                    "m_LocalRotation.z" => rot[2] = val,
                    "m_LocalRotation.w" => rot[3] = val,
                    _ => {}
                }
            }
        }
        let q = Quat::from_xyzw(rot[0], rot[1], rot[2], rot[3]);
        let root_rot = if q.length() < 1e-6 {
            Quat::IDENTITY
        } else {
            q.normalize()
        };
        Some(PrefabInstance {
            source_guid,
            transform_parent,
            root_pos: pos,
            root_rot,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_fbx::LocalTransform;

    fn approx(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn fbx_local_matrix_identity() {
        let m = fbx_local_matrix(&LocalTransform::default());
        assert_eq!(m, Mat4::IDENTITY);
    }

    #[test]
    fn fbx_local_matrix_neg90_x_maps_y_to_neg_z() {
        // The Blender→FBX axis-conversion rotation: −90° about X sends +Y to −Z.
        let t = LocalTransform {
            translation: Some([0.0, 0.0, 0.0]),
            rotation: Some([-90.0, 0.0, 0.0]),
            scaling: Some([1.0, 1.0, 1.0]),
        };
        let m = fbx_local_matrix(&t);
        let mapped = m.transform_vector3(Vec3::Y);
        assert!(approx(mapped, -Vec3::Z), "expected +Y→−Z, got {mapped:?}");
    }

    #[test]
    fn fbx_local_matrix_applies_scale_then_translation() {
        let t = LocalTransform {
            translation: Some([10.0, 0.0, 0.0]),
            rotation: None,
            scaling: Some([2.0, 2.0, 2.0]),
        };
        let m = fbx_local_matrix(&t);
        // A unit point scales by 2 then shifts by +10 → 12.
        assert!(approx(
            m.transform_point3(Vec3::new(1.0, 0.0, 0.0)),
            Vec3::new(12.0, 0.0, 0.0)
        ));
    }

    /// Parse the `PrefabInstance` body out of a synthetic 1001 document.
    fn prefab_from(text: &str) -> PrefabInstance {
        let uf = UnityFile::parse_lossy(text);
        let doc = uf
            .documents
            .iter()
            .find(|d| d.class_id == 1001)
            .expect("a 1001 doc");
        PrefabInstance::parse(&doc.body).expect("parse")
    }

    #[test]
    fn prefab_instance_parses_source_parent_and_root() {
        // Mirrors the cabin instance: a source FBX, a parent transform, and root position/rotation
        // overrides (scale intentionally absent — it defaults to the FBX import scale).
        let text = "\
--- !u!1001 &707374702
PrefabInstance:
  m_ObjectHideFlags: 0
  m_Modification:
    m_TransformParent: {fileID: 1019458579}
    m_Modifications:
    - target: {fileID: -8679921383154817045, guid: c9fe5810f3436e146bfa9a41d2b05442, type: 3}
      propertyPath: m_LocalPosition.x
      value: -0.5
      objectReference: {fileID: 0}
    - target: {fileID: -8679921383154817045, guid: c9fe5810f3436e146bfa9a41d2b05442, type: 3}
      propertyPath: m_LocalPosition.y
      value: 1.5
      objectReference: {fileID: 0}
    - target: {fileID: -8679921383154817045, guid: c9fe5810f3436e146bfa9a41d2b05442, type: 3}
      propertyPath: m_LocalRotation.w
      value: 1
      objectReference: {fileID: 0}
  m_SourcePrefab: {fileID: 100100000, guid: c9fe5810f3436e146bfa9a41d2b05442, type: 3}
";
        let p = prefab_from(text);
        assert_eq!(
            p.source_guid.as_deref(),
            Some("c9fe5810f3436e146bfa9a41d2b05442")
        );
        assert_eq!(p.transform_parent, Some(1019458579));
        assert!(approx(p.root_pos, Vec3::new(-0.5, 1.5, 0.0)));
        assert!((p.root_rot.length() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn prefab_instance_defaults_identity_rotation() {
        let text = "\
--- !u!1001 &1
PrefabInstance:
  m_Modification:
    m_TransformParent: {fileID: 0}
    m_Modifications: []
  m_SourcePrefab: {fileID: 100100000, guid: aaaa1111bbbb2222cccc3333dddd4444, type: 3}
";
        let p = prefab_from(text);
        assert_eq!(p.transform_parent, None); // fileID 0 → no parent
        assert_eq!(p.root_rot, Quat::IDENTITY);
        assert!(approx(p.root_pos, Vec3::ZERO));
    }

    #[test]
    fn material_def_reads_color_tint() {
        let text = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  m_Name: Wood
  m_SavedProperties:
    m_TexEnvs:
    - _MainTex:
        m_Texture: {fileID: 0}
    m_Colors:
    - _Color: {r: 0.5, g: 0.25, b: 0.125, a: 1}
    - _EmissionColor: {r: 0, g: 0, b: 0, a: 1}
";
        let def = material_def_from_text(text);
        let c = def.color.expect("color");
        assert!((c[0] - 0.5).abs() < 1e-4);
        assert!((c[1] - 0.25).abs() < 1e-4);
        assert!((c[2] - 0.125).abs() < 1e-4);
        assert!((c[3] - 1.0).abs() < 1e-4);
        // `_MainTex` here points at fileID 0 (no texture), so no GUID is captured.
        assert_eq!(def.main_tex, None);
    }

    #[test]
    fn material_def_reads_main_tex_guid() {
        let text = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  m_Name: Bark
  m_SavedProperties:
    m_TexEnvs:
    - _BumpMap:
        m_Texture: {fileID: 0}
    - _MainTex:
        m_Texture: {fileID: 2800000, guid: abcdef0123456789abcdef0123456789, type: 3}
    m_Colors:
    - _Color: {r: 1, g: 1, b: 1, a: 1}
";
        let def = material_def_from_text(text);
        assert_eq!(
            def.main_tex.as_deref(),
            Some("abcdef0123456789abcdef0123456789")
        );
    }
}
