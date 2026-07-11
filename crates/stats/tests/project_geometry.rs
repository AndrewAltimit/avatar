//! End-to-end project analysis: a temp Unity project whose avatar prefab references a real FBX
//! mesh by guid. Exercises the renderer→FBX triangle resolution and the `m_Bones` bone count
//! against actual files on disk (no committed fixture, no env var).

use std::io::Cursor;
use std::path::PathBuf;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// A mesh-only FBX: one quad → 2 triangles.
fn quad_fbx_bytes() -> Vec<u8> {
    let tree: Tree = tree_v7400! {
        Objects: {
            Model: [10i64, "Mesh\u{0}\u{1}Model", "Mesh"] {},
            Geometry: [20i64, "Mesh\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0]] {},
                PolygonVertexIndex: [vec![0i32, 1, 2, -4]] {},
            },
        },
        Connections: {
            C: ["OO", 20i64, 10i64] {},
        },
    };
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(&tree).unwrap();
    w.finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner()
}

/// A prefab declaring an avatar (VRC Avatar Descriptor) with one skinned mesh renderer that points
/// at the FBX mesh by `guid`, skinned to three distinct bones.
fn avatar_prefab(mesh_guid: &str) -> String {
    format!(
        "\
--- !u!114 &1
MonoBehaviour:
  m_Name: TestAvatar
  ViewPosition: {{x: 0, y: 1.2, z: 0.1}}
  baseAnimationLayers:
  - type: 4
    isDefault: 0
--- !u!137 &2
SkinnedMeshRenderer:
  m_Materials:
  - {{fileID: 2100000, guid: cccccccccccccccccccccccccccccccc, type: 2}}
  m_Mesh: {{fileID: 4300000, guid: {mesh_guid}, type: 3}}
  m_Bones:
  - {{fileID: 100}}
  - {{fileID: 101}}
  - {{fileID: 102}}
"
    )
}

/// A minimal PNG (signature + IHDR) of the given size and colour type (6 = RGBA, 2 = RGB).
fn png(width: u32, height: u32, colour_type: u8) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0, 0, 0, 13]);
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&width.to_be_bytes());
    v.extend_from_slice(&height.to_be_bytes());
    v.extend_from_slice(&[8, colour_type, 0, 0, 0]);
    v
}

/// A material referencing one texture by `tex_guid` on `_MainTex`.
fn material(tex_guid: &str) -> String {
    format!(
        "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!21 &2100000
Material:
  m_Name: Body
  m_SavedProperties:
    m_TexEnvs:
    - _MainTex:
        m_Texture: {{fileID: 2800000, guid: {tex_guid}, type: 3}}
        m_Scale: {{x: 1, y: 1}}
        m_Offset: {{x: 0, y: 0}}
"
    )
}

/// A texture `.meta` with the given guid: 2048 cap, compressed, mipmaps on.
fn texture_meta(guid: &str) -> String {
    format!(
        "\
fileFormatVersion: 2
guid: {guid}
TextureImporter:
  mipmaps:
    enableMipMap: 1
  maxTextureSize: 2048
  platformSettings:
  - buildTarget: DefaultTexturePlatform
    maxTextureSize: 2048
    textureCompression: 1
"
    )
}

/// Unique temp project dir for this test (cleaned up at the end).
fn temp_project() -> PathBuf {
    std::env::temp_dir().join(format!("avatar-stats-proj-{}", std::process::id()))
}

#[test]
fn resolves_triangles_and_bones_from_a_project() {
    let mesh_guid = "abcdef0123456789abcdef0123456789";
    // Own subdirectory: removing the shared `temp_project()` root would race the sibling tests'
    // subdirectories under parallel test execution.
    let root = temp_project().join("basic");
    let avatar_dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&avatar_dir).unwrap();

    // The source FBX + its `.meta` carrying the guid the prefab references.
    std::fs::write(avatar_dir.join("Body.fbx"), quad_fbx_bytes()).unwrap();
    std::fs::write(
        avatar_dir.join("Body.fbx.meta"),
        format!("fileFormatVersion: 2\nguid: {mesh_guid}\n"),
    )
    .unwrap();

    // The material the renderer uses (guid `cccc…`, per `avatar_prefab`) → one RGBA texture.
    let tex_guid = "dddddddddddddddddddddddddddddddd";
    std::fs::write(avatar_dir.join("Body.mat"), material(tex_guid)).unwrap();
    std::fs::write(
        avatar_dir.join("Body.mat.meta"),
        "fileFormatVersion: 2\nguid: cccccccccccccccccccccccccccccccc\n",
    )
    .unwrap();

    // The texture (256×256 RGBA) + its import settings.
    std::fs::write(avatar_dir.join("Body.png"), png(256, 256, 6)).unwrap();
    std::fs::write(avatar_dir.join("Body.png.meta"), texture_meta(tex_guid)).unwrap();

    // The avatar prefab.
    std::fs::write(avatar_dir.join("Avatar.prefab"), avatar_prefab(mesh_guid)).unwrap();

    let reports = avatar_stats::analyze_project(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);

    assert_eq!(reports.len(), 1, "one avatar prefab -> one report");
    let r = &reports[0];
    let value = |name: &str| r.stats.iter().find(|s| s.name == name).unwrap().value;

    assert_eq!(
        value("Triangles"),
        2,
        "resolved through the FBX mesh reference"
    );
    assert_eq!(value("Bones"), 3, "distinct m_Bones transforms");
    assert_eq!(value("Skinned Meshes"), 1);
    assert_eq!(value("Material Slots"), 1);

    // 256×256 RGBA, compressed + mipmaps (×4/3), resolved material → texture → import meta.
    // PC = DXT5/BC7 (1 bpp); Android = ASTC 6x6 (16/36 bpp). The stat carries both.
    let tex = r.stats.iter().find(|s| s.name == "Texture Memory").unwrap();
    let px = 256.0_f64 * 256.0 * (4.0 / 3.0);
    assert_eq!(tex.value, (px * 1.0).round() as u64, "PC texture memory");
    assert_eq!(
        tex.android_value,
        (px * (16.0 / 36.0)).round() as u64,
        "Android texture memory (ASTC 6x6)"
    );
    assert!(
        !r.not_evaluated.iter().any(|m| m.starts_with("Triangles")),
        "the only mesh resolved cleanly, so no unresolved-triangle note: {:?}",
        r.not_evaluated
    );

    cleanup.unwrap();
}

/// A prefab declaring an avatar with one particle system (10/s × 3s = 30 live particles) and a
/// two-deep constraint chain (C1 on GO 1000 driven by C2 on GO 1001).
const PARTICLE_CONSTRAINT_PREFAB: &str = "\
--- !u!114 &1
MonoBehaviour:
  m_Name: ParticleAvatar
  ViewPosition: {x: 0, y: 1.2, z: 0.1}
  baseAnimationLayers:
  - type: 4
    isDefault: 0
--- !u!198 &2
ParticleSystem:
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 3
  EmissionModule:
    rateOverTime:
      scalar: 10
--- !u!4 &10
Transform:
  m_GameObject: {fileID: 1000}
--- !u!4 &11
Transform:
  m_GameObject: {fileID: 1001}
--- !u!324 &20
ParentConstraint:
  m_GameObject: {fileID: 1000}
  m_Sources:
  - sourceTransform: {fileID: 11}
    weight: 1
--- !u!324 &21
ParentConstraint:
  m_GameObject: {fileID: 1001}
  m_Sources: []
";

#[test]
fn resolves_particles_and_constraints_end_to_end() {
    let root = temp_project().join("particles");
    let avatar_dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&avatar_dir).unwrap();
    std::fs::write(avatar_dir.join("Avatar.prefab"), PARTICLE_CONSTRAINT_PREFAB).unwrap();

    let reports = avatar_stats::analyze_project(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);

    assert_eq!(reports.len(), 1);
    let r = &reports[0];
    let value = |name: &str| r.stats.iter().find(|s| s.name == name).map(|s| s.value);

    assert_eq!(value("Particle Systems"), Some(1));
    assert_eq!(
        value("Total Particles"),
        Some(30),
        "10/s × 3s, under the cap"
    );
    assert_eq!(value("Constraints"), Some(2));
    assert_eq!(value("Constraint Depth"), Some(2), "C1 <- C2");
    assert!(
        !r.not_evaluated
            .iter()
            .any(|m| m == "Total Particles" || m == "Constraints"),
        "particles and constraints are now measured, not deferred: {:?}",
        r.not_evaluated
    );

    cleanup.unwrap();
}

#[test]
fn flags_unresolved_meshes() {
    let root = temp_project().join("unresolved");
    let avatar_dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&avatar_dir).unwrap();

    // The prefab references a mesh guid that has no `.meta` in the project -> unresolved.
    std::fs::write(
        avatar_dir.join("Avatar.prefab"),
        avatar_prefab("deadbeefdeadbeefdeadbeefdeadbeef"),
    )
    .unwrap();

    let reports = avatar_stats::analyze_project(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);

    assert_eq!(reports.len(), 1);
    let r = &reports[0];
    assert_eq!(
        r.stats
            .iter()
            .find(|s| s.name == "Triangles")
            .unwrap()
            .value,
        0,
        "unresolved mesh contributes no triangles"
    );
    assert!(
        r.not_evaluated.iter().any(|m| m.starts_with("Triangles")),
        "an unresolved mesh should be flagged: {:?}",
        r.not_evaluated
    );

    cleanup.unwrap();
}
