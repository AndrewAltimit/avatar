//! Golden snapshot of the armature analysis over the in-code synthetic humanoid skeleton.
//!
//! Pins the whole `ArmatureReport` — the humanoid bone→source mapping, the required/recommended
//! missing lists, the finger/leaf exclusion counts, the armature vs mesh roots — so any change to
//! the mapping logic surfaces as a diff. The fixture is a complete Mixamo-style skeleton, so it is
//! humanoid-ready (`missing_required` empty). Regenerate: `UPDATE_GOLDEN=1 cargo test -p avatar-armature`.

use avatar_fbx::FbxDocument;
use avatar_testkit::golden;

#[test]
fn golden_humanoid_skeleton_armature() {
    let bytes = avatar_testkit::fbx::humanoid_skeleton();
    let doc = FbxDocument::from_bytes(&bytes).expect("parse synthetic FBX");
    let report = avatar_armature::analyze(&doc.scene());

    assert!(
        report.is_humanoid_ready(),
        "the synthetic skeleton must be humanoid-ready; missing: {:?}",
        report.missing_required
    );

    let value = serde_json::to_value(&report).expect("report serializes to JSON");
    golden::assert_json("tests/golden/humanoid_skeleton.armature.json", &value);
}
