//! Geometry + skin extraction: turn the FBX `Geometry`/`Deformer` nodes into renderer-agnostic
//! [`avatar_mesh::RawMesh`]s. This is the data a runtime needs to *render* a posed avatar — the
//! piece `avatar-fbx` did not previously surface.
//!
//! It operates on the retained [`Tree`] (the array payloads live there, not on the flattened
//! [`FbxScene`]), but uses the scene's connection graph for the object wiring.

use std::collections::HashMap;

use anyhow::Result;
use avatar_mesh::{IDENTITY_16, RawMesh, SkinCluster, SkinData};
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

        meshes.push(RawMesh {
            model_id,
            positions: tri.positions,
            normals,
            uvs,
            indices: tri.indices,
            control_point_of_vertex: tri.control_point_of_vertex,
            skin,
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

/// Result of triangulating one geometry: emitted vertices plus the two index maps a skinner needs.
struct Triangulated {
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    /// Source control-point index per emitted vertex (for attaching skin weights).
    control_point_of_vertex: Vec<u32>,
    /// Source polygon-vertex position (index into `PolygonVertexIndex`) per emitted vertex —
    /// internal, used to resolve `ByPolygonVertex` layer elements.
    polygon_vertex_of_vertex: Vec<u32>,
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

    // (control-point index, polygon-vertex position) for the polygon being accumulated.
    let mut poly: Vec<(u32, u32)> = Vec::new();
    for (pv_pos, &raw) in pvi.iter().enumerate() {
        let (cp, end) = if raw < 0 {
            ((!raw) as u32, true)
        } else {
            (raw as u32, false)
        };
        poly.push((cp, pv_pos as u32));
        if end {
            for i in 1..poly.len().saturating_sub(1) {
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
            }
            poly.clear();
        }
    }

    Triangulated {
        positions,
        indices,
        control_point_of_vertex: cp_of_vertex,
        polygon_vertex_of_vertex: pv_of_vertex,
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
