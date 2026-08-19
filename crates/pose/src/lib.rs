//! Runtime posing + skinning for a rig: turn a [`Skeleton`] + [`RawMesh`] bind data into world
//! bone matrices, a GPU-ready bone-matrix **palette**, and (optionally) CPU-skinned vertices.
//!
//! Renderer-agnostic by design — the viewport owns the wgpu draw; this crate only produces data.
//! It is the consumer of the bind matrices surfaced by the importers (`avatar-fbx`,
//! `avatar-gltf`), and the only place `glam` lives in the workspace's runtime tier.
//!
//! ## The bind math (why it's robust)
//!
//! Each bone's bind-world matrix comes straight from the file (FBX `TransformLink`; glTF
//! `inverse(inverseBindMatrix)`), so we never reconstruct it from Lcl Rotation plus PreRotation,
//! sidestepping the FBX pivot/pre-rotation morass entirely for skinned bones. The inverse-bind is
//! `transform_link.inverse() * transform` (the mesh bind), and each bone's local-bind is
//! `bind_world[parent].inverse() * bind_world[b]`. At rest, every world matrix equals its bind
//! world, so the palette reproduces the mesh exactly — the invariant the tests assert.

pub mod ik;

use std::collections::HashMap;

use avatar_armature::{HumanBone, HumanoidMapping, Skeleton};
use avatar_fbx::{FbxScene, LocalTransform};
use avatar_mesh::RawMesh;
use glam::{EulerRot, Mat4, Quat, Vec3};

/// Load a 4×4 from FBX's 16 row-major doubles. FBX is row-major with row-vectors; glam is
/// column-major with column-vectors, so the *same* 16 contiguous floats fed to `from_cols_array`
/// yield glam's column-vector form of the identical transform — **do not transpose** (translation
/// lands in `col(3)`).
pub fn mat4_from_fbx(m: &[f64; 16]) -> Mat4 {
    Mat4::from_cols_array(&m.map(|x| x as f32))
}

/// Convert an FBX `Lcl` TRS (Euler degrees, XYZ order) to a local matrix. Used only for bones that
/// carry no skin cluster (twist/accessory bones); it ignores `PreRotation`/pivots, which
/// `avatar-fbx` does not read — acceptable because such bones drive no vertices.
/// An FBX node's local matrix from its `Lcl Translation/Rotation/Scaling` (XYZ Euler, degrees).
/// Note: `PreRotation`/pivots are not folded in — this is the plain Lcl TRS, which is what a
/// Blender-style export carries on mesh and armature nodes.
pub fn lcl_to_mat4(t: &LocalTransform) -> Mat4 {
    let [tx, ty, tz] = t.translation.unwrap_or([0.0; 3]);
    let [rx, ry, rz] = t.rotation.unwrap_or([0.0; 3]);
    let [sx, sy, sz] = t.scaling.unwrap_or([1.0; 3]);
    let rot = Quat::from_euler(
        EulerRot::XYZ,
        (rx as f32).to_radians(),
        (ry as f32).to_radians(),
        (rz as f32).to_radians(),
    );
    Mat4::from_scale_rotation_translation(
        Vec3::new(sx as f32, sy as f32, sz as f32),
        rot,
        Vec3::new(tx as f32, ty as f32, tz as f32),
    )
}

/// The global matrix of FBX `Model` node `id`: its Lcl TRS composed with every `Model` ancestor's.
/// For a skinned mesh node this is the space its raw control points live in (e.g. Blender's
/// `-90° X` on the mesh object that turns Z-up geometry into the file's Y-up world).
pub fn model_global_matrix(scene: &FbxScene, id: i64) -> Mat4 {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(i) = cur {
        let Some(o) = scene.object(i) else {
            break;
        };
        if o.class != "Model" {
            break;
        }
        chain.push(lcl_to_mat4(&o.transform));
        cur = scene.parent_of(i);
        guard += 1;
        if guard > 1024 {
            break;
        }
    }
    chain.iter().rev().fold(Mat4::IDENTITY, |acc, m| acc * *m)
}

/// Up-to-4 bone influences for one vertex (GPU linear-blend skinning layout).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexSkin {
    /// Compact bone indices (into a [`PosedSkeleton`]'s bone array).
    pub joints: [u16; 4],
    /// Weights, normalized to sum to 1.
    pub weights: [f32; 4],
}

impl Default for VertexSkin {
    fn default() -> Self {
        VertexSkin {
            joints: [0; 4],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// An immutable rig: bind data + topology in a compact `0..N` bone index space. Build it once, then
/// evaluate many [`Pose`]s against it.
#[derive(Debug, Clone)]
pub struct PosedSkeleton {
    /// `bone_ids[i]` is the FBX/glTF object id of compact bone `i`.
    bone_ids: Vec<i64>,
    /// Compact-index parent of each bone (`None` = root).
    parent: Vec<Option<usize>>,
    /// Parent-before-child evaluation order (cycle-safe).
    order: Vec<usize>,
    /// Rest local transforms (the default pose).
    local_bind: Vec<Mat4>,
    /// `world[b]·inverse_bind[b]` is the palette; at rest it is identity.
    inverse_bind: Vec<Mat4>,
    id_to_index: HashMap<i64, usize>,
}

/// A mutable set of per-bone local transforms, index-aligned with its [`PosedSkeleton`].
#[derive(Debug, Clone)]
pub struct Pose {
    pub local: Vec<Mat4>,
}

impl Pose {
    /// Replace bone `idx`'s local transform.
    pub fn set_local(&mut self, idx: usize, m: Mat4) {
        self.local[idx] = m;
    }

    /// Set bone `idx`'s local transform from translation/rotation/scale.
    pub fn set_local_trs(&mut self, idx: usize, t: Vec3, r: Quat, s: Vec3) {
        self.local[idx] = Mat4::from_scale_rotation_translation(s, r, t);
    }
}

impl PosedSkeleton {
    /// Build directly from compact parts: each bone's bind-world and inverse-bind matrix (both in
    /// the same space). This is the renderer-agnostic core both importers feed.
    pub fn from_parts(
        bone_ids: Vec<i64>,
        parent: Vec<Option<usize>>,
        bind_world: Vec<Mat4>,
        inverse_bind: Vec<Mat4>,
    ) -> Self {
        let n = bone_ids.len();
        assert_eq!(parent.len(), n);
        assert_eq!(bind_world.len(), n);
        assert_eq!(inverse_bind.len(), n);

        let order = topo_order(&parent);
        let local_bind = (0..n)
            .map(|b| match parent[b] {
                Some(p) => bind_world[p].inverse() * bind_world[b],
                None => bind_world[b],
            })
            .collect();
        let id_to_index = bone_ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();

        PosedSkeleton {
            bone_ids,
            parent,
            order,
            local_bind,
            inverse_bind,
            id_to_index,
        }
    }

    /// Build from an `avatar-armature` skeleton + an `avatar-fbx` scene (for `Lcl` fallback on
    /// non-skinned bones) + the mesh's skin. Clustered bones take their authoritative bind from
    /// `TransformLink`; the rest are FK-composed from their `Lcl` TRS.
    pub fn from_fbx(skeleton: &Skeleton, scene: &FbxScene, mesh: &RawMesh) -> Self {
        Self::from_skin_inner(skeleton, mesh, Some(scene))
    }

    /// Build from a skeleton + skinned mesh **without** an FBX scene. Suitable when every posable
    /// bone carries a skin cluster (e.g. a glTF rig, where each joint has an inverse-bind matrix);
    /// any bone lacking a cluster gets an identity local-bind. The importer encodes each cluster's
    /// `transform_link` as its bind-world (column-major) and `transform` as the identity.
    pub fn from_skinned_mesh(skeleton: &Skeleton, mesh: &RawMesh) -> Self {
        Self::from_skin_inner(skeleton, mesh, None)
    }

    fn from_skin_inner(skeleton: &Skeleton, mesh: &RawMesh, scene: Option<&FbxScene>) -> Self {
        let bone_ids: Vec<i64> = skeleton.bones.iter().map(|b| b.id).collect();
        let index: HashMap<i64, usize> = bone_ids
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i))
            .collect();
        let parent: Vec<Option<usize>> = skeleton
            .bones
            .iter()
            .map(|b| b.parent.and_then(|p| index.get(&p).copied()))
            .collect();

        // Per-bone cluster bind (TransformLink, Transform), if the bone is skinned.
        let mut cluster: HashMap<usize, (Mat4, Mat4)> = HashMap::new();
        if let Some(skin) = &mesh.skin {
            for c in &skin.clusters {
                if let Some(&i) = index.get(&c.bone_id) {
                    cluster.insert(
                        i,
                        (
                            mat4_from_fbx(&c.transform_link),
                            mat4_from_fbx(&c.transform),
                        ),
                    );
                }
            }
        }

        let order = topo_order(&parent);
        let mut bind_world = vec![Mat4::IDENTITY; bone_ids.len()];
        let mut inverse_bind = vec![Mat4::IDENTITY; bone_ids.len()];
        for &b in &order {
            match cluster.get(&b) {
                Some(&(link, transform)) => {
                    bind_world[b] = link;
                    inverse_bind[b] = link.inverse() * transform;
                }
                None => {
                    // Non-skinned bone: FK from its Lcl TRS when we have the FBX scene, else an
                    // identity local (glTF rigs never hit this — every joint is clustered).
                    let local = match scene.and_then(|s| s.object(bone_ids[b])) {
                        Some(o) => lcl_to_mat4(&o.transform),
                        None => Mat4::IDENTITY,
                    };
                    bind_world[b] = match parent[b] {
                        Some(p) => bind_world[p] * local,
                        None => local,
                    };
                    inverse_bind[b] = bind_world[b].inverse();
                }
            }
        }

        Self::from_parts(bone_ids, parent, bind_world, inverse_bind)
    }

    /// Number of bones.
    pub fn len(&self) -> usize {
        self.bone_ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bone_ids.is_empty()
    }

    /// Compact index of a bone by its object id.
    pub fn index_of(&self, bone_id: i64) -> Option<usize> {
        self.id_to_index.get(&bone_id).copied()
    }

    /// A fresh pose at the rest (bind) configuration.
    pub fn rest_pose(&self) -> Pose {
        Pose {
            local: self.local_bind.clone(),
        }
    }

    /// World matrices for a pose, FK-evaluated parent-before-child.
    pub fn world_matrices(&self, pose: &Pose) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.len()];
        for &b in &self.order {
            world[b] = match self.parent[b] {
                Some(p) => world[p] * pose.local[b],
                None => pose.local[b],
            };
        }
        world
    }

    /// GPU bone-matrix palette: `world[b]·inverse_bind[b]`. Identity at rest.
    pub fn palette(&self, pose: &Pose) -> Vec<Mat4> {
        let world = self.world_matrices(pose);
        (0..self.len())
            .map(|b| world[b] * self.inverse_bind[b])
            .collect()
    }

    /// The palette as raw column-major `[f32;16]` arrays — upload straight to a storage buffer
    /// without coupling the caller to this crate's `glam` version.
    pub fn palette_cols(&self, pose: &Pose) -> Vec<[f32; 16]> {
        self.palette(pose).iter().map(Mat4::to_cols_array).collect()
    }

    /// Pose a humanoid slot by an extra local rotation relative to its bind frame. No-op if the
    /// slot isn't uniquely mapped or isn't in this rig.
    pub fn pose_humanoid(
        &self,
        pose: &mut Pose,
        mapping: &HumanoidMapping,
        slot: HumanBone,
        rotation: Quat,
    ) {
        if let Some(id) = mapping.unique_id(slot)
            && let Some(i) = self.index_of(id)
        {
            pose.local[i] = self.local_bind[i] * Mat4::from_quat(rotation);
        }
    }

    /// Build per-vertex skin influences (≤4, normalized) by scattering control-point-keyed cluster
    /// weights onto emitted vertices via `RawMesh::control_point_of_vertex`.
    pub fn build_vertex_skin(&self, mesh: &RawMesh) -> Vec<VertexSkin> {
        let cp_count = mesh.control_point_count();
        let mut by_cp: Vec<Vec<(u16, f32)>> = vec![Vec::new(); cp_count];
        if let Some(skin) = &mesh.skin {
            for c in &skin.clusters {
                let Some(j) = self.index_of(c.bone_id) else {
                    continue;
                };
                for (&cp, &w) in c.indexes.iter().zip(&c.weights) {
                    if let Some(slot) = by_cp.get_mut(cp as usize) {
                        slot.push((j as u16, w));
                    }
                }
            }
        }
        let per_cp: Vec<VertexSkin> = by_cp.iter().map(|infs| finalize_influences(infs)).collect();

        mesh.control_point_of_vertex
            .iter()
            .map(|&cp| per_cp.get(cp as usize).copied().unwrap_or_default())
            .collect()
    }
}

/// Linear-blend CPU skinning: `Σ wᵢ · palette[jointᵢ] · v`. For testing/headless use; the GPU path
/// does the same sum in a vertex shader.
pub fn cpu_skin(mesh: &RawMesh, skin: &[VertexSkin], palette: &[Mat4]) -> Vec<[f32; 3]> {
    mesh.positions
        .iter()
        .enumerate()
        .map(|(k, p)| {
            let v = Vec3::from_array(*p);
            // A vertex with no skin entry, or an influence pointing past the palette, passes through
            // unskinned rather than panicking — `cpu_skin` is a headless/testing path and must not
            // crash on a malformed or mis-sized mesh/skin/palette triple.
            let Some(&s) = skin.get(k) else {
                return *p;
            };
            let mut m = Mat4::ZERO;
            for i in 0..4 {
                if s.weights[i] != 0.0
                    && let Some(j) = palette.get(s.joints[i] as usize)
                {
                    m += j.mul_scalar(s.weights[i]);
                }
            }
            // A vertex with no influence (shouldn't happen after normalization) passes through.
            if m == Mat4::ZERO {
                m = Mat4::IDENTITY;
            }
            m.transform_point3(v).to_array()
        })
        .collect()
}

/// Sort influences by weight desc, clamp to 4, renormalize to sum 1. Empty → bind to bone 0.
fn finalize_influences(infs: &[(u16, f32)]) -> VertexSkin {
    let mut v = infs.to_vec();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    v.truncate(4);
    let sum: f32 = v.iter().map(|(_, w)| *w).sum();
    let mut out = VertexSkin {
        joints: [0; 4],
        weights: [0.0; 4],
    };
    if sum > 0.0 {
        for (k, (j, w)) in v.iter().enumerate() {
            out.joints[k] = *j;
            out.weights[k] = w / sum;
        }
    } else {
        out.weights[0] = 1.0;
    }
    out
}

/// Parent-before-child ordering of a flat parent array. Cycle-safe: bounded passes, and any node
/// left unplaced by a cycle is appended rather than looping forever.
fn topo_order(parent: &[Option<usize>]) -> Vec<usize> {
    let n = parent.len();
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    for _ in 0..=n {
        let mut progressed = false;
        for i in 0..n {
            if placed[i] {
                continue;
            }
            let ready = match parent[i] {
                None => true,
                Some(p) => placed[p],
            };
            if ready {
                placed[i] = true;
                order.push(i);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    for (i, done) in placed.iter().enumerate() {
        if !done {
            order.push(i);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_mesh::{IDENTITY_16, RawMesh, SkinCluster, SkinData};

    fn translate16(x: f64, y: f64, z: f64) -> [f64; 16] {
        let mut m = IDENTITY_16;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        m
    }

    /// Two bones (Bone0 root @ origin, Bone1 child @ (0,1,0)); a 2-vertex mesh: vertex 0 bound 100%
    /// to Bone0, vertex 1 bound 100% to Bone1. Mesh bind `Transform` is identity, so the rest
    /// palette is identity and CPU-skinning reproduces the input.
    fn two_bone_mesh() -> (PosedSkeleton, RawMesh) {
        let mesh = RawMesh {
            model_id: 10,
            positions: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: None,
            uvs: None,
            indices: vec![0, 1],
            control_point_of_vertex: vec![0, 1],
            skin: Some(SkinData {
                clusters: vec![
                    SkinCluster {
                        bone_id: 30,
                        indexes: vec![0],
                        weights: vec![1.0],
                        transform_link: IDENTITY_16,
                        transform: IDENTITY_16,
                    },
                    SkinCluster {
                        bone_id: 31,
                        indexes: vec![1],
                        weights: vec![1.0],
                        transform_link: translate16(0.0, 1.0, 0.0),
                        transform: IDENTITY_16,
                    },
                ],
            }),
            materials: Vec::new(),
            material_of_triangle: Vec::new(),
            polygon_of_triangle: Vec::new(),
        };
        // bone_ids parallel: [Bone0=30 root, Bone1=31 child of 0].
        let posed = PosedSkeleton::from_parts(
            vec![30, 31],
            vec![None, Some(0)],
            vec![
                mat4_from_fbx(&IDENTITY_16),
                mat4_from_fbx(&translate16(0.0, 1.0, 0.0)),
            ],
            vec![
                mat4_from_fbx(&IDENTITY_16),
                mat4_from_fbx(&translate16(0.0, 1.0, 0.0)).inverse(),
            ],
        );
        (posed, mesh)
    }

    #[test]
    fn matrix_convention_no_transpose() {
        // Row-major translate(1,2,3): glam must see translation in column 3.
        let m = mat4_from_fbx(&translate16(1.0, 2.0, 3.0));
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(m.w_axis.truncate(), Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn rest_pose_reproduces_mesh_and_palette_is_identity() {
        let (posed, mesh) = two_bone_mesh();
        let rest = posed.rest_pose();
        for m in posed.palette(&rest) {
            assert!(
                m.abs_diff_eq(Mat4::IDENTITY, 1e-5),
                "rest palette is identity"
            );
        }
        let vskin = posed.build_vertex_skin(&mesh);
        let out = cpu_skin(&mesh, &vskin, &posed.palette(&rest));
        for (a, b) in out.iter().zip(&mesh.positions) {
            assert!((Vec3::from_array(*a) - Vec3::from_array(*b)).length() < 1e-5);
        }
    }

    #[test]
    fn translating_a_bone_moves_its_bound_vertex() {
        let (posed, mesh) = two_bone_mesh();
        let mut pose = posed.rest_pose();
        // Translate Bone1's local frame by +10 in x; its 100%-bound vertex must move by (10,0,0).
        let i = posed.index_of(31).unwrap();
        pose.set_local(
            i,
            Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)) * posed.local_bind[i],
        );

        let vskin = posed.build_vertex_skin(&mesh);
        let out = cpu_skin(&mesh, &vskin, &posed.palette(&pose));
        // vertex 0 (Bone0) unchanged; vertex 1 (Bone1) moved by (10,0,0).
        assert!((Vec3::from_array(out[0]) - Vec3::new(0.0, 0.0, 0.0)).length() < 1e-5);
        assert!((Vec3::from_array(out[1]) - Vec3::new(10.0, 1.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn topo_order_terminates_on_cycle() {
        // 0 -> 1 -> 0 cycle: must not loop forever, and must place every index once.
        let order = topo_order(&[Some(1), Some(0)]);
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn cpu_skin_tolerates_short_skin_and_oob_joint() {
        // A headless/testing path must not panic on a mis-sized mesh/skin/palette triple.
        let (_, mut mesh) = two_bone_mesh();
        mesh.positions = vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        // skin shorter than the mesh (1 entry for 2 vertices) and a joint past an empty palette.
        let skin = vec![VertexSkin {
            joints: [99, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }];
        let palette: Vec<Mat4> = Vec::new();
        let out = cpu_skin(&mesh, &skin, &palette);
        // Both vertices pass through unchanged: vertex 0's only influence is out of palette range
        // (no contribution → identity passthrough), vertex 1 has no skin entry at all.
        assert_eq!(out, vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
    }

    #[test]
    fn euler_xyz_rotation_direction() {
        // Pin glam's EulerRot::XYZ against a known FBX Lcl Rotation of 90° about Z: x-axis -> y.
        let t = LocalTransform {
            rotation: Some([0.0, 0.0, 90.0]),
            ..Default::default()
        };
        let m = lcl_to_mat4(&t);
        assert!((m.transform_vector3(Vec3::X) - Vec3::Y).length() < 1e-5);
    }
}
