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
///
/// # Example
///
/// ```
/// use avatar_mesh::RawMesh;
///
/// // A static (unskinned) quad, two triangles, no material info.
/// let mesh = RawMesh {
///     model_id: 0,
///     positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
///     normals: None,
///     uvs: None,
///     indices: vec![0, 1, 2, 0, 2, 3],
///     control_point_of_vertex: vec![0, 1, 2, 3],
///     skin: None,
///     materials: Vec::new(),
///     material_of_triangle: Vec::new(),
///     polygon_of_triangle: Vec::new(),
/// };
/// assert_eq!(mesh.vertex_count(), 4);
/// assert!(!mesh.is_skinned());
/// // Every mesh has at least slot 0, so the slot count floors at 1.
/// assert_eq!(mesh.material_slot_count(), 1);
/// ```
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
    /// Materials assigned to this mesh, in the slot order the file references them
    /// (`material_of_triangle` indexes into this list). Empty if no material info was found.
    pub materials: Vec<MeshMaterial>,
    /// For each triangle (one entry per 3 entries of `indices`), the material **slot** it uses —
    /// an index into [`materials`]. Empty when the layout was a single material / not understood
    /// (treat as slot 0).
    ///
    /// [`materials`]: RawMesh::materials
    pub material_of_triangle: Vec<u32>,
    /// For each triangle, the index of the source **polygon** it was triangulated from (FBX
    /// `PolygonVertexIndex` order) — what a per-polygon layer edit (material reassignment) is
    /// keyed by. Empty when the importer has no polygon notion (each triangle is its own polygon).
    pub polygon_of_triangle: Vec<u32>,
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

    /// Number of material slots: the larger of the materials list and any referenced slot, with a
    /// floor of 1 (every mesh has at least slot 0). Useful when splitting a mesh by material.
    pub fn material_slot_count(&self) -> usize {
        let max_ref = self
            .material_of_triangle
            .iter()
            .copied()
            .max()
            .map_or(0, |m| m as usize + 1);
        self.materials.len().max(max_ref).max(1)
    }

    /// The material slot for triangle `tri` (0 when no per-triangle info is present).
    pub fn triangle_material(&self, tri: usize) -> usize {
        self.material_of_triangle
            .get(tri)
            .map_or(0, |&s| s as usize)
    }
}

/// One material assigned to a mesh — the renderer-relevant subset of an FBX/glTF material.
///
/// Plain data, no format coupling: importers (`avatar-fbx`, later `avatar-gltf`) fill it, and the
/// preview layer resolves [`TextureRef`] paths/bytes into pixels at its own boundary.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MeshMaterial {
    /// The material's name (for diagnostics / matching).
    pub name: String,
    /// Diffuse / base colour tint (linear-ish RGBA as stored), if the file carried one.
    pub diffuse_color: Option<[f32; 4]>,
    /// Diffuse / base-colour texture, if the material referenced one.
    pub texture: Option<TextureRef>,
}

/// A reference to a material's texture image, *unresolved*. An FBX texture may be an external file
/// (relative to the FBX and/or an absolute authoring path) and/or embedded as raw bytes in the file.
/// The consumer decides how to fetch the pixels (decode `embedded`, else resolve a path on disk).
#[derive(Debug, Clone, Default, Serialize)]
pub struct TextureRef {
    /// `RelativeFilename` as stored — relative to the model file's directory.
    pub relative: Option<String>,
    /// `FileName` as stored — usually an absolute authoring-machine path.
    pub absolute: Option<String>,
    /// Raw image bytes embedded in the file (an FBX `Video`/`Media` `Content` blob), if present.
    #[serde(skip)]
    pub embedded: Option<Vec<u8>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh_with(materials: usize, mat_of_tri: Vec<u32>) -> RawMesh {
        RawMesh {
            model_id: 0,
            positions: Vec::new(),
            normals: None,
            uvs: None,
            indices: Vec::new(),
            control_point_of_vertex: Vec::new(),
            skin: None,
            materials: vec![MeshMaterial::default(); materials],
            material_of_triangle: mat_of_tri,
            polygon_of_triangle: vec![],
        }
    }

    #[test]
    fn slot_count_floors_at_one() {
        assert_eq!(mesh_with(0, vec![]).material_slot_count(), 1);
        assert_eq!(mesh_with(2, vec![]).material_slot_count(), 2);
        // A referenced slot beyond the materials list still counts.
        assert_eq!(mesh_with(1, vec![0, 3]).material_slot_count(), 4);
    }

    #[test]
    fn triangle_material_defaults_to_zero() {
        let m = mesh_with(2, vec![0, 1, 1]);
        assert_eq!(m.triangle_material(0), 0);
        assert_eq!(m.triangle_material(2), 1);
        // No entry → slot 0.
        assert_eq!(m.triangle_material(99), 0);
        assert_eq!(mesh_with(1, vec![]).triangle_material(0), 0);
    }
}
