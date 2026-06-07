//! Parse a Unity `.unity` scene into placed render meshes.
//!
//! Reads the scene's `Transform` (class 4) hierarchy and `MeshFilter` (class 33) components,
//! composes each mesh's world matrix, resolves its `m_Mesh` GUID to a source FBX via the project's
//! `.meta` index, and emits one [`RenderMesh`] per submesh placed at that transform. Unity's
//! left-handed Y-up space is converted to the renderer's right-handed space by negating Z.
//!
//! Scope (a best-effort static preview, not a Unity-accurate render):
//! - Only **directly-placed** MeshFilters are drawn. Prefab *instances* (class 1001) are not
//!   expanded — the Cozy Cabin main scene is almost entirely flat, so this still covers the map.
//! - Built-in meshes (Unity primitives / default resources, whose GUID isn't a project asset) and
//!   non-FBX mesh assets are skipped.
//! - Per-prop scale/orientation assumes the FBX imports roughly 1:1; we apply the FBX up-axis
//!   correction but do not replicate Unity's full import pipeline (FBX scale factor, node
//!   transforms), so some props may be mis-scaled. Logged via the caller's mesh/skip counts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_mesh::RawMesh;
use avatar_render::RenderMesh;
use avatar_unity_yaml::{UnityFile, field_f64, field_i64, field_str};
use glam::{Mat4, Quat, Vec3};
use yaml_rust2::Yaml;

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
    pub placed: usize,
    pub skipped_builtin: usize,
    pub skipped_unresolved: usize,
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

/// World matrix for a transform, composing up the father chain (memoized, cycle-guarded).
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

/// Up-axis correction (Z-up → Y-up) for a static FBX mesh, matching `render_scene`'s convention.
fn up_axis_correction(up_axis: Option<i32>) -> Mat4 {
    match up_axis {
        Some(2) => Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        _ => Mat4::IDENTITY,
    }
}

/// Load the meshes (and FBX up-axis) for an asset GUID, or `None` if it isn't a usable FBX.
fn load_fbx_for_guid(path: &Path) -> Option<(Vec<RawMesh>, Mat4)> {
    if !path.extension().is_some_and(|x| {
        let x = x.to_ascii_lowercase();
        x == "fbx"
    }) {
        return None;
    }
    let doc = avatar_fbx::FbxDocument::load(path).ok()?;
    let up = up_axis_correction(doc.scene().global_settings.up_axis);
    let meshes = doc.meshes().ok()?;
    Some((meshes, up))
}

/// Parse a `.unity` scene (or a project dir) into placed render meshes. `extra` is prepended to
/// every world transform (e.g. the Unity→renderer handedness flip, shared with a co-placed avatar).
pub fn load(path: &Path, extra: Mat4) -> Result<WorldLoad> {
    let (scene_path, root) = resolve_scene(path)?;
    let guid_index = build_guid_index(&root);
    let text = std::fs::read_to_string(&scene_path)
        .with_context(|| format!("reading {}", scene_path.display()))?;
    // Lossy: scenes contain MonoBehaviours whose serialized scalars `yaml-rust2` rejects; we only
    // need Transforms and MeshFilters, so skip the unparseable rest rather than fail the whole file.
    let uf = UnityFile::parse_lossy(&text);

    let mut nodes: HashMap<i64, Node> = HashMap::new();
    let mut transform_of_go: HashMap<i64, i64> = HashMap::new();
    let mut filters: Vec<(i64, String)> = Vec::new(); // (gameobject fileID, mesh guid)

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
            _ => {}
        }
    }

    let mut cache: HashMap<i64, Mat4> = HashMap::new();
    let mut fbx_cache: HashMap<String, Option<(Vec<RawMesh>, Mat4)>> = HashMap::new();
    let mut out = Vec::new();
    let mut placed = 0;
    let mut skipped_builtin = 0;
    let mut skipped_unresolved = 0;

    for (go, guid) in &filters {
        let Some(&tfid) = transform_of_go.get(go) else {
            skipped_unresolved += 1;
            continue;
        };
        let world = world_matrix(tfid, &nodes, &mut cache);

        let Some(asset) = guid_index.get(guid) else {
            skipped_builtin += 1; // built-in mesh or asset outside the project
            continue;
        };
        let entry = fbx_cache
            .entry(guid.clone())
            .or_insert_with(|| load_fbx_for_guid(asset));
        let Some((meshes, up)) = entry else {
            skipped_unresolved += 1;
            continue;
        };

        let place = extra * world * *up;
        let mut any = false;
        for m in meshes.iter() {
            if m.positions.is_empty() || m.indices.is_empty() {
                continue;
            }
            out.push(RenderMesh {
                positions: m.positions.clone(),
                normals: Vec::new(),
                indices: m.indices.clone(),
                color: [0.72, 0.72, 0.74, 1.0],
                transform: place,
            });
            any = true;
        }
        if any {
            placed += 1;
        }
    }

    if out.is_empty() {
        bail!(
            "rendered no world geometry from {} ({} mesh filters, {} built-in/unresolved)",
            scene_path.display(),
            filters.len(),
            skipped_builtin + skipped_unresolved
        );
    }

    Ok(WorldLoad {
        meshes: out,
        placed,
        skipped_builtin,
        skipped_unresolved,
    })
}
