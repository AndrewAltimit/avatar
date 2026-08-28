//! In-code synthetic FBX fixtures (feature `fbx`).
//!
//! Following the workspace convention — never commit user FBX, synthesize binary FBX in-memory via
//! the `fbxcel` writer — these builders return the *bytes* of a tiny but structurally-real FBX so
//! the FBX read paths (armature analysis, geometry stats) get hermetic, machine-independent golden
//! coverage. Object ids and names are fixed, so the produced bytes (and the reports derived from
//! them) are deterministic.

use std::io::Cursor;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// Serialize an `fbxcel` tree to binary FBX 7.4 bytes.
fn to_bytes(tree: &Tree) -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).expect("create FBX writer");
    w.write_tree(tree).expect("write tree");
    w.finalize_and_flush(&FbxFooter::default())
        .expect("finalize FBX")
        .into_inner()
}

/// Bytes of a complete, humanoid-ready Mixamo-style skeleton: a full spine (Hips→Spine→Chest→Neck→
/// Head), both arms (Shoulder→Arm→ForeArm→Hand + a finger), and both legs (UpLeg→Leg→Foot→Toe), all
/// wired by `OO` connections, plus a leaf `*_End` and a baked uniform scale on the root.
///
/// Every Unity-required humanoid bone is present, so `avatar_armature::analyze` reports it
/// humanoid-ready; the finger and `_End` bones exercise the finger/leaf exclusion paths. No mesh
/// geometry — this fixture targets the *armature* surface (where "armature not set up right" lives);
/// geometry-stats over it report zero triangles, which is itself a useful shape to pin.
pub fn humanoid_skeleton() -> Vec<u8> {
    let tree = tree_v7400! {
        GlobalSettings: {
            Properties70: {
                P: ["UnitScaleFactor", "double", "Number", "", 1.0f64] {},
                P: ["UpAxis", "int", "Integer", "", 1i32] {},
            },
        },
        Objects: {
            // Spine chain.
            Model: [100i64, "mixamorig:Hips\u{0}\u{1}Model", "LimbNode"] {
                Properties70: {
                    P: ["Lcl Scaling", "Lcl Scaling", "", "A", 1.0f64, 1.0f64, 1.0f64] {},
                },
            },
            Model: [101i64, "mixamorig:Spine\u{0}\u{1}Model", "LimbNode"] {},
            Model: [102i64, "mixamorig:Spine1\u{0}\u{1}Model", "LimbNode"] {},
            Model: [103i64, "mixamorig:Neck\u{0}\u{1}Model", "LimbNode"] {},
            Model: [104i64, "mixamorig:Head\u{0}\u{1}Model", "LimbNode"] {},
            Model: [105i64, "mixamorig:HeadTop_End\u{0}\u{1}Model", "LimbNode"] {},
            // Left arm.
            Model: [110i64, "mixamorig:LeftShoulder\u{0}\u{1}Model", "LimbNode"] {},
            Model: [111i64, "mixamorig:LeftArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [112i64, "mixamorig:LeftForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [113i64, "mixamorig:LeftHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [114i64, "mixamorig:LeftHandMiddle1\u{0}\u{1}Model", "LimbNode"] {},
            // Right arm.
            Model: [120i64, "mixamorig:RightShoulder\u{0}\u{1}Model", "LimbNode"] {},
            Model: [121i64, "mixamorig:RightArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [122i64, "mixamorig:RightForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [123i64, "mixamorig:RightHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [124i64, "mixamorig:RightHandMiddle1\u{0}\u{1}Model", "LimbNode"] {},
            // Left leg.
            Model: [130i64, "mixamorig:LeftUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [131i64, "mixamorig:LeftLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [132i64, "mixamorig:LeftFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [133i64, "mixamorig:LeftToeBase\u{0}\u{1}Model", "LimbNode"] {},
            // Right leg.
            Model: [140i64, "mixamorig:RightUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [141i64, "mixamorig:RightLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [142i64, "mixamorig:RightFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [143i64, "mixamorig:RightToeBase\u{0}\u{1}Model", "LimbNode"] {},
        },
        Connections: {
            // Spine.
            C: ["OO", 101i64, 100i64] {},
            C: ["OO", 102i64, 101i64] {},
            C: ["OO", 103i64, 102i64] {},
            C: ["OO", 104i64, 103i64] {},
            C: ["OO", 105i64, 104i64] {},
            // Left arm off the chest.
            C: ["OO", 110i64, 102i64] {},
            C: ["OO", 111i64, 110i64] {},
            C: ["OO", 112i64, 111i64] {},
            C: ["OO", 113i64, 112i64] {},
            C: ["OO", 114i64, 113i64] {},
            // Right arm off the chest.
            C: ["OO", 120i64, 102i64] {},
            C: ["OO", 121i64, 120i64] {},
            C: ["OO", 122i64, 121i64] {},
            C: ["OO", 123i64, 122i64] {},
            C: ["OO", 124i64, 123i64] {},
            // Left leg off the hips.
            C: ["OO", 130i64, 100i64] {},
            C: ["OO", 131i64, 130i64] {},
            C: ["OO", 132i64, 131i64] {},
            C: ["OO", 133i64, 132i64] {},
            // Right leg off the hips.
            C: ["OO", 140i64, 100i64] {},
            C: ["OO", 141i64, 140i64] {},
            C: ["OO", 142i64, 141i64] {},
            C: ["OO", 143i64, 142i64] {},
        },
    };
    to_bytes(&tree)
}

// --- humanoid_skinned -------------------------------------------------------------------------

/// One rig bone of the skinned demo figure: `(id, name, parent id, world position in cm)`.
type BoneDef = (i64, &'static str, Option<i64>, [f64; 3]);

/// The T-posed ~1.6 m figure's bones (Y-up, centimetres, left = +X). Same ids and Mixamo names as
/// [`humanoid_skeleton`] so both fixtures map to the same humanoid slots.
const SKINNED_BONES: &[BoneDef] = &[
    (100, "mixamorig:Hips", None, [0.0, 95.0, 0.0]),
    (101, "mixamorig:Spine", Some(100), [0.0, 105.0, 0.0]),
    (102, "mixamorig:Spine1", Some(101), [0.0, 120.0, 0.0]),
    (103, "mixamorig:Neck", Some(102), [0.0, 142.0, 0.0]),
    (104, "mixamorig:Head", Some(103), [0.0, 150.0, 0.0]),
    (105, "mixamorig:HeadTop_End", Some(104), [0.0, 170.0, 0.0]),
    (110, "mixamorig:LeftShoulder", Some(102), [5.0, 138.0, 0.0]),
    (111, "mixamorig:LeftArm", Some(110), [18.0, 138.0, 0.0]),
    (112, "mixamorig:LeftForeArm", Some(111), [44.0, 138.0, 0.0]),
    (113, "mixamorig:LeftHand", Some(112), [68.0, 138.0, 0.0]),
    (
        114,
        "mixamorig:LeftHandMiddle1",
        Some(113),
        [76.0, 138.0, 0.0],
    ),
    (
        120,
        "mixamorig:RightShoulder",
        Some(102),
        [-5.0, 138.0, 0.0],
    ),
    (121, "mixamorig:RightArm", Some(120), [-18.0, 138.0, 0.0]),
    (
        122,
        "mixamorig:RightForeArm",
        Some(121),
        [-44.0, 138.0, 0.0],
    ),
    (123, "mixamorig:RightHand", Some(122), [-68.0, 138.0, 0.0]),
    (
        124,
        "mixamorig:RightHandMiddle1",
        Some(123),
        [-76.0, 138.0, 0.0],
    ),
    (130, "mixamorig:LeftUpLeg", Some(100), [9.0, 92.0, 0.0]),
    (131, "mixamorig:LeftLeg", Some(130), [9.0, 50.0, 0.0]),
    (132, "mixamorig:LeftFoot", Some(131), [9.0, 8.0, 0.0]),
    (133, "mixamorig:LeftToeBase", Some(132), [9.0, 2.0, 12.0]),
    (140, "mixamorig:RightUpLeg", Some(100), [-9.0, 92.0, 0.0]),
    (141, "mixamorig:RightLeg", Some(140), [-9.0, 50.0, 0.0]),
    (142, "mixamorig:RightFoot", Some(141), [-9.0, 8.0, 0.0]),
    (143, "mixamorig:RightToeBase", Some(142), [-9.0, 2.0, 12.0]),
];

fn bone_pos(id: i64) -> [f64; 3] {
    SKINNED_BONES
        .iter()
        .find(|b| b.0 == id)
        .map(|b| b.3)
        .expect("known bone id")
}

/// Mesh under construction: control points with per-point normal/uv/skin, and polygons.
#[derive(Default)]
struct MeshBuild {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    uvs: Vec<[f64; 2]>,
    /// Per control point: `(bone id, weight)` list (sums to 1).
    weights: Vec<Vec<(i64, f64)>>,
    /// Polygons as control-point index lists (all triangles here).
    polygons: Vec<[u32; 3]>,
    /// Material slot per polygon, parallel to `polygons`.
    polygon_material: Vec<i32>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn norm(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl MeshBuild {
    fn push(&mut self, p: [f64; 3], n: [f64; 3], uv: [f64; 2], w: Vec<(i64, f64)>) -> u32 {
        self.positions.push(p);
        self.normals.push(norm(n));
        self.uvs.push(uv);
        self.weights.push(w);
        (self.positions.len() - 1) as u32
    }

    fn tri(&mut self, a: u32, b: u32, c: u32, material: i32) {
        self.polygons.push([a, b, c]);
        self.polygon_material.push(material);
    }

    /// A capped tube following a chain of bones (`chain[i]` = bone id; keypoints are the bone
    /// positions). `rings_per_segment` rings between consecutive keypoints; weights blend between
    /// the two bones bounding each ring.
    fn chain_tube(&mut self, chain: &[i64], radius: f64, rings_per_segment: usize, segs: usize) {
        let pts: Vec<[f64; 3]> = chain.iter().map(|&id| bone_pos(id)).collect();
        let axis = norm(sub(pts[pts.len() - 1], pts[0]));
        // A basis perpendicular to the chain axis.
        let helper = if axis[1].abs() < 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let u = norm(cross(axis, helper));
        let v = norm(cross(axis, u));
        let mut rings: Vec<Vec<u32>> = Vec::new();
        let n_seg = pts.len() - 1;
        let total_rings = n_seg * rings_per_segment + 1;
        for r in 0..total_rings {
            let (si, t) = if r == total_rings - 1 {
                (n_seg - 1, 1.0)
            } else {
                (
                    r / rings_per_segment,
                    (r % rings_per_segment) as f64 / rings_per_segment as f64,
                )
            };
            let (a, b) = (pts[si], pts[si + 1]);
            let center = [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ];
            let w = if t <= 0.0 {
                vec![(chain[si], 1.0)]
            } else if t >= 1.0 {
                vec![(chain[si + 1], 1.0)]
            } else {
                vec![(chain[si], 1.0 - t), (chain[si + 1], t)]
            };
            let along = r as f64 / (total_rings - 1) as f64;
            let ring: Vec<u32> = (0..segs)
                .map(|s| {
                    let ang = s as f64 / segs as f64 * std::f64::consts::TAU;
                    let (sn, cs) = ang.sin_cos();
                    let n = [
                        u[0] * cs + v[0] * sn,
                        u[1] * cs + v[1] * sn,
                        u[2] * cs + v[2] * sn,
                    ];
                    let p = [
                        center[0] + n[0] * radius,
                        center[1] + n[1] * radius,
                        center[2] + n[2] * radius,
                    ];
                    self.push(p, n, [s as f64 / segs as f64, along], w.clone())
                })
                .collect();
            rings.push(ring);
        }
        for pair in rings.windows(2) {
            let (lo, hi) = (&pair[0], &pair[1]);
            for s in 0..segs {
                let s2 = (s + 1) % segs;
                self.tri(lo[s], hi[s], hi[s2], 0);
                self.tri(lo[s], hi[s2], lo[s2], 0);
            }
        }
        // Caps.
        let first = rings.first().unwrap().clone();
        let last = rings.last().unwrap().clone();
        let c0 = self.push(
            pts[0],
            [-axis[0], -axis[1], -axis[2]],
            [0.5, 0.0],
            vec![(chain[0], 1.0)],
        );
        let c1 = self.push(
            pts[pts.len() - 1],
            axis,
            [0.5, 1.0],
            vec![(chain[chain.len() - 1], 1.0)],
        );
        for s in 0..segs {
            let s2 = (s + 1) % segs;
            self.tri(c0, first[s2], first[s], 0);
            self.tri(c1, last[s], last[s2], 0);
        }
    }

    /// A UV sphere fully weighted to one bone, on material slot `material`.
    fn sphere(
        &mut self,
        bone: i64,
        center: [f64; 3],
        radius: f64,
        lat: usize,
        lon: usize,
        material: i32,
    ) {
        let mut rows: Vec<Vec<u32>> = Vec::new();
        for i in 0..=lat {
            let theta = i as f64 / lat as f64 * std::f64::consts::PI;
            let (st, ct) = theta.sin_cos();
            let row: Vec<u32> = (0..lon)
                .map(|j| {
                    let phi = j as f64 / lon as f64 * std::f64::consts::TAU;
                    let (sp, cp) = phi.sin_cos();
                    let n = [st * cp, ct, st * sp];
                    let p = [
                        center[0] + n[0] * radius,
                        center[1] + n[1] * radius,
                        center[2] + n[2] * radius,
                    ];
                    self.push(
                        p,
                        n,
                        [j as f64 / lon as f64, i as f64 / lat as f64],
                        vec![(bone, 1.0)],
                    )
                })
                .collect();
            rows.push(row);
        }
        for pair in rows.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            for j in 0..lon {
                let j2 = (j + 1) % lon;
                self.tri(a[j], b[j2], b[j], material);
                self.tri(a[j], a[j2], b[j2], material);
            }
        }
    }
}

/// Bytes of a **skinned, T-posed, ~1.6 m humanoid** with real geometry — the "try a sample" demo
/// avatar: the same bone ids/names as [`humanoid_skeleton`] laid out with `Lcl Translation`s (cm,
/// `UnitScaleFactor` 100, Y-up), one skinned mesh of low-poly tubes per limb + a head sphere
/// (a few thousand triangles, per-control-point normals + UVs), one skin `Cluster` per bone that
/// drives geometry (indexes/weights/`TransformLink`), two material slots (`Body` with a diffuse
/// colour, `Head` for the sphere, via a per-polygon `LayerElementMaterial`), and a `BlendShape`
/// deformer with channels `vrc.v_aa`, `vrc.v_oh`, `Blink`, `Smile`. Deterministic, like every
/// fixture here; `humanoid_skeleton` is untouched so its golden snapshots stand.
pub fn humanoid_skinned() -> Vec<u8> {
    // --- geometry ---
    let mut mesh = MeshBuild::default();
    let segs = 16;
    mesh.chain_tube(&[100, 101, 102, 103], 14.0, 4, segs); // torso
    mesh.chain_tube(&[103, 104], 5.0, 2, segs); // neck
    mesh.sphere(104, [0.0, 158.0, 0.0], 11.0, 12, 16, 1); // head (slot 1)
    for arm in [[110, 111, 112, 113, 114], [120, 121, 122, 123, 124]] {
        mesh.chain_tube(&arm, 5.0, 3, segs);
    }
    for leg in [[130, 131, 132], [140, 141, 142]] {
        mesh.chain_tube(&leg, 7.0, 4, segs);
    }
    for foot in [[132, 133], [142, 143]] {
        mesh.chain_tube(&foot, 4.0, 2, segs);
    }

    let vertices: Vec<f64> = mesh.positions.iter().flatten().copied().collect();
    let pvi: Vec<i32> = mesh
        .polygons
        .iter()
        .flat_map(|t| [t[0] as i32, t[1] as i32, -(t[2] as i32) - 1])
        .collect();
    let normals: Vec<f64> = mesh.normals.iter().flatten().copied().collect();
    let uvs: Vec<f64> = mesh.uvs.iter().flatten().copied().collect();

    // Cluster data per bone: (control-point indexes, weights).
    let mut clusters: Vec<(i64, Vec<i32>, Vec<f64>)> = Vec::new();
    for b in SKINNED_BONES {
        let mut idx = Vec::new();
        let mut wts = Vec::new();
        for (cp, w) in mesh.weights.iter().enumerate() {
            if let Some(&(_, wt)) = w.iter().find(|(id, _)| *id == b.0) {
                idx.push(cp as i32);
                wts.push(wt);
            }
        }
        if !idx.is_empty() {
            clusters.push((b.0, idx, wts));
        }
    }

    // --- tree ---
    use fbxcel::low::v7400::AttributeValue as A;
    let mut tree = Tree::default();
    let root = tree.root().node_id();
    let node = |tree: &mut Tree, parent, name: &str, attrs: Vec<A>| {
        let n = tree.append_new(parent, name);
        for a in attrs {
            tree.append_attribute(n, a);
        }
        n
    };
    let s = |v: &str| A::from(v.to_string());
    let obj_name = |name: &str, class: &str| A::from(format!("{name}\u{0}\u{1}{class}"));

    let gs = node(&mut tree, root, "GlobalSettings", vec![]);
    let gp = node(&mut tree, gs, "Properties70", vec![]);
    node(
        &mut tree,
        gp,
        "P",
        vec![
            s("UnitScaleFactor"),
            s("double"),
            s("Number"),
            s(""),
            100.0f64.into(),
        ],
    );
    node(
        &mut tree,
        gp,
        "P",
        vec![s("UpAxis"), s("int"), s("Integer"), s(""), 1i32.into()],
    );
    node(
        &mut tree,
        gp,
        "P",
        vec![s("FrontAxis"), s("int"), s("Integer"), s(""), 2i32.into()],
    );

    let objects = node(&mut tree, root, "Objects", vec![]);
    // Mesh model + geometry.
    node(
        &mut tree,
        objects,
        "Model",
        vec![10i64.into(), obj_name("Body", "Model"), s("Mesh")],
    );
    let geom = node(
        &mut tree,
        objects,
        "Geometry",
        vec![20i64.into(), obj_name("Body", "Geometry"), s("Mesh")],
    );
    node(&mut tree, geom, "Vertices", vec![vertices.into()]);
    node(&mut tree, geom, "PolygonVertexIndex", vec![pvi.into()]);
    let ln = node(&mut tree, geom, "LayerElementNormal", vec![0i32.into()]);
    node(
        &mut tree,
        ln,
        "MappingInformationType",
        vec![s("ByControlPoint")],
    );
    node(&mut tree, ln, "ReferenceInformationType", vec![s("Direct")]);
    node(&mut tree, ln, "Normals", vec![normals.into()]);
    let lu = node(&mut tree, geom, "LayerElementUV", vec![0i32.into()]);
    node(
        &mut tree,
        lu,
        "MappingInformationType",
        vec![s("ByControlPoint")],
    );
    node(&mut tree, lu, "ReferenceInformationType", vec![s("Direct")]);
    node(&mut tree, lu, "UV", vec![uvs.into()]);
    let lm = node(&mut tree, geom, "LayerElementMaterial", vec![0i32.into()]);
    node(
        &mut tree,
        lm,
        "MappingInformationType",
        vec![s("ByPolygon")],
    );
    node(
        &mut tree,
        lm,
        "ReferenceInformationType",
        vec![s("IndexToDirect")],
    );
    node(
        &mut tree,
        lm,
        "Materials",
        vec![mesh.polygon_material.clone().into()],
    );
    // Materials.
    for (id, name, rgb) in [
        (60i64, "Body", [0.55f64, 0.62, 0.80]),
        (61, "Head", [0.92, 0.78, 0.68]),
    ] {
        let m = node(
            &mut tree,
            objects,
            "Material",
            vec![id.into(), obj_name(name, "Material"), s("")],
        );
        let p = node(&mut tree, m, "Properties70", vec![]);
        node(
            &mut tree,
            p,
            "P",
            vec![
                s("DiffuseColor"),
                s("Color"),
                s(""),
                s("A"),
                rgb[0].into(),
                rgb[1].into(),
                rgb[2].into(),
            ],
        );
    }
    // Bones with local translations.
    for &(id, name, parent, world) in SKINNED_BONES {
        let local = parent.map_or(world, |p| sub(world, bone_pos(p)));
        let m = node(
            &mut tree,
            objects,
            "Model",
            vec![id.into(), obj_name(name, "Model"), s("LimbNode")],
        );
        let p = node(&mut tree, m, "Properties70", vec![]);
        node(
            &mut tree,
            p,
            "P",
            vec![
                s("Lcl Translation"),
                s("Lcl Translation"),
                s(""),
                s("A"),
                local[0].into(),
                local[1].into(),
                local[2].into(),
            ],
        );
    }
    // Skin + clusters.
    node(
        &mut tree,
        objects,
        "Deformer",
        vec![40i64.into(), obj_name("Skin", "Deformer"), s("Skin")],
    );
    let identity: Vec<f64> = vec![
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    for (i, (bone, idx, wts)) in clusters.iter().enumerate() {
        let cid = 500 + i as i64;
        let bname = SKINNED_BONES.iter().find(|b| b.0 == *bone).unwrap().1;
        let c = node(
            &mut tree,
            objects,
            "SubDeformer",
            vec![cid.into(), obj_name(bname, "SubDeformer"), s("Cluster")],
        );
        node(&mut tree, c, "Indexes", vec![idx.clone().into()]);
        node(&mut tree, c, "Weights", vec![wts.clone().into()]);
        node(&mut tree, c, "Transform", vec![identity.clone().into()]);
        let w = bone_pos(*bone);
        let mut link = identity.clone();
        link[12] = w[0];
        link[13] = w[1];
        link[14] = w[2];
        node(&mut tree, c, "TransformLink", vec![link.into()]);
    }
    // Blendshapes.
    node(
        &mut tree,
        objects,
        "Deformer",
        vec![
            90i64.into(),
            obj_name("Shapes", "Deformer"),
            s("BlendShape"),
        ],
    );
    let channels = ["vrc.v_aa", "vrc.v_oh", "Blink", "Smile"];
    for (i, ch) in channels.iter().enumerate() {
        node(
            &mut tree,
            objects,
            "Deformer",
            vec![
                (91 + i as i64).into(),
                obj_name(ch, "Deformer"),
                s("BlendShapeChannel"),
            ],
        );
    }

    // --- connections (hierarchy before cluster links: `parent_of` takes a child's first OO) ---
    let conns = node(&mut tree, root, "Connections", vec![]);
    let oo = |tree: &mut Tree, child: i64, parent: i64| {
        node(tree, conns, "C", vec![s("OO"), child.into(), parent.into()]);
    };
    oo(&mut tree, 20, 10);
    oo(&mut tree, 60, 10);
    oo(&mut tree, 61, 10);
    for &(id, _, parent, _) in SKINNED_BONES {
        if let Some(p) = parent {
            oo(&mut tree, id, p);
        }
    }
    oo(&mut tree, 40, 20);
    for (i, (bone, _, _)) in clusters.iter().enumerate() {
        let cid = 500 + i as i64;
        oo(&mut tree, cid, 40);
        oo(&mut tree, *bone, cid);
    }
    oo(&mut tree, 90, 20);
    for i in 0..channels.len() as i64 {
        oo(&mut tree, 91 + i, 90);
    }
    to_bytes(&tree)
}
