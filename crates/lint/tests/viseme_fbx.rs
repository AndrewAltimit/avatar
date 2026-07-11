//! End-to-end VRC039: a temp Unity project whose avatar descriptor points its viseme
//! SkinnedMeshRenderer at a real (synthetic, in-memory) FBX. Exercises the descriptor →
//! renderer → `m_Mesh` guid → source-FBX → morph-channel resolution against actual files on
//! disk — no committed FBX (corpus policy), same approach as the `avatar-stats` geometry tests.

use std::io::Cursor;
use std::path::PathBuf;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// A one-triangle mesh whose morph deformer carries exactly two channels:
/// `vrc.v_aa` and `Smile`.
fn morph_fbx_bytes() -> Vec<u8> {
    let tree: Tree = tree_v7400! {
        Objects: {
            Model: [10i64, "Body\u{0}\u{1}Model", "Mesh"] {},
            Geometry: [20i64, "Body\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0]] {},
                PolygonVertexIndex: [vec![0i32, 1, -3]] {},
            },
            Deformer: [40i64, "\u{0}\u{1}Deformer", "BlendShape"] {},
            Deformer: [50i64, "vrc.v_aa\u{0}\u{1}SubDeformer", "BlendShapeChannel"] {},
            Deformer: [51i64, "Smile\u{0}\u{1}SubDeformer", "BlendShapeChannel"] {},
        },
        Connections: {
            C: ["OO", 20i64, 10i64] {},
            C: ["OO", 40i64, 20i64] {},
            C: ["OO", 50i64, 40i64] {},
            C: ["OO", 51i64, 40i64] {},
        },
    };
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(&tree).unwrap();
    w.finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner()
}

/// A prefab whose descriptor uses viseme blend shapes on a renderer that points at the FBX by
/// guid. One viseme (`vrc.v_aa`) exists on the mesh; the other (`vrc.v_ch`) does not.
fn avatar_prefab(mesh_guid: &str) -> String {
    format!(
        "\
--- !u!114 &1
MonoBehaviour:
  m_Name: VisemeAvatar
  ViewPosition: {{x: 0, y: 1.2, z: 0.1}}
  lipSync: 3
  VisemeSkinnedMesh: {{fileID: 2}}
  VisemeBlendShapes:
  - vrc.v_aa
  - vrc.v_ch
  baseAnimationLayers:
  - type: 4
    isDefault: 1
--- !u!137 &2
SkinnedMeshRenderer:
  m_Mesh: {{fileID: 4300000, guid: {mesh_guid}, type: 3}}
  m_Bones: []
"
    )
}

fn temp_project(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("avatar-lint-viseme-{}-{name}", std::process::id()))
}

#[test]
fn flags_visemes_missing_from_the_source_fbx() {
    let mesh_guid = "abcdef0123456789abcdef0123456789";
    let root = temp_project("missing");
    let avatar_dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&avatar_dir).unwrap();

    std::fs::write(avatar_dir.join("Body.fbx"), morph_fbx_bytes()).unwrap();
    std::fs::write(
        avatar_dir.join("Body.fbx.meta"),
        format!("fileFormatVersion: 2\nguid: {mesh_guid}\n"),
    )
    .unwrap();
    std::fs::write(avatar_dir.join("Avatar.prefab"), avatar_prefab(mesh_guid)).unwrap();

    let report = avatar_lint::run(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);

    let vrc039: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC039")
        .collect();
    assert_eq!(
        vrc039.len(),
        1,
        "one VRC039 finding: {:?}",
        report.diagnostics
    );
    assert!(
        vrc039[0].message.contains("'vrc.v_ch'"),
        "names the missing shape: {}",
        vrc039[0].message
    );
    assert!(
        !vrc039[0].message.contains("'vrc.v_aa'"),
        "does not flag the shape that exists: {}",
        vrc039[0].message
    );

    cleanup.unwrap();
}

#[test]
fn silent_when_all_visemes_exist_or_mesh_unresolvable() {
    let mesh_guid = "abcdef0123456789abcdef0123456789";
    let root = temp_project("clean");
    let avatar_dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&avatar_dir).unwrap();

    std::fs::write(avatar_dir.join("Body.fbx"), morph_fbx_bytes()).unwrap();
    std::fs::write(
        avatar_dir.join("Body.fbx.meta"),
        format!("fileFormatVersion: 2\nguid: {mesh_guid}\n"),
    )
    .unwrap();
    // Only the viseme that exists on the mesh.
    let prefab = avatar_prefab(mesh_guid).replace("  - vrc.v_ch\n", "");
    std::fs::write(avatar_dir.join("Avatar.prefab"), &prefab).unwrap();

    let report = avatar_lint::run(&root).unwrap();

    // Second scenario in the same project dir: a descriptor whose mesh guid resolves to nothing —
    // the rule must stay quiet (missing assets are other rules' findings).
    let ghost = avatar_prefab("dddddddddddddddddddddddddddddddd");
    std::fs::write(avatar_dir.join("Ghost.prefab"), &ghost).unwrap();
    let report2 = avatar_lint::run(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);

    assert!(
        !report.diagnostics.iter().any(|d| d.code == "VRC039"),
        "clean project must not fire VRC039: {:?}",
        report.diagnostics
    );
    assert!(
        !report2.diagnostics.iter().any(|d| d.code == "VRC039"),
        "unresolvable mesh guid must not fire VRC039: {:?}",
        report2.diagnostics
    );

    cleanup.unwrap();
}
