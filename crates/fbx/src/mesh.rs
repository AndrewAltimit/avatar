//! Geometry + skin extraction: turn the FBX `Geometry`/`Deformer` nodes into renderer-agnostic
//! [`avatar_mesh::RawMesh`]s. This is the data a runtime needs to *render* a posed avatar — the
//! piece `avatar-fbx` did not previously surface.
//!
//! It operates on the retained [`Tree`] (the array payloads live there, not on the flattened
//! [`FbxScene`]), but uses the scene's connection graph for the object wiring.

use std::collections::HashMap;

use anyhow::Result;
use avatar_mesh::{IDENTITY_16, MeshMaterial, RawMesh, SkinCluster, SkinData, TextureRef};
use fbxcel::low::v7400::AttributeValue;
use fbxcel::tree::v7400::{NodeHandle, NodeId, Tree};

use crate::{FbxScene, as_i64, as_str, child_named};

/// Extract every skinned/static mesh in the scene. Empty if there are no `Geometry` nodes.
pub(crate) fn extract_meshes(tree: &Tree, scene: &FbxScene) -> Result<Vec<RawMesh>> {
    let root = tree.root();
    let id_to_node = object_node_index(&root);

    let mut meshes = Vec::new();
    for geom in scene.objects.iter().filter(|o| o.node_name == "Geometry") {
        let Some(&node_id) = id_to_node.get(&geom.id) else {
            continue;
        };
        let geom_node = node_id.to_handle(tree);

        let Some(vertices) = node_f64_array(&geom_node, "Vertices") else {
            continue; // not a mesh geometry (e.g. a blendshape Shape) — skip.
        };
        let Some(pvi) = node_i32_array(&geom_node, "PolygonVertexIndex") else {
            continue;
        };

        let tri = triangulate(&vertices, &pvi);
        let normals = read_layer(
            &geom_node,
            "LayerElementNormal",
            "Normals",
            "NormalsIndex",
            3,
            &tri,
        )
        .map(|flat| chunk3(&flat));
        let uvs = read_layer(&geom_node, "LayerElementUV", "UV", "UVIndex", 2, &tri)
            .map(|flat| chunk2(&flat));

        // Geometry's OO parent is its mesh Model.
        let model_id = scene.parent_of(geom.id).unwrap_or(geom.id);
        let skin = extract_skin(tree, scene, &id_to_node, geom.id);

        // Materials are connected to the mesh's Model (slot order = connection order); the
        // per-polygon `LayerElementMaterial` indexes into that slot list.
        let materials = collect_materials(tree, scene, &id_to_node, model_id);
        let material_of_triangle = material_of_triangle(&geom_node, &tri, materials.len());

        meshes.push(RawMesh {
            model_id,
            positions: tri.positions,
            normals,
            uvs,
            indices: tri.indices,
            control_point_of_vertex: tri.control_point_of_vertex,
            skin,
            materials,
            material_of_triangle,
            polygon_of_triangle: tri.polygon_of_triangle.clone(),
        });
    }
    Ok(meshes)
}

/// Map every object id under `Objects` to its tree node, so we can read array payloads by id.
fn object_node_index(root: &NodeHandle) -> HashMap<i64, NodeId> {
    let mut map = HashMap::new();
    if let Some(objects) = child_named(root, "Objects") {
        for n in objects.children() {
            if let Some(id) = n.attributes().first().and_then(as_i64) {
                map.insert(id, n.node_id());
            }
        }
    }
    map
}

/// Result of triangulating one geometry: emitted vertices plus the index maps a skinner needs.
struct Triangulated {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    /// Source control-point index per emitted vertex (for attaching skin weights).
    control_point_of_vertex: Vec<u32>,
    /// Source polygon-vertex position (index into `PolygonVertexIndex`) per emitted vertex —
    /// internal, used to resolve `ByPolygonVertex` layer elements.
    polygon_vertex_of_vertex: Vec<u32>,
    /// Source polygon index per emitted **triangle** (one entry per 3 of `indices`) — used to
    /// resolve the per-polygon `LayerElementMaterial`.
    polygon_of_triangle: Vec<u32>,
}

/// Fan-triangulate FBX polygons. `PolygonVertexIndex` is a flat list of control-point indices; a
/// polygon ends at a negative entry whose true index is `!raw` (one's-complement). Emits an
/// un-indexed triangle soup (every triangle vertex is a distinct emitted vertex) — simple, and it
/// makes per-polygon-vertex normals/UVs map 1:1.
fn triangulate(vertices: &[f64], pvi: &[i32]) -> Triangulated {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut cp_of_vertex = Vec::new();
    let mut pv_of_vertex = Vec::new();
    let mut poly_of_tri = Vec::new();

    // (control-point index, polygon-vertex position) for the polygon being accumulated.
    let mut poly: Vec<(u32, u32)> = Vec::new();
    let mut poly_index: u32 = 0;
    for (pv_pos, &raw) in pvi.iter().enumerate() {
        let (cp, end) = if raw < 0 {
            ((!raw) as u32, true)
        } else {
            (raw as u32, false)
        };
        poly.push((cp, pv_pos as u32));
        if end {
            for i in 1..poly.len().saturating_sub(1) {
                let before = indices.len();
                for &(c, pv) in &[poly[0], poly[i], poly[i + 1]] {
                    // Control-point indices come straight from the (untrusted) file. Use checked
                    // arithmetic so a hostile index can never wrap `usize` and slip past the
                    // `base + 2 < len` bounds check into an out-of-range slice index.
                    let Some(base) = (c as usize).checked_mul(3) else {
                        continue;
                    };
                    if base + 2 < vertices.len() {
                        positions.push([
                            vertices[base] as f32,
                            vertices[base + 1] as f32,
                            vertices[base + 2] as f32,
                        ]);
                        cp_of_vertex.push(c);
                        pv_of_vertex.push(pv);
                        indices.push(positions.len() as u32 - 1);
                    }
                }
                // A whole triangle (3 indices) was emitted → record which polygon it came from.
                if indices.len() == before + 3 {
                    poly_of_tri.push(poly_index);
                }
            }
            poly.clear();
            poly_index += 1;
        }
    }

    Triangulated {
        positions,
        indices,
        control_point_of_vertex: cp_of_vertex,
        polygon_vertex_of_vertex: pv_of_vertex,
        polygon_of_triangle: poly_of_tri,
    }
}

/// Read a `LayerElement*` (normals/UVs) into a flat per-emitted-vertex array, honoring the common
/// `Mapping`/`Reference` combinations. Returns `None` on an unsupported layout (caller degrades to
/// `None` rather than emitting garbage). Skinning does not depend on this.
fn read_layer(
    geom: &NodeHandle,
    elem_name: &str,
    data_name: &str,
    index_name: &str,
    comps: usize,
    tri: &Triangulated,
) -> Option<Vec<f32>> {
    let elem = child_named(geom, elem_name)?;
    let data = node_f64_array(&elem, data_name)?;
    let mapping = node_str(&elem, "MappingInformationType")?;
    let reference = node_str(&elem, "ReferenceInformationType").unwrap_or_default();
    let index = node_i32_array(&elem, index_name);

    let key_for = |k: usize| -> Option<u32> {
        match mapping.as_str() {
            "ByControlPoint" | "ByControlpoint" => tri.control_point_of_vertex.get(k).copied(),
            "ByPolygonVertex" => tri.polygon_vertex_of_vertex.get(k).copied(),
            _ => None,
        }
    };

    // `positions.len() * comps` is derived from already-parsed geometry, but guard the multiply so
    // it can never overflow `usize` when pre-sizing the output buffer.
    let mut out = Vec::with_capacity(tri.positions.len().saturating_mul(comps));
    for k in 0..tri.positions.len() {
        let key = key_for(k)? as usize;
        let src = if reference == "IndexToDirect" || reference == "Index" {
            *index.as_ref()?.get(key)? as usize
        } else {
            key
        };
        // `src` may originate from a file-supplied index buffer; checked-multiply so a hostile
        // index cannot wrap and produce a false in-bounds `base`.
        let base = src.checked_mul(comps)?;
        if base + comps > data.len() {
            return None;
        }
        for c in 0..comps {
            out.push(data[base + c] as f32);
        }
    }
    Some(out)
}

/// Walk Geometry -> Deformer(Skin) -> SubDeformer(Cluster) -> bone Model, reading each cluster's
/// influence indices/weights and bind matrices.
fn extract_skin(
    tree: &Tree,
    scene: &FbxScene,
    id_to_node: &HashMap<i64, NodeId>,
    geom_id: i64,
) -> Option<SkinData> {
    let mut clusters = Vec::new();
    for skin_id in scene.children_of(geom_id) {
        // Deformers are distinguished by their sub-class token, not node name, across exporters.
        if scene.object(skin_id).map(|o| o.subclass.as_str()) != Some("Skin") {
            continue;
        }
        for cluster_id in scene.children_of(skin_id) {
            if scene.object(cluster_id).map(|o| o.subclass.as_str()) != Some("Cluster") {
                continue;
            }
            let Some(&nid) = id_to_node.get(&cluster_id) else {
                continue;
            };
            let cl = nid.to_handle(tree);

            // The cluster's bone is its OO child Model.
            let Some(bone_id) = scene
                .children_of(cluster_id)
                .into_iter()
                .find(|&c| scene.object(c).is_some_and(|o| o.is_model()))
            else {
                continue;
            };

            let indexes: Vec<u32> = node_i32_array(&cl, "Indexes")
                .unwrap_or_default()
                .iter()
                .map(|&i| i.max(0) as u32)
                .collect();
            let weights: Vec<f32> = node_f64_array(&cl, "Weights")
                .unwrap_or_default()
                .iter()
                .map(|&w| w as f32)
                .collect();

            clusters.push(SkinCluster {
                bone_id,
                indexes,
                weights,
                transform_link: node_mat16(&cl, "TransformLink"),
                transform: node_mat16(&cl, "Transform"),
            });
        }
    }
    (!clusters.is_empty()).then_some(SkinData { clusters })
}

// --- materials & textures ---------------------------------------------------------------------

/// Per-triangle material **slot** (index into the mesh's material list), resolved from the
/// geometry's `LayerElementMaterial`. Returns an empty vec when there is nothing to disambiguate
/// (no layer, or a single material used everywhere) — the consumer then treats every triangle as
/// slot 0.
fn material_of_triangle(geom: &NodeHandle, tri: &Triangulated, n_materials: usize) -> Vec<u32> {
    let Some(elem) = child_named(geom, "LayerElementMaterial") else {
        return Vec::new();
    };
    let Some(materials) = node_i32_array(&elem, "Materials") else {
        return Vec::new();
    };
    let mapping = node_str(&elem, "MappingInformationType").unwrap_or_default();
    let n_tri = tri.polygon_of_triangle.len();
    match mapping.as_str() {
        // One index for the whole mesh. Only meaningful (non-empty) when it isn't slot 0.
        "AllSame" | "AllSameForAll" => {
            let slot = materials.first().copied().unwrap_or(0).max(0) as u32;
            if slot == 0 {
                Vec::new()
            } else {
                vec![slot; n_tri]
            }
        }
        // One index per polygon → expand to per triangle. Skip when there's nothing to split into.
        "ByPolygon" => {
            if n_materials <= 1 {
                return Vec::new();
            }
            tri.polygon_of_triangle
                .iter()
                .map(|&p| materials.get(p as usize).copied().unwrap_or(0).max(0) as u32)
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Collect the materials connected to a mesh `Model`, in slot order (the order their `OO`
/// connections appear — which is what `LayerElementMaterial` indexes into).
fn collect_materials(
    tree: &Tree,
    scene: &FbxScene,
    id_to_node: &HashMap<i64, NodeId>,
    model_id: i64,
) -> Vec<MeshMaterial> {
    scene
        .children_of(model_id)
        .into_iter()
        .filter(|&c| scene.object(c).map(|o| o.node_name.as_str()) == Some("Material"))
        .map(|mat_id| read_material(tree, scene, id_to_node, mat_id))
        .collect()
}

/// Read one `Material` object: its name, diffuse colour, and (resolved) diffuse texture reference.
fn read_material(
    tree: &Tree,
    scene: &FbxScene,
    id_to_node: &HashMap<i64, NodeId>,
    mat_id: i64,
) -> MeshMaterial {
    let name = scene
        .object(mat_id)
        .map(|o| o.name.clone())
        .unwrap_or_default();
    let diffuse_color = id_to_node.get(&mat_id).and_then(|nid| {
        let node = nid.to_handle(tree);
        // Prefer the standard `DiffuseColor`; fall back to `Diffuse` (older/3ds Max exports).
        prop_color(&node, "DiffuseColor").or_else(|| prop_color(&node, "Diffuse"))
    });
    let texture = find_diffuse_texture(tree, scene, id_to_node, mat_id);
    MeshMaterial {
        name,
        diffuse_color,
        texture,
    }
}

/// Find a material's diffuse/base-colour texture: the `Texture` object connected to it (preferring
/// a connection whose destination property names the diffuse/base colour), then read that texture's
/// file path(s) and any embedded image bytes.
fn find_diffuse_texture(
    tree: &Tree,
    scene: &FbxScene,
    id_to_node: &HashMap<i64, NodeId>,
    mat_id: i64,
) -> Option<TextureRef> {
    // Connections into the material whose child is a `Texture`. An `OP` connection carries the
    // destination property (e.g. "DiffuseColor", "Maps|DiffuseColor"); `OO` ones carry none.
    let is_texture = |id: i64| scene.object(id).map(|o| o.node_name.as_str()) == Some("Texture");
    let prop_is_diffuse = |p: &Option<String>| {
        p.as_deref().is_some_and(|p| {
            let p = p.to_ascii_lowercase();
            p.contains("diffuse") || p.contains("basecolor") || p.contains("base_color")
        })
    };
    let candidates: Vec<&crate::Connection> = scene
        .connections
        .iter()
        .filter(|c| c.parent == mat_id && is_texture(c.child))
        .collect();
    // Prefer an explicit diffuse/base-colour binding; else take the first texture on the material.
    let tex_id = candidates
        .iter()
        .find(|c| prop_is_diffuse(&c.property))
        .or_else(|| candidates.first())
        .map(|c| c.child)?;

    read_texture(tree, scene, id_to_node, tex_id)
}

/// Read a `Texture` object's file references and embedded image bytes (the latter via a connected
/// `Video`/`Media` object's `Content` blob, or directly on the texture for older exports).
fn read_texture(
    tree: &Tree,
    scene: &FbxScene,
    id_to_node: &HashMap<i64, NodeId>,
    tex_id: i64,
) -> Option<TextureRef> {
    let tex = id_to_node.get(&tex_id)?.to_handle(tree);
    let mut tref = TextureRef {
        relative: node_str(&tex, "RelativeFilename").filter(|s| !s.is_empty()),
        absolute: node_str(&tex, "FileName").filter(|s| !s.is_empty()),
        embedded: node_binary(&tex, "Content").filter(|b| !b.is_empty()),
    };

    // Embedded bytes (and a better relative path) usually live on a connected `Video`/`Media`.
    if tref.embedded.is_none() || tref.relative.is_none() {
        // Either direction of OO connection between the texture and a Video.
        let video_id = scene
            .connections
            .iter()
            .filter(|c| c.kind == "OO")
            .find_map(|c| {
                let other = if c.parent == tex_id {
                    c.child
                } else if c.child == tex_id {
                    c.parent
                } else {
                    return None;
                };
                let nm = scene.object(other).map(|o| o.node_name.as_str());
                matches!(nm, Some("Video") | Some("Media")).then_some(other)
            });
        if let Some(vid) = video_id.and_then(|v| id_to_node.get(&v)) {
            let vnode = vid.to_handle(tree);
            if tref.embedded.is_none() {
                tref.embedded = node_binary(&vnode, "Content").filter(|b| !b.is_empty());
            }
            if tref.relative.is_none() {
                tref.relative = node_str(&vnode, "RelativeFilename").filter(|s| !s.is_empty());
            }
            if tref.absolute.is_none() {
                tref.absolute = node_str(&vnode, "Filename")
                    .or_else(|| node_str(&vnode, "FileName"))
                    .filter(|s| !s.is_empty());
            }
        }
    }

    (tref.relative.is_some() || tref.absolute.is_some() || tref.embedded.is_some()).then_some(tref)
}

/// Read a `Properties70` colour property (`[name, "Color"/"ColorRGB", label, flags, r, g, b]`).
fn prop_color(obj: &NodeHandle, key: &str) -> Option<[f32; 4]> {
    let props = child_named(obj, "Properties70")?;
    for p in props.children().filter(|n| n.name() == "P") {
        let attrs = p.attributes();
        if attrs.first().and_then(as_str) == Some(key) {
            let r = attrs.get(4).and_then(attr_f64)? as f32;
            let g = attrs.get(5).and_then(attr_f64)? as f32;
            let b = attrs.get(6).and_then(attr_f64)? as f32;
            return Some([r, g, b, 1.0]);
        }
    }
    None
}

/// Coerce a numeric attribute to `f64` (colours are usually `F64`, occasionally `F32`).
fn attr_f64(v: &AttributeValue) -> Option<f64> {
    match v {
        AttributeValue::F64(n) => Some(*n),
        AttributeValue::F32(n) => Some(*n as f64),
        AttributeValue::I32(n) => Some(*n as f64),
        AttributeValue::I64(n) => Some(*n as f64),
        _ => None,
    }
}

/// Read a named child node's first attribute as raw bytes (an FBX `Content` blob).
fn node_binary(node: &NodeHandle, name: &str) -> Option<Vec<u8>> {
    match first_attr(node, name)? {
        AttributeValue::Binary(b) => Some(b.clone()),
        _ => None,
    }
}

// --- small array/string readers over fbxcel nodes ---------------------------------------------

fn first_attr<'a>(node: &'a NodeHandle, name: &str) -> Option<&'a AttributeValue> {
    child_named(node, name)?.attributes().first()
}

/// Read a named child node's first attribute as an `f64` array (upcasting an `f32` array).
fn node_f64_array(node: &NodeHandle, name: &str) -> Option<Vec<f64>> {
    match first_attr(node, name)? {
        AttributeValue::ArrF64(a) => Some(a.clone()),
        AttributeValue::ArrF32(a) => Some(a.iter().map(|&x| x as f64).collect()),
        _ => None,
    }
}

/// Read a named child node's first attribute as an `i32` array (downcasting an `i64` array).
fn node_i32_array(node: &NodeHandle, name: &str) -> Option<Vec<i32>> {
    match first_attr(node, name)? {
        AttributeValue::ArrI32(a) => Some(a.clone()),
        AttributeValue::ArrI64(a) => Some(a.iter().map(|&x| x as i32).collect()),
        _ => None,
    }
}

fn node_str(node: &NodeHandle, name: &str) -> Option<String> {
    first_attr(node, name).and_then(as_str).map(str::to_string)
}

/// Read a 16-element matrix (row-major FBX convention); identity if absent or malformed.
fn node_mat16(node: &NodeHandle, name: &str) -> [f64; 16] {
    node_f64_array(node, name)
        .and_then(|v| <[f64; 16]>::try_from(v).ok())
        .unwrap_or(IDENTITY_16)
}

fn chunk3(flat: &[f32]) -> Vec<[f32; 3]> {
    flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

fn chunk2(flat: &[f32]) -> Vec<[f32; 2]> {
    flat.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangulate_ignores_out_of_range_control_point_indices() {
        // `PolygonVertexIndex` points at control points that don't exist in `Vertices` (a tiny
        // 1-point buffer). The hostile indices must be skipped, not panic or wrap into a bad slice.
        let vertices = vec![0.0, 0.0, 0.0]; // a single control point (indices 1..=3 are invalid)
        // A quad referencing control points 0,1,2,3 — only 0 is in range; the polygon ends at !3.
        let pvi = vec![0, 1, 2, !3];
        let tri = triangulate(&vertices, &pvi);
        // No panic; only the in-range control point could ever be emitted, so the (degenerate)
        // triangle soup contains no out-of-bounds reads.
        assert!(tri.positions.iter().all(|p| *p == [0.0, 0.0, 0.0]));
        assert_eq!(tri.positions.len(), tri.control_point_of_vertex.len());
    }

    #[test]
    fn triangulate_handles_huge_index_without_wrapping() {
        // A control-point index of i32::MAX must not wrap `usize` and slip past the bounds check.
        let vertices = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let pvi = vec![0, 1, i32::MAX, !2]; // last entry closes the polygon at control point 2
        let tri = triangulate(&vertices, &pvi);
        // The i32::MAX vertex is dropped; in-range ones (0,1,2) survive.
        assert!(tri.control_point_of_vertex.iter().all(|&c| c <= 2));
        assert_eq!(tri.positions.len(), tri.control_point_of_vertex.len());
    }

    #[test]
    fn chunkers_drop_a_trailing_partial_group() {
        // A malformed flat array whose length isn't a multiple of the component count must not
        // panic; `chunks_exact` simply ignores the remainder.
        assert_eq!(chunk3(&[1.0, 2.0, 3.0, 4.0]), vec![[1.0, 2.0, 3.0]]);
        assert_eq!(chunk2(&[1.0, 2.0, 3.0]), vec![[1.0, 2.0]]);
        assert_eq!(chunk3(&[1.0]), Vec::<[f32; 3]>::new());
    }
}
