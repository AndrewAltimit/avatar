//! Emit a non-standard (Mixamo-named) humanoid skeleton as a binary FBX, for the Unity
//! acceptance workflow. We never commit an FBX (`.gitignore` blocks `*.fbx`), so CI generates
//! the fixture, runs `avatar armature fix` over it, and feeds the *repaired* file to Unity.
//!
//! ```sh
//! cargo run -p avatar-fbx --example emit_broken_rig -- /tmp/broken.fbx
//! ```
//!
//! The rig carries the full required humanoid bone set under `mixamorig:` names, so after
//! `armature fix` renames them to canonical Unity names the result is humanoid-ready. It is a
//! skeleton only (no mesh): humanoid avatar configuration in Unity keys on the transform
//! hierarchy, so this is the minimal fixture that exercises the rename + writer round-trip. If a
//! real Unity run rejects a mesh-less skeleton, the follow-up is to add a one-triangle skinned
//! mesh here — see docs/reference/armature-repair.md.

use std::io::Cursor;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

fn broken_rig() -> Tree {
    tree_v7400! {
        Objects: {
            Model: [1i64, "mixamorig:Hips\u{0}\u{1}Model", "LimbNode"] {},
            Model: [2i64, "mixamorig:Spine\u{0}\u{1}Model", "LimbNode"] {},
            Model: [3i64, "mixamorig:Head\u{0}\u{1}Model", "LimbNode"] {},
            Model: [4i64, "mixamorig:LeftArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [5i64, "mixamorig:LeftForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [6i64, "mixamorig:LeftHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [7i64, "mixamorig:RightArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [8i64, "mixamorig:RightForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [9i64, "mixamorig:RightHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [10i64, "mixamorig:LeftUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [11i64, "mixamorig:LeftLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [12i64, "mixamorig:LeftFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [13i64, "mixamorig:RightUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [14i64, "mixamorig:RightLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [15i64, "mixamorig:RightFoot\u{0}\u{1}Model", "LimbNode"] {},
        },
        Connections: {
            C: ["OO", 2i64, 1i64] {},
            C: ["OO", 3i64, 2i64] {},
            C: ["OO", 4i64, 2i64] {},
            C: ["OO", 5i64, 4i64] {},
            C: ["OO", 6i64, 5i64] {},
            C: ["OO", 7i64, 2i64] {},
            C: ["OO", 8i64, 7i64] {},
            C: ["OO", 9i64, 8i64] {},
            C: ["OO", 10i64, 1i64] {},
            C: ["OO", 11i64, 10i64] {},
            C: ["OO", 12i64, 11i64] {},
            C: ["OO", 13i64, 1i64] {},
            C: ["OO", 14i64, 13i64] {},
            C: ["OO", 15i64, 14i64] {},
        },
    }
}

fn main() {
    let out = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: emit_broken_rig <output.fbx>");
            std::process::exit(2);
        }
    };

    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).expect("create writer");
    w.write_tree(&broken_rig()).expect("write tree");
    let bytes = w
        .finalize_and_flush(&FbxFooter::default())
        .expect("finalize")
        .into_inner();

    std::fs::write(&out, bytes).expect("write fbx file");
    eprintln!("wrote {out}");
}
