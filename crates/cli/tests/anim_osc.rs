//! End-to-end coverage of the `avatar anim-gen` and `avatar osc query` subcommands, driven through
//! the real `avatar` binary (`CARGO_BIN_EXE_avatar`). These exercise the CLI wiring on top of the
//! `avatar-anim-gen` and `avatar-osc` libraries: the deterministic, no-socket paths (generation and
//! offline OSCQuery parsing), so they run anywhere without a VRChat instance.

use std::path::PathBuf;
use std::process::Command;

const AVATAR: &str = env!("CARGO_BIN_EXE_avatar");

/// Run the binary and return `(exit_code, stdout)`.
fn run(args: &[&str]) -> (i32, String) {
    let out = Command::new(AVATAR)
        .args(args)
        .output()
        .expect("run avatar binary");
    let code = out.status.code().expect("exited via signal, not code");
    (
        code,
        String::from_utf8(out.stdout).expect("stdout is utf-8"),
    )
}

/// A unique temp path per (pid, label) so concurrent tests don't collide.
fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("avatar-cli-{}-{label}", std::process::id()))
}

#[test]
fn anim_gen_blendtree_emits_a_valid_206_document() {
    let (code, out) = run(&[
        "anim-gen",
        "blendtree",
        "--name",
        "FistBlend",
        "--parameter",
        "GestureLeftWeight",
        "--clip",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@0.0",
        "--clip",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb@1.0",
        "--tree-only",
    ]);
    assert_eq!(code, 0);
    assert!(out.contains("%YAML 1.1"), "has the Unity preamble");
    assert!(out.contains("--- !u!206 &"), "is a BlendTree document");
    assert!(out.contains("m_BlendParameter: GestureLeftWeight"));
    // Both child clips made it in, referenced by guid.
    assert!(out.contains("guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(out.contains("guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}

#[test]
fn anim_gen_blendtree_default_emits_state_machine_fragment() {
    let (code, out) = run(&[
        "anim-gen",
        "blendtree",
        "--name",
        "FistBlend",
        "--clip",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@0.0",
    ]);
    assert_eq!(code, 0);
    // The self-contained fragment carries the state machine + state + tree.
    assert!(out.contains("--- !u!1107 &"), "AnimatorStateMachine");
    assert!(out.contains("--- !u!1102 &"), "AnimatorState");
    assert!(out.contains("--- !u!206 &"), "BlendTree");
}

#[test]
fn anim_gen_blendtree_is_deterministic() {
    let args = [
        "anim-gen",
        "blendtree",
        "--name",
        "Repeatable",
        "--clip",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@0.0",
        "--tree-only",
    ];
    let (_, a) = run(&args);
    let (_, b) = run(&args);
    assert_eq!(a, b, "the same input must produce byte-identical YAML");
}

#[test]
fn anim_gen_clip_emits_blendshape_and_toggle_curves() {
    let (code, out) = run(&[
        "anim-gen",
        "clip",
        "--name",
        "Smile",
        "--blendshape",
        "Body:Smile:100",
        "--toggle",
        "Armature/Head/Hat",
    ]);
    assert_eq!(code, 0);
    assert!(out.contains("--- !u!74 &"), "is an AnimationClip document");
    assert!(out.contains("m_Name: Smile"));
    assert!(out.contains("attribute: blendShape.Smile"));
    assert!(out.contains("path: Body"));
    assert!(out.contains("attribute: m_IsActive"));
    assert!(out.contains("path: Armature/Head/Hat"));
}

#[test]
fn anim_gen_clip_requires_at_least_one_curve() {
    let (code, _) = run(&["anim-gen", "clip", "--name", "Empty"]);
    assert_ne!(code, 0, "no --blendshape/--toggle must error");
}

#[test]
fn anim_gen_clip_writes_to_output_file() {
    let path = temp_path("clip.anim");
    let (code, _) = run(&[
        "anim-gen",
        "clip",
        "--name",
        "HatOn",
        "--toggle",
        "Armature/Hat",
        "-o",
        path.to_str().unwrap(),
    ]);
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
    assert!(written.contains("--- !u!74 &"), "wrote a real .anim file");
}

#[test]
fn anim_gen_clip_json_is_machine_readable_and_emits_no_raw_yaml() {
    let (code, out) = run(&[
        "anim-gen",
        "clip",
        "--name",
        "Smile",
        "--blendshape",
        "Body:Smile:100",
        "--json",
    ]);
    assert_eq!(code, 0);
    // In --json mode stdout is the report, not the YAML — it must be parseable JSON.
    let v: serde_json::Value = serde_json::from_str(&out).expect("stdout is JSON, not raw YAML");
    assert_eq!(v["kind"], "clip");
    assert!(v["clip_file_id"].is_u64(), "carries the allocated fileID");
    assert_eq!(v["written"], false, "no -o, so nothing was written");
    // The YAML asset is embedded in the report so an agent gets both in one call.
    assert!(
        v["yaml"].as_str().unwrap().contains("--- !u!74 &"),
        "report embeds the AnimationClip YAML"
    );
}

#[test]
fn anim_gen_controller_emits_a_full_fx_controller() {
    let (code, out) = run(&[
        "anim-gen",
        "controller",
        "--name",
        "FX",
        "--clip",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa@0.0",
        "--clip",
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb@1.0",
    ]);
    assert_eq!(code, 0);
    assert!(
        out.contains("--- !u!91 &"),
        "is an AnimatorController document"
    );
    assert!(out.contains("--- !u!206 &"), "wraps a BlendTree");
    assert!(out.contains("m_Name: FX"));
}

#[test]
fn anim_gen_dry_run_writes_nothing() {
    let path = temp_path("dry.anim");
    let _ = std::fs::remove_file(&path);
    let (code, out) = run(&[
        "anim-gen",
        "clip",
        "--name",
        "Dry",
        "--blendshape",
        "Body:Smile:100",
        "-o",
        path.to_str().unwrap(),
        "--dry-run",
    ]);
    let exists = path.exists();
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
    assert!(out.is_empty(), "dry-run prints nothing to stdout");
    assert!(!exists, "dry-run must not create the output file");
}

#[test]
fn anim_gen_refuses_to_overwrite_without_force() {
    let path = temp_path("guard.anim");
    let p = path.to_str().unwrap();
    let args = [
        "anim-gen",
        "clip",
        "--name",
        "Guard",
        "--blendshape",
        "Body:Smile:100",
        "-o",
        p,
    ];
    // First write succeeds; a second write to the same path is refused...
    assert_eq!(run(&args).0, 0, "first write succeeds");
    assert_ne!(run(&args).0, 0, "second write is refused (no --force)");
    // ...but --force allows it.
    let mut forced = args.to_vec();
    forced.push("--force");
    let forced_code = run(&forced).0;
    let _ = std::fs::remove_file(&path);
    assert_eq!(forced_code, 0, "--force permits overwrite");
}

const OSCQUERY_CONFIG: &str = r#"{
  "name": "MyAvatar",
  "FULL_PATH": "/",
  "ACCESS": 0,
  "CONTENTS": {
    "avatar": {
      "FULL_PATH": "/avatar",
      "ACCESS": 0,
      "CONTENTS": {
        "parameters": {
          "FULL_PATH": "/avatar/parameters",
          "ACCESS": 0,
          "CONTENTS": {
            "VRCEmote":  { "FULL_PATH": "/avatar/parameters/VRCEmote",  "TYPE": "i", "ACCESS": 3 },
            "Grounded":  { "FULL_PATH": "/avatar/parameters/Grounded",  "TYPE": "F", "ACCESS": 1 }
          }
        }
      }
    }
  }
}"#;

#[test]
fn osc_query_lists_parameters() {
    let path = temp_path("avtr.json");
    std::fs::write(&path, OSCQUERY_CONFIG).unwrap();
    let (code, out) = run(&["osc", "query", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
    assert!(out.contains("MyAvatar"));
    assert!(out.contains("VRCEmote"));
    assert!(out.contains("Grounded"));
    assert!(out.contains("read/write"), "VRCEmote is ACCESS 3");
}

#[test]
fn osc_query_json_is_machine_readable() {
    let path = temp_path("avtr-json.json");
    std::fs::write(&path, OSCQUERY_CONFIG).unwrap();
    let (code, out) = run(&["osc", "query", "--json", path.to_str().unwrap()]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["name"], "MyAvatar");
    let params = v["parameters"].as_array().expect("parameters array");
    assert_eq!(params.len(), 2);
    // VRCEmote (ACCESS 3) is both readable and writable.
    let emote = params
        .iter()
        .find(|p| p["name"] == "VRCEmote")
        .expect("VRCEmote present");
    assert_eq!(emote["readable"], true);
    assert_eq!(emote["writable"], true);
    assert_eq!(emote["type"], "i");
}
