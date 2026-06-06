//! End-to-end test over a real FBX file.
//!
//! Set `AVATAR_SAMPLE_FBX` to the absolute path of a binary FBX to exercise the full
//! load -> skeleton -> humanoid-analysis pipeline. If the var is unset the test prints a
//! one-line skip notice and returns OK, so CI (and other machines) without a sample stay green.
//! Never commit user FBX files — see `.gitignore`.

use std::path::PathBuf;

use avatar_fbx::FbxScene;

#[test]
fn analyzes_sample_fbx() {
    let Ok(path) = std::env::var("AVATAR_SAMPLE_FBX") else {
        eprintln!("skip: AVATAR_SAMPLE_FBX not set");
        return;
    };
    let path = PathBuf::from(path);

    let scene = FbxScene::load(&path).expect("load sample FBX");
    assert!(scene.version >= 7000, "expected an FBX 7.x file");

    let report = avatar_armature::analyze(&scene);

    // The report must be internally consistent regardless of whether the sample is a
    // full humanoid avatar or just a prop.
    assert_eq!(
        report.is_humanoid_ready(),
        report.missing_required.is_empty()
    );
    assert!(report.total_models >= report.bone_like_count);

    eprintln!(
        "sample {}: {} models, {} mapped humanoid bones, {} missing required",
        path.display(),
        report.total_models,
        report.mapped.len(),
        report.missing_required.len()
    );
}
