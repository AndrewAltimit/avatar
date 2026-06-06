//! End-to-end exit-code coverage, driven through the real `avatar` binary.
//!
//! Cargo exposes the built binary path to integration tests as `CARGO_BIN_EXE_avatar`, so these run
//! the actual CLI a user/CI would. Fixtures are synthesized as binary FBX in memory (no committed
//! asset, no `AVATAR_SAMPLE_FBX` needed) and written to a temp file for the duration of one test.
//!
//! This locks in the CI-gating contract:
//!   * `armature check` exits 0 on a humanoid-ready rig and **non-zero when a required bone is
//!     missing** — the failure path that gates a pipeline.
//!   * `armature fix` (dry run) exits 0.

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

const AVATAR: &str = env!("CARGO_BIN_EXE_avatar");

/// Serialize a tree to a binary FBX on a unique temp path. The path is unique per (pid, label) so
/// concurrent tests don't collide; each test removes its own file when done.
fn write_fbx(tree: &Tree, label: &str) -> PathBuf {
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(tree).unwrap();
    let bytes = w
        .finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner();
    let path = std::env::temp_dir().join(format!("avatar-cli-{}-{label}.fbx", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A complete set of required humanoid bones (Mixamo-named), so the rig maps as humanoid-ready.
fn humanoid_ready_rig() -> Tree {
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

/// Only a spine — every required arm/leg/hand/foot bone is missing, so the rig is NOT
/// humanoid-ready.
fn incomplete_rig() -> Tree {
    tree_v7400! {
        Objects: {
            Model: [1i64, "Hips\u{0}\u{1}Model", "LimbNode"] {},
            Model: [2i64, "Spine\u{0}\u{1}Model", "LimbNode"] {},
            Model: [3i64, "Head\u{0}\u{1}Model", "LimbNode"] {},
        },
        Connections: {
            C: ["OO", 2i64, 1i64] {},
            C: ["OO", 3i64, 2i64] {},
        },
    }
}

fn status(args: &[&str]) -> i32 {
    Command::new(AVATAR)
        .args(args)
        .output()
        .expect("run avatar binary")
        .status
        .code()
        .expect("process exited via signal, not code")
}

#[test]
fn armature_check_succeeds_on_humanoid_ready_rig() {
    let path = write_fbx(&humanoid_ready_rig(), "ready");
    let code = status(&["armature", "check", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "a humanoid-ready rig must exit 0");
}

#[test]
fn armature_check_fails_when_required_bones_missing() {
    let path = write_fbx(&incomplete_rig(), "incomplete");
    let code = status(&["armature", "check", path.to_str().unwrap()]);
    // The same must hold for --json so CI can gate on machine-readable output too.
    let code_json = status(&["armature", "check", "--json", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_ne!(code, 0, "missing required bones must fail the command");
    assert_ne!(code_json, 0, "--json must use the same exit code");
}

#[test]
fn armature_fix_dry_run_succeeds() {
    let path = write_fbx(&humanoid_ready_rig(), "fix");
    let code = status(&["armature", "fix", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0, "a dry-run fix must exit 0");
}
