//! glTF 2.0 importer producing the same renderer-agnostic [`avatar_mesh::RawMesh`] +
//! [`avatar_armature::Skeleton`] that the FBX path produces, so `avatar-pose` can pose and skin a
//! glTF rig identically. glTF is friendlier than FBX for non-VRChat rigs: geometry is already
//! triangulated and indexed, and every joint carries an inverse-bind matrix directly.
//!
//! Bind convention bridge: each skin cluster's `transform_link` is set to the joint's **bind-world**
//! (`inverse(inverseBindMatrix)`) stored column-major, and `transform` to the identity. `avatar-pose`
//! then derives `inverse_bind = transform_link⁻¹ · transform`, recovering the original glTF
//! inverse-bind — so the rest-pose-reproduction invariant holds the same as for FBX.

use std::collections::HashMap;

use anyhow::{Context, Result};
use avatar_armature::{Bone, Skeleton, humanoid};
use avatar_mesh::{IDENTITY_16, RawMesh, SkinCluster, SkinData};
use glam::Mat4;

/// A loaded glTF document (buffers resolved).
pub struct GltfDocument {
    doc: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
}

impl GltfDocument {
    /// Load from `.gltf`/`.glb` bytes (embedded or external buffers must be resolvable from bytes;
    /// use [`GltfDocument::import`] for files with sidecar buffers).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use avatar_gltf::GltfDocument;
    ///
    /// let bytes = std::fs::read("avatar.glb")?;
    /// let doc = GltfDocument::from_slice(&bytes)?;
    /// let meshes = doc.meshes(); // Vec<avatar_mesh::RawMesh>
    /// let skeleton = doc.skeleton();
    /// # let _ = (meshes, skeleton);
    /// # anyhow::Ok(())
    /// ```
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let (doc, buffers, _images) =
            gltf::import_slice(bytes).context("parsing glTF from bytes")?;
        Ok(GltfDocument { doc, buffers })
    }

    /// Load a `.gltf`/`.glb` file (resolving any sidecar `.bin` buffers/images).
    pub fn import<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let (doc, buffers, _images) = gltf::import(path).context("importing glTF file")?;
        Ok(GltfDocument { doc, buffers })
    }

    /// Every mesh primitive as a [`RawMesh`], with skin attached when the owning node is skinned.
    pub fn meshes(&self) -> Vec<RawMesh> {
        let mut out = Vec::new();
        for node in self.doc.nodes() {
            let Some(mesh) = node.mesh() else { continue };
            let skin = node.skin();
            for prim in mesh.primitives() {
                if let Some(m) = self.primitive_to_mesh(node.index() as i64, &prim, skin.as_ref()) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Build a [`Skeleton`] from the first skin's joints (node hierarchy → bone parents, names
    /// classified into humanoid categories so glTF rigs map like FBX ones).
    pub fn skeleton(&self) -> Skeleton {
        let Some(skin) = self.doc.skins().next() else {
            return Skeleton { bones: Vec::new() };
        };
        let joints: Vec<usize> = skin.joints().map(|n| n.index()).collect();
        let joint_set: std::collections::HashSet<usize> = joints.iter().copied().collect();
        let parent_of = self.child_to_parent();

        let bones = skin
            .joints()
            .map(|n| {
                let name = n
                    .name()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("joint{}", n.index()));
                Bone {
                    id: n.index() as i64,
                    subclass: "LimbNode".to_string(),
                    parent: nearest_joint_ancestor(n.index(), &parent_of, &joint_set)
                        .map(|i| i as i64),
                    info: humanoid::classify(&name),
                    name,
                }
            })
            .collect();
        Skeleton { bones }
    }

    fn primitive_to_mesh(
        &self,
        node_id: i64,
        prim: &gltf::Primitive,
        skin: Option<&gltf::Skin>,
    ) -> Option<RawMesh> {
        let reader = prim.reader(|b| self.buffers.get(b.index()).map(|d| d.0.as_slice()));
        let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
        let vcount = positions.len();
        // The whole interchange model — indices and the control-point map — is `u32`, so a
        // primitive with more than `u32::MAX` vertices can't be represented; reject it (returning
        // `None` degrades to "skip this primitive") rather than wrapping `vcount as u32`.
        if vcount > u32::MAX as usize {
            return None;
        }
        let indices: Vec<u32> = match reader.read_indices() {
            Some(r) => r.into_u32().collect(),
            None => (0..vcount as u32).collect(),
        };
        let normals = reader.read_normals().map(|r| r.collect::<Vec<[f32; 3]>>());
        let uvs = reader
            .read_tex_coords(0)
            .map(|r| r.into_f32().collect::<Vec<[f32; 2]>>());

        // glTF vertices are already control points: identity mapping.
        let control_point_of_vertex: Vec<u32> = (0..vcount as u32).collect();

        let skin_data = skin.and_then(|skin| {
            let joints = reader.read_joints(0)?.into_u16().collect::<Vec<[u16; 4]>>();
            let weights = reader
                .read_weights(0)?
                .into_f32()
                .collect::<Vec<[f32; 4]>>();
            Some(self.build_skin(skin, &joints, &weights))
        });

        Some(RawMesh {
            model_id: node_id,
            positions,
            normals,
            uvs,
            indices,
            control_point_of_vertex,
            skin: skin_data,
            // glTF material/texture import is not wired into the preview yet.
            materials: Vec::new(),
            material_of_triangle: Vec::new(),
            polygon_of_triangle: Vec::new(),
        })
    }

    fn build_skin(&self, skin: &gltf::Skin, joints: &[[u16; 4]], weights: &[[f32; 4]]) -> SkinData {
        let joint_nodes: Vec<i64> = skin.joints().map(|n| n.index() as i64).collect();
        // bind-world (= inverse(IBM)) per joint, column-major, for transform_link.
        let reader = skin.reader(|b| self.buffers.get(b.index()).map(|d| d.0.as_slice()));
        let ibms: Vec<[[f32; 4]; 4]> = match reader.read_inverse_bind_matrices() {
            Some(r) => r.collect(),
            None => vec![[[0.0; 4]; 4]; joint_nodes.len()], // sentinel → identity below
        };
        let bind_world_cols = |local: usize| -> [f64; 16] {
            let ibm = ibms.get(local).copied().unwrap_or([[0.0; 4]; 4]);
            let m = Mat4::from_cols_array_2d(&ibm);
            let bind = if m == Mat4::ZERO {
                Mat4::IDENTITY
            } else {
                m.inverse()
            };
            bind.to_cols_array().map(|x| x as f64)
        };

        // Invert the per-vertex influences into per-joint clusters.
        let mut idx_w: HashMap<usize, (Vec<u32>, Vec<f32>)> = HashMap::new();
        for (v, (j4, w4)) in joints.iter().zip(weights).enumerate() {
            for k in 0..4 {
                let w = w4[k];
                if w > 0.0 {
                    let entry = idx_w.entry(j4[k] as usize).or_default();
                    entry.0.push(v as u32);
                    entry.1.push(w);
                }
            }
        }

        let mut clusters = Vec::new();
        for (local, &bone_id) in joint_nodes.iter().enumerate() {
            let (indexes, weights) = idx_w.remove(&local).unwrap_or_default();
            clusters.push(SkinCluster {
                bone_id,
                indexes,
                weights,
                transform_link: bind_world_cols(local),
                transform: IDENTITY_16,
            });
        }
        SkinData { clusters }
    }

    /// child node index -> parent node index, over the whole document.
    fn child_to_parent(&self) -> HashMap<usize, usize> {
        let mut map = HashMap::new();
        for node in self.doc.nodes() {
            for child in node.children() {
                map.insert(child.index(), node.index());
            }
        }
        map
    }
}

/// Walk up `parent_of` from `start` to the first ancestor that is itself a joint.
fn nearest_joint_ancestor(
    start: usize,
    parent_of: &HashMap<usize, usize>,
    joints: &std::collections::HashSet<usize>,
) -> Option<usize> {
    let mut cur = parent_of.get(&start).copied();
    let mut guard = 0;
    while let Some(p) = cur {
        if joints.contains(&p) {
            return Some(p);
        }
        cur = parent_of.get(&p).copied();
        guard += 1;
        if guard > 4096 {
            break; // malformed cyclic hierarchy guard
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Wrap a JSON string + binary buffer into a GLB blob (no base64, no committed file).
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json_b = json.as_bytes().to_vec();
        while !json_b.len().is_multiple_of(4) {
            json_b.push(b' ');
        }
        let mut bin_b = bin.to_vec();
        while !bin_b.len().is_multiple_of(4) {
            bin_b.push(0);
        }
        let total = 12 + 8 + json_b.len() + 8 + bin_b.len();
        let mut out = Vec::new();
        out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_b.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
        out.extend_from_slice(&json_b);
        out.extend_from_slice(&(bin_b.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin_b);
        out
    }

    /// A minimal skinned glTF: a triangle (3 verts) over 2 joints. Joint0 bind = identity, joint1
    /// bind = translate(0,1,0); verts 0 & 2 bound to joint0, vert 1 to joint1.
    fn simple_skinned_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        // view0: indices u16 [0,1,2] (offset 0)
        for i in [0u16, 1, 2] {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        while bin.len() < 8 {
            bin.push(0);
        }
        // view1: POSITION f32 vec3 ×3 (offset 8)
        for p in [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]] {
            for c in p {
                bin.extend_from_slice(&(c as f32).to_le_bytes());
            }
        }
        // view2: JOINTS_0 u16 vec4 ×3 (offset 44)
        for j in [[0u16, 0, 0, 0], [1, 0, 0, 0], [0, 0, 0, 0]] {
            for c in j {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        // view3: WEIGHTS_0 f32 vec4 ×3 (offset 68)
        for _ in 0..3 {
            for c in [1.0f32, 0.0, 0.0, 0.0] {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        // view4: inverseBindMatrices f32 mat4 ×2 (offset 116), column-major
        let ibm0 = Mat4::IDENTITY.to_cols_array();
        let ibm1 = Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0)).to_cols_array();
        for m in [ibm0, ibm1] {
            for c in m {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        assert_eq!(bin.len(), 244);

        let json = r#"{
          "asset":{"version":"2.0"},
          "scene":0,
          "scenes":[{"nodes":[0,2]}],
          "nodes":[
            {"children":[1]},
            {"translation":[0,1,0]},
            {"mesh":0,"skin":0}
          ],
          "meshes":[{"primitives":[{"attributes":{"POSITION":1,"JOINTS_0":2,"WEIGHTS_0":3},"indices":0,"mode":4}]}],
          "skins":[{"joints":[0,1],"inverseBindMatrices":4,"skeleton":0}],
          "buffers":[{"byteLength":244}],
          "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":6},
            {"buffer":0,"byteOffset":8,"byteLength":36},
            {"buffer":0,"byteOffset":44,"byteLength":24},
            {"buffer":0,"byteOffset":68,"byteLength":48},
            {"buffer":0,"byteOffset":116,"byteLength":128}
          ],
          "accessors":[
            {"bufferView":0,"componentType":5123,"count":3,"type":"SCALAR"},
            {"bufferView":1,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[1.0,1.0,0.0]},
            {"bufferView":2,"componentType":5123,"count":3,"type":"VEC4"},
            {"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"},
            {"bufferView":4,"componentType":5126,"count":2,"type":"MAT4"}
          ]
        }"#;
        glb(json, &bin)
    }

    #[test]
    fn imports_geometry_skin_and_skeleton() {
        let doc = GltfDocument::from_slice(&simple_skinned_glb()).unwrap();
        let meshes = doc.meshes();
        assert_eq!(meshes.len(), 1);
        let mesh = &meshes[0];
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        let skin = mesh.skin.as_ref().expect("skinned");
        assert_eq!(skin.clusters.len(), 2);
        assert_eq!(skin.clusters[0].bone_id, 0);
        assert_eq!(skin.clusters[1].bone_id, 1);

        let skel = doc.skeleton();
        assert_eq!(skel.bones.len(), 2);
        assert_eq!(skel.bones[0].parent, None);
        assert_eq!(skel.bones[1].parent, Some(0), "joint1 is a child of joint0");
    }

    #[test]
    fn rest_pose_reproduces_mesh_parity_with_fbx() {
        let doc = GltfDocument::from_slice(&simple_skinned_glb()).unwrap();
        let mesh = &doc.meshes()[0];
        let skel = doc.skeleton();

        let posed = avatar_pose::PosedSkeleton::from_skinned_mesh(&skel, mesh);
        let rest = posed.rest_pose();
        let vskin = posed.build_vertex_skin(mesh);
        let out = avatar_pose::cpu_skin(mesh, &vskin, &posed.palette(&rest));

        for (a, b) in out.iter().zip(&mesh.positions) {
            assert!(
                (Vec3::from_array(*a) - Vec3::from_array(*b)).length() < 1e-4,
                "glTF rest pose must reproduce the input vertices"
            );
        }
    }
}
