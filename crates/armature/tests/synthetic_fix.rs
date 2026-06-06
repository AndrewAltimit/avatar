//! Non-gated, end-to-end exercise of the real FBX write path.
//!
//! Unlike `sample_fix.rs` (which needs an `AVATAR_SAMPLE_FBX` on disk), this synthesizes a broken
//! Mixamo-style rig as a binary FBX *in memory*, then runs the full
//! `from_bytes -> plan_repairs -> apply_plan -> to_bytes -> reload` pipeline. It proves the writer
//! on a real serialized document in CI, and it locks in the repair boundary: renames are applied
//! and persist; the mis-parented bone is detected but **not** silently reparented (a bare
//! connection edit would move its rest/bind pose — see `avatar_armature::repair`).

use std::io::Cursor;

use avatar_armature::{RepairEdit, apply_plan, plan_repairs};
use avatar_fbx::FbxDocument;
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

/// A full set of Mixamo-named required humanoid bones (so every slot maps and every name differs
/// from canonical), with `LeftHand` (7) mis-parented onto `Hips` (1) instead of `LeftForeArm` (6),
/// plus non-standard units/axis.
fn broken_rig_fbx() -> Vec<u8> {
    let tree = tree_v7400! {
        GlobalSettings: {
            Properties70: {
                P: ["UnitScaleFactor", "double", "Number", "", 1.0f64] {},
                P: ["UpAxis", "int", "Integer", "", 2i32] {},
            },
        },
        Objects: {
            Model: [1i64, "mixamorig:Hips\u{0}\u{1}Model", "LimbNode"] {},
            Model: [2i64, "mixamorig:Spine\u{0}\u{1}Model", "LimbNode"] {},
            Model: [3i64, "mixamorig:Neck\u{0}\u{1}Model", "LimbNode"] {},
            Model: [4i64, "mixamorig:Head\u{0}\u{1}Model", "LimbNode"] {},
            Model: [5i64, "mixamorig:LeftArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [6i64, "mixamorig:LeftForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [7i64, "mixamorig:LeftHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [8i64, "mixamorig:RightArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [9i64, "mixamorig:RightForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [10i64, "mixamorig:RightHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [11i64, "mixamorig:LeftUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [12i64, "mixamorig:LeftLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [13i64, "mixamorig:LeftFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [14i64, "mixamorig:RightUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [15i64, "mixamorig:RightLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [16i64, "mixamorig:RightFoot\u{0}\u{1}Model", "LimbNode"] {},
        },
        Connections: {
            C: ["OO", 2i64, 1i64] {},
            C: ["OO", 3i64, 2i64] {},
            C: ["OO", 4i64, 3i64] {},
            C: ["OO", 5i64, 2i64] {},
            C: ["OO", 6i64, 5i64] {},
            C: ["OO", 7i64, 1i64] {}, // mis-parented: LeftHand under Hips, not LeftForeArm (6).
            C: ["OO", 8i64, 2i64] {},
            C: ["OO", 9i64, 8i64] {},
            C: ["OO", 10i64, 9i64] {},
            C: ["OO", 11i64, 1i64] {},
            C: ["OO", 12i64, 11i64] {},
            C: ["OO", 13i64, 12i64] {},
            C: ["OO", 14i64, 1i64] {},
            C: ["OO", 15i64, 14i64] {},
            C: ["OO", 16i64, 15i64] {},
        },
    };
    to_fbx_bytes(&tree)
}

fn count<F: Fn(&RepairEdit) -> bool>(plan: &avatar_armature::RepairPlan, pred: F) -> usize {
    plan.edits.iter().filter(|e| pred(e)).count()
}

#[test]
fn synthetic_broken_rig_round_trips_through_writer() {
    let mut doc = FbxDocument::from_bytes(&broken_rig_fbx()).expect("parse synthetic FBX");
    let plan = plan_repairs(&doc.scene());

    // All 16 Mixamo bones differ from canonical, so all are renamed.
    assert_eq!(
        count(&plan, |e| matches!(e, RepairEdit::RenameBone { .. })),
        16
    );

    // The mis-parented LeftHand is detected, but reparent is report-only.
    let reparents: Vec<_> = plan
        .edits
        .iter()
        .filter(|e| matches!(e, RepairEdit::Reparent { .. }))
        .collect();
    assert_eq!(reparents.len(), 1);
    assert!(
        !reparents[0].is_native(),
        "reparent must be flagged, not applied"
    );

    // Only the renames apply.
    let applied = apply_plan(&mut doc, &plan).expect("apply repair plan");
    assert_eq!(applied, 16);
    assert_eq!(applied, plan.native().count());

    // Round-trip through the binary writer and re-parse from memory.
    let bytes = doc.to_bytes().expect("serialize repaired FBX");
    let scene = FbxDocument::from_bytes(&bytes)
        .expect("re-parse repaired FBX")
        .scene();

    // Renames persisted, addressed by stable object id.
    assert_eq!(scene.object(5).unwrap().name, "LeftUpperArm");
    assert_eq!(scene.object(6).unwrap().name, "LeftLowerArm");
    assert_eq!(scene.object(7).unwrap().name, "LeftHand");

    // The mis-parent was NOT silently fixed — LeftHand still hangs off Hips (1).
    assert_eq!(scene.parent_of(7), Some(1));

    // Re-planning the repaired scene: no more renames (idempotent), but the reparent is still
    // flagged for the user.
    let plan2 = plan_repairs(&scene);
    assert_eq!(
        count(&plan2, |e| matches!(e, RepairEdit::RenameBone { .. })),
        0,
        "repaired FBX should need no more renames"
    );
    assert_eq!(
        count(&plan2, |e| matches!(e, RepairEdit::Reparent { .. })),
        1,
        "the mis-parent remains flagged"
    );
}
