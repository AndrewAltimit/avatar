//! End-to-end VRC062: a temp Unity project whose descriptor registers `Blink` as an eyelid
//! blendshape (`eyelidType: 2` + `eyelidsBlendshapes`) while the FX controller's clip animates
//! `blendShape.Blink` — the exact "my blink expression does nothing in-game" conflict (VRChat's
//! eyelid driver overrides the animator). Same on-disk resolution harness as the VRC039 test:
//! descriptor → eyelid renderer → `m_Mesh` guid → synthetic in-memory FBX → morph channels.

use std::io::Cursor;
use std::path::PathBuf;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// A one-triangle mesh whose morph channels are, in import order: `vrc.v_aa` (0), `Blink` (1).
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
            Deformer: [51i64, "Blink\u{0}\u{1}SubDeformer", "BlendShapeChannel"] {},
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

/// Descriptor with blendshape eyelids: `Blink` (channel index 1) registered, FX layer pointing
/// at the controller by guid.
fn avatar_prefab(mesh_guid: &str, controller_guid: &str) -> String {
    format!(
        "\
--- !u!114 &1
MonoBehaviour:
  m_Name: EyelidAvatar
  ViewPosition: {{x: 0, y: 1.2, z: 0.1}}
  enableEyeLook: 0
  customEyeLookSettings:
    eyelidType: 2
    eyelidsSkinnedMesh: {{fileID: 2}}
    eyelidsBlendshapes: [1, -1, -1]
  baseAnimationLayers:
  - type: 4
    isDefault: 0
    animatorController: {{fileID: 9100000, guid: {controller_guid}, type: 2}}
--- !u!137 &2
SkinnedMeshRenderer:
  m_Mesh: {{fileID: 4300000, guid: {mesh_guid}, type: 3}}
  m_Bones: []
"
    )
}

/// A minimal controller: one state playing the clip (by guid).
fn fx_controller(clip_guid: &str) -> String {
    format!(
        "\
--- !u!91 &9100000
AnimatorController:
  m_Name: FX
  m_AnimatorParameters: []
  m_AnimatorLayers:
  - m_Name: Gestures
    m_StateMachine: {{fileID: 2}}
--- !u!1107 &2
AnimatorStateMachine:
  m_Name: Gestures
  m_ChildStates:
  - m_State: {{fileID: 3}}
  m_DefaultState: {{fileID: 3}}
--- !u!1102 &3
AnimatorState:
  m_Name: Fist
  m_WriteDefaultValues: 0
  m_Motion: {{fileID: 7400000, guid: {clip_guid}, type: 2}}
"
    )
}

/// A clip that animates `blendShape.Blink` to 100 on `Body`.
fn blink_clip() -> String {
    "\
--- !u!74 &7400000
AnimationClip:
  m_Name: Gesture_Fist
  m_FloatCurves:
  - curve:
      m_Curve:
      - time: 0
        value: 100
    attribute: blendShape.Blink
    path: Body
    classID: 137
"
    .to_string()
}

fn temp_project(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("avatar-lint-eyelid-{}-{name}", std::process::id()))
}

#[test]
fn flags_fx_animating_a_registered_eyelid_blendshape() {
    let mesh_guid = "eeeeef0123456789abcdef0123456789";
    let ctrl_guid = "cccccf0123456789abcdef0123456789";
    let clip_guid = "aaaaaf0123456789abcdef0123456789";
    let root = temp_project("conflict");
    let dir = root.join("Assets/Avatar");
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("Body.fbx"), morph_fbx_bytes()).unwrap();
    std::fs::write(
        dir.join("Body.fbx.meta"),
        format!("fileFormatVersion: 2\nguid: {mesh_guid}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("FX.controller"), fx_controller(clip_guid)).unwrap();
    std::fs::write(
        dir.join("FX.controller.meta"),
        format!("fileFormatVersion: 2\nguid: {ctrl_guid}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("Fist.anim"), blink_clip()).unwrap();
    std::fs::write(
        dir.join("Fist.anim.meta"),
        format!("fileFormatVersion: 2\nguid: {clip_guid}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("Avatar.prefab"),
        avatar_prefab(mesh_guid, ctrl_guid),
    )
    .unwrap();

    let report = avatar_lint::run(&root).unwrap();

    let vrc062: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC062")
        .collect();
    assert_eq!(
        vrc062.len(),
        1,
        "one VRC062 finding: {:?}",
        report.diagnostics
    );
    assert!(
        vrc062[0].message.contains("'Blink'") && vrc062[0].message.contains("Fist.anim"),
        "names the shape and the clip: {}",
        vrc062[0].message
    );

    // Unregister the eyelids (eyelidType None): the same project must go quiet.
    let fixed = avatar_prefab(mesh_guid, ctrl_guid).replace("eyelidType: 2", "eyelidType: 0");
    std::fs::write(dir.join("Avatar.prefab"), fixed).unwrap();
    let report2 = avatar_lint::run(&root).unwrap();
    let cleanup = std::fs::remove_dir_all(&root);
    assert!(
        report2.diagnostics.iter().all(|d| d.code != "VRC062"),
        "{:?}",
        report2.diagnostics
    );
    cleanup.unwrap();
}
