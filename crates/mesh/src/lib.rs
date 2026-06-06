//! Renderer-agnostic mesh + skin interchange types.
//!
//! These are plain data (`#[repr]`-free POD) with **no linear-algebra or format dependency** —
//! deliberately so. Importers (`avatar-fbx`, `avatar-gltf`) *produce* a [`RawMesh`]; the runtime
//! posing layer (`avatar-pose`) *consumes* it and owns all matrix math. Keeping this crate
//! math-free lets both importers stay free of `glam`, and lets the type cross crate boundaries
//! without coupling callers to a specific `glam` version.
//!
//! Matrices are stored as 16 `f64` in **FBX row-major convention** (translation in elements
//! `[12..=14]`). `avatar-pose` converts to `glam::Mat4` at its boundary.

use serde::Serialize;

/// One importer-produced mesh: triangulated geometry plus optional skin.
///
/// `positions` (and any `normals`/`uvs`) are per **emitted vertex** after triangulation. Skin
/// weights, however, are keyed by FBX **control point**, so [`control_point_of_vertex`] maps each
/// emitted vertex back to its source control point — the bridge `avatar-pose` uses to attach
/// weights. Without it, weights cannot be reattached after fan-triangulation.
///
/// [`control_point_of_vertex`]: RawMesh::control_point_of_vertex
#[derive(Debug, Clone, Serialize)]
pub struct RawMesh {
    /// FBX object id (or glTF mesh index) of the mesh node this came from.
    pub model_id: i64,
    /// Triangulated vertex positions in mesh-local space.
    pub positions: Vec<[f32; 3]>,
    /// Per-emitted-vertex normals, if the source layout was understood.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Per-emitted-vertex UVs, if the source layout was understood.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Triangle index buffer into `positions` (3 indices per triangle).
    pub indices: Vec<u32>,
    /// For each emitted vertex, the source control-point index it was expanded from. Parallel to
    /// `positions`.
    pub control_point_of_vertex: Vec<u32>,
    /// Skin binding, if the mesh is skinned. `None` for a static mesh.
    pub skin: Option<SkinData>,
}

impl RawMesh {
    /// Number of emitted (triangulated) vertices.
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// Number of control points (distinct source positions) this mesh expanded from.
    pub fn control_point_count(&self) -> usize {
        self.control_point_of_vertex
            .iter()
            .copied()
            .max()
            .map_or(0, |m| m as usize + 1)
    }

    /// True if the mesh carries skin weights.
    pub fn is_skinned(&self) -> bool {
        self.skin.is_some()
    }
}

/// Skin binding for one mesh: the set of bone clusters that influence its control points.
#[derive(Debug, Clone, Serialize)]
pub struct SkinData {
    pub clusters: Vec<SkinCluster>,
}

/// One bone's influence over a mesh — the FBX `SubDeformer`(`Cluster`) shape.
///
/// `indexes`/`weights` are parallel and keyed by **control point** (pre-triangulation). The two
/// matrices are the bind-time transforms an importer reads directly from the file:
/// - `transform_link` — the bone's world transform at bind time (FBX `TransformLink`).
/// - `transform` — the mesh/geometry's world transform at bind time (FBX `Transform`).
///
/// `avatar-pose` derives the inverse-bind as `transform_link⁻¹ · transform` (NOT `transform_link⁻¹`
/// alone, which silently breaks whenever the mesh bind transform is not identity). glTF importers
/// fill `transform_link` with `inverse(inverseBindMatrix)` and `transform` with the identity, which
/// reduces the same formula to the glTF inverse-bind.
#[derive(Debug, Clone, Serialize)]
pub struct SkinCluster {
    /// Object id of the bone (FBX `Model`/`LimbNode`) this cluster drives.
    pub bone_id: i64,
    /// Control-point indices this cluster influences.
    pub indexes: Vec<u32>,
    /// Weight per index, parallel to `indexes`.
    pub weights: Vec<f32>,
    /// Bone world transform at bind (`TransformLink`), row-major.
    pub transform_link: [f64; 16],
    /// Mesh/geometry world transform at bind (`Transform`), row-major.
    pub transform: [f64; 16],
}

/// The 4×4 identity in the row-major 16-`f64` convention used by [`SkinCluster`].
pub const IDENTITY_16: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
];
