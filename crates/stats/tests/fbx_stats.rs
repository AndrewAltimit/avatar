//! Geometry-stats coverage over the real FBX read path. The primary fixture is synthesized as a
//! binary FBX in memory (no committed asset, no env var needed); a second test runs the analyzer
//! over a real model when `AVATAR_SAMPLE_FBX` points at one, and self-skips otherwise (repo
//! convention — CI without fixtures stays green).

use std::io::Cursor;

use avatar_stats::{Platform, Rank};
use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

fn to_fbx_bytes(tree: &Tree) -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(tree).unwrap();
    w.finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner()
}

/// One quad (→ 2 triangles) skinned to two bones, with two materials attached to the mesh Model.
fn skinned_quad_with_materials() -> Tree {
    let translate = |x: f64, y: f64, z: f64| {
        vec![
            1.0f64, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            x, y, z, 1.0,
        ]
    };
    let identity = translate(0.0, 0.0, 0.0);
    tree_v7400! {
        Objects: {
            Model: [10i64, "Mesh\u{0}\u{1}Model", "Mesh"] {},
            Geometry: [20i64, "Mesh\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]] {},
                PolygonVertexIndex: [vec![0i32, 1, 2, -4]] {},
            },
            Model: [30i64, "Bone0\u{0}\u{1}Model", "LimbNode"] {},
            Model: [31i64, "Bone1\u{0}\u{1}Model", "LimbNode"] {},
            Material: [60i64, "Body\u{0}\u{1}Material", ""] {},
            Material: [61i64, "Face\u{0}\u{1}Material", ""] {},
            Deformer: [40i64, "Skin\u{0}\u{1}Deformer", "Skin"] {},
            SubDeformer: [50i64, "Cluster0\u{0}\u{1}SubDeformer", "Cluster"] {
                Indexes: [vec![0i32, 1]] {},
                Weights: [vec![1.0f64, 1.0]] {},
                Transform: [identity.clone()] {},
                TransformLink: [translate(0.0, 0.0, 0.0)] {},
            },
            SubDeformer: [51i64, "Cluster1\u{0}\u{1}SubDeformer", "Cluster"] {
                Indexes: [vec![2i32, 3]] {},
                Weights: [vec![1.0f64, 1.0]] {},
                Transform: [identity] {},
                TransformLink: [translate(0.0, 1.0, 0.0)] {},
            },
        },
        Connections: {
            C: ["OO", 20i64, 10i64] {}, // Geometry -> mesh Model
            C: ["OO", 40i64, 20i64] {}, // Skin -> Geometry
            C: ["OO", 50i64, 40i64] {}, // Cluster0 -> Skin
            C: ["OO", 51i64, 40i64] {}, // Cluster1 -> Skin
            C: ["OO", 30i64, 50i64] {}, // Bone0 -> Cluster0
            C: ["OO", 31i64, 51i64] {}, // Bone1 -> Cluster1
            C: ["OO", 60i64, 10i64] {}, // Body material -> mesh Model (slot 1)
            C: ["OO", 61i64, 10i64] {}, // Face material -> mesh Model (slot 2)
        },
    }
}

#[test]
fn measures_geometry_stats_from_fbx() {
    let bytes = to_fbx_bytes(&skinned_quad_with_materials());
    let report = avatar_stats::analyze_fbx_bytes(&bytes, "quad.fbx").unwrap();

    let value = |name: &str| report.stats.iter().find(|s| s.name == name).unwrap().value;
    assert_eq!(value("Triangles"), 2, "a quad fan-triangulates into 2 tris");
    assert_eq!(value("Skinned Meshes"), 1);
    assert_eq!(value("Basic Meshes"), 0);
    assert_eq!(
        value("Material Slots"),
        2,
        "two materials attached to the mesh"
    );
    assert_eq!(
        value("Bones"),
        2,
        "distinct bones driving the skin clusters"
    );

    // A handful of triangles and two slots is comfortably Excellent on PC. On Android the stricter
    // material-slot limits (1/1/2/4) make two slots Medium, which becomes the overall.
    assert_eq!(report.overall(Platform::Pc), Rank::Excellent);
    assert_eq!(report.overall(Platform::Android), Rank::Medium);
    assert_eq!(report.kind, "fbx");
    assert!(report.not_evaluated.iter().any(|m| m == "Texture Memory"));
}

#[test]
fn analyzes_a_real_fbx_when_provided() {
    let Ok(path) = std::env::var("AVATAR_SAMPLE_FBX") else {
        eprintln!("skipping: set AVATAR_SAMPLE_FBX to a binary FBX to run this test");
        return;
    };
    let report = avatar_stats::analyze_fbx(std::path::Path::new(&path)).unwrap();
    // A real character model always has geometry and a skeleton driving it.
    let value = |name: &str| report.stats.iter().find(|s| s.name == name).unwrap().value;
    assert!(value("Triangles") > 0, "a real model should have triangles");
    assert!(value("Bones") > 0, "a real rig should have bones");
}
