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
