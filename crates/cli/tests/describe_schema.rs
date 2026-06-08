//! End-to-end coverage of `avatar describe` (the one-shot asset snapshot) and `avatar schema` (the
//! `--json` output contract), driven through the real `avatar` binary. Fixtures are synthesized as
//! binary FBX in memory, like `exit_codes.rs`, so no committed asset is needed.

use std::io::Cursor;
use std::path::PathBuf;
use std::process::Command;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

const AVATAR: &str = env!("CARGO_BIN_EXE_avatar");

fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(AVATAR)
        .args(args)
        .output()
        .expect("run avatar binary");
    (
        out.status.code().expect("exited via signal, not code"),
        String::from_utf8(out.stdout).expect("stdout is utf-8"),
    )
}

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

/// A spine-only rig: required arm/leg/hand/foot bones are missing, so it is NOT humanoid-ready.
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

#[test]
fn describe_fbx_json_aggregates_inspect_armature_and_performance() {
    let path = write_fbx(&incomplete_rig(), "describe");
    let (code, out) = run(&["describe", "--json", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);

    // A non-humanoid-ready rig gates non-zero, like `armature check`.
    assert_ne!(code, 0, "missing required bones must fail describe");
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["target"], "fbx");
    // All three sub-reports are present in the one snapshot.
    assert!(
        v["fbx"]["inspect"]["version"].is_u64(),
        "has inspect summary"
    );
    assert_eq!(v["fbx"]["humanoid_ready"], false, "has armature verdict");
    assert!(
        v["fbx"]["performance"]["stats"].is_array(),
        "has performance stats"
    );
    assert!(v["project"].is_null(), "project side absent for an FBX");
}

#[test]
fn describe_human_output_is_readable() {
    let path = write_fbx(&incomplete_rig(), "describe-human");
    let (_, out) = run(&["describe", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert!(out.contains("Describe (FBX)"));
    assert!(out.contains("Armature"));
    assert!(out.contains("Performance"));
}

#[test]
fn schema_lists_available_names() {
    let (code, out) = run(&["schema"]);
    assert_eq!(code, 0);
    for name in ["describe", "lint", "stats", "armature", "fbx-inspect"] {
        assert!(out.contains(name), "lists schema '{name}'");
    }
}

#[test]
fn schema_emits_valid_json_schema_per_type() {
    for name in ["describe", "lint", "stats", "armature", "fbx-inspect"] {
        let (code, out) = run(&["schema", name]);
        assert_eq!(code, 0, "schema {name} exits 0");
        let v: serde_json::Value =
            serde_json::from_str(&out).unwrap_or_else(|_| panic!("schema {name} is JSON"));
        assert!(
            v.get("$schema").is_some(),
            "schema {name} is a JSON Schema document"
        );
        assert!(v.get("properties").is_some() || v.get("oneOf").is_some());
    }
}

#[test]
fn schema_all_bundles_every_schema() {
    let (code, out) = run(&["schema", "all"]);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON object");
    assert!(v["describe"].is_object() && v["armature"].is_object());
}

#[test]
fn schema_rejects_unknown_name() {
    let code = Command::new(AVATAR)
        .args(["schema", "definitely-not-a-schema"])
        .output()
        .unwrap()
        .status
        .code()
        .unwrap();
    assert_ne!(code, 0, "an unknown schema name must error");
}
