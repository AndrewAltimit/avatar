//! End-to-end repair + FBX write-back test over a real FBX file.
//!
//! Set `AVATAR_SAMPLE_FBX` to the absolute path of a binary FBX to exercise the full
//! load -> plan_repairs -> apply -> write -> reload pipeline, proving the native FBX writer on a
//! real asset. If the var is unset the test prints a one-line skip notice and returns OK, so CI
//! (and other machines) without a sample stay green. Never commit user FBX files — see `.gitignore`.

use std::path::PathBuf;

use avatar_armature::{RepairEdit, apply_plan, plan_repairs};
use avatar_fbx::FbxDocument;

#[test]
fn repairs_and_round_trips_sample_fbx() {
    let Ok(path) = std::env::var("AVATAR_SAMPLE_FBX") else {
        eprintln!("skip: AVATAR_SAMPLE_FBX not set");
        return;
    };
    let path = PathBuf::from(path);

    let mut doc = FbxDocument::load(&path).expect("load sample FBX");
    let plan = plan_repairs(&doc.scene());

    // Capture the rename targets we expect to see after a successful write.
    let renames: Vec<(i64, String)> = plan
        .edits
        .iter()
        .filter_map(|e| match e {
            RepairEdit::RenameBone { id, to, .. } => Some((*id, to.clone())),
            _ => None,
        })
        .collect();

    let applied = apply_plan(&mut doc, &plan).expect("apply repair plan");
    assert_eq!(
        applied,
        plan.native().count(),
        "all native edits should apply"
    );

    // Serialize and re-parse from memory — the writer must produce a parseable binary FBX.
    let bytes = doc.to_bytes().expect("serialize repaired FBX");
    let reloaded = FbxDocument::from_bytes(&bytes).expect("re-parse repaired FBX");
    let scene = reloaded.scene();

    // Every renamed bone now carries its canonical name, addressed by its stable object id.
    for (id, expected) in &renames {
        let obj = scene
            .object(*id)
            .unwrap_or_else(|| panic!("object id {id} missing after round-trip"));
        assert_eq!(&obj.name, expected, "rename did not persist for id {id}");
    }

    // Re-planning the repaired scene must find no remaining renames (idempotent).
    let plan2 = plan_repairs(&scene);
    let remaining_renames = plan2
        .edits
        .iter()
        .filter(|e| matches!(e, RepairEdit::RenameBone { .. }))
        .count();
    assert_eq!(
        remaining_renames, 0,
        "repaired FBX should need no more renames"
    );

    eprintln!(
        "sample {}: applied {applied} native edit(s), {} renames persisted, {} flagged",
        path.display(),
        renames.len(),
        plan.flagged().count(),
    );
}
