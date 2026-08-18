//! End-to-end migration of the synthetic SDK2 fixture project (`fixtures/projects/Sdk2Project`):
//! every migration path fires (descriptor + PipelineManager retype, root motion off, subtree strip,
//! Cloth drop + capsule → PhysBone collider, DynamicBone(+collider) → PhysBone, an added skirt
//! chain, rig-derived eye look, FX from gesture overrides with a muscle curve dropped, the
//! missing-shader-include warning). The report and the rewritten prefab are pinned as goldens.
//!
//! Regenerate after an intentional change: `UPDATE_GOLDEN=1 cargo test -p avatar-migrate`.

use std::fs;
use std::path::{Path, PathBuf};

use avatar_migrate::{MigrateOptions, PhysBoneRootSpec, migrate};
use avatar_testkit::{corpus, golden};
use avatar_unity_yaml::UnityFile;

fn fresh_out_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "avatar-migrate-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn options(out: &Path) -> MigrateOptions {
    let mut opts = MigrateOptions::new(corpus("projects/Sdk2Project"), out, "Fixture");
    opts.strip = vec!["Vest".into()];
    opts.drop_cloth = true;
    opts.capsules_to_physbone_colliders = true;
    opts.physbone_roots = vec![PhysBoneRootSpec::parse("Hips|Spine,Left leg").unwrap()];
    opts.eye_bones = Some(("Eye_L".into(), "Eye_R".into()));
    opts
}

/// Golden comparison for a text file, mirroring `golden::assert_json`'s UPDATE_GOLDEN contract.
fn assert_text(rel: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    if golden::update_enabled() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        eprintln!("golden: UPDATED {}", path.display());
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden {} ({e}); run with UPDATE_GOLDEN=1",
            path.display()
        )
    });
    if expected != actual {
        let mismatch = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "golden mismatch for {} at line {} (expected {} lines, got {}); run with UPDATE_GOLDEN=1 to accept",
            path.display(),
            mismatch + 1,
            expected.lines().count(),
            actual.lines().count()
        );
    }
}

#[test]
fn golden_sdk2_project_migration() {
    let out = fresh_out_dir("golden");
    let opts = options(&out);
    let report = migrate(&opts).expect("migration runs");

    // ---- the output tree
    let assets = out.join("Assets");
    assert!(assets.join("Fixture_SDK3/Fixture.prefab").is_file());
    assert!(assets.join("Fixture_SDK3/Fixture.prefab.meta").is_file());
    assert!(assets.join("Fixture_SDK3/FX/FX.controller").is_file());
    assert!(assets.join("Fixture_SDK3/FX/Gesture_Fist.anim").is_file());
    assert!(
        assets
            .join("Fixture_SDK3/FX/Gesture_Victory.anim")
            .is_file()
    );
    assert!(
        assets
            .join("Fixture_SDK3/FX/Gesture_Neutral.anim")
            .is_file()
    );
    assert!(assets.join("Fixture_SDK3/Parameters.asset").is_file());
    assert!(assets.join("Fixture_SDK3/Menu.asset").is_file());
    // Source assets copied, SDK2 bits and the old prefab not.
    assert!(assets.join("Anim/smile.anim").is_file());
    assert!(assets.join("Mat/Body.mat").is_file());
    assert!(
        !assets.join("VRCSDK").exists(),
        "SDK2 VRCSDK must not be copied"
    );
    assert!(
        !assets.join("Avatar.prefab").exists(),
        "the SDK2 prefab is replaced, not copied"
    );
    assert!(out.join("Packages/vpm-manifest.json").is_file());
    assert!(out.join("ProjectSettings/ProjectVersion.txt").is_file());
    let manifest = fs::read_to_string(out.join("Packages/vpm-manifest.json")).unwrap();
    assert!(manifest.contains("\"com.vrchat.avatars\""));

    // ---- the migrated prefab parses and is structurally right
    let prefab = fs::read_to_string(assets.join("Fixture_SDK3/Fixture.prefab")).unwrap();
    let file = UnityFile::parse(&prefab).expect("migrated prefab is valid Unity YAML");
    let mono: Vec<_> = file
        .documents
        .iter()
        .filter(|d| d.class_id == 114)
        .collect();
    let script_ids: Vec<i64> = mono
        .iter()
        .filter_map(|d| d.body["m_Script"]["fileID"].as_i64())
        .collect();
    assert!(script_ids.contains(&542108242), "SDK3 descriptor present");
    assert!(script_ids.contains(&-1427037861), "PipelineManager present");
    assert_eq!(
        script_ids.iter().filter(|&&id| id == 1661641543).count(),
        2,
        "hair + skirt PhysBones"
    );
    assert_eq!(
        script_ids.iter().filter(|&&id| id == -1631200402).count(),
        2,
        "head + leg colliders"
    );
    assert!(!prefab.contains("Cloth:"), "Cloth removed");
    assert!(!prefab.contains("CapsuleCollider:"), "capsule retyped");
    assert!(
        !prefab.contains("m_Name: Vest") && !prefab.contains("Camera:"),
        "vest subtree stripped"
    );
    assert!(prefab.contains("m_ApplyRootMotion: 0"));
    // Untouched objects are byte-identical to the source.
    let src = fs::read_to_string(corpus("projects/Sdk2Project/Assets/Avatar.prefab")).unwrap();
    let hair_1 = "--- !u!4 &408\nTransform:\n  m_GameObject: {fileID: 108}\n";
    assert!(src.contains(hair_1) && prefab.contains(hair_1));

    // ---- report: expressions were split from muscles, slots classified
    let fx = report.fx.as_ref().expect("FX generated");
    assert_eq!(fx.gestures.len(), 2);
    let fist = fx
        .gestures
        .iter()
        .find(|g| g.gesture_name == "Fist")
        .unwrap();
    assert_eq!(
        fist.blendshapes.len(),
        2,
        "two blendshapes lifted from smile.anim"
    );
    assert_eq!(
        fist.dropped_curves, 1,
        "the finger muscle curve was dropped"
    );
    assert!(fx.skipped.iter().any(|(slot, _)| slot == "IDLE"));
    assert!(
        report.warnings.iter().any(|w| w.contains("#include")),
        "{:?}",
        report.warnings
    );
    assert!(report.eye_look.is_some());

    // ---- goldens (paths redacted)
    let mut value = serde_json::to_value(&report).unwrap();
    value["output_project"] = serde_json::Value::String("<out>".into());
    golden::redact_roots(&mut value);
    golden::assert_json("tests/golden/Sdk2Project.migrate.json", &value);
    assert_text("tests/golden/Sdk2Project.migrated.prefab.txt", &prefab);
    let controller = fs::read_to_string(assets.join("Fixture_SDK3/FX/FX.controller")).unwrap();
    assert_text("tests/golden/Sdk2Project.FX.controller.txt", &controller);

    let _ = fs::remove_dir_all(&out);
}

#[test]
fn dry_run_writes_nothing_and_refuses_existing_output() {
    let out = fresh_out_dir("dry");
    let mut opts = options(&out);
    opts.dry_run = true;
    let report = migrate(&opts).expect("dry run");
    assert!(report.dry_run);
    assert!(!out.exists(), "dry run must not create the output");
    assert_eq!(report.assets_copied, 0);

    // A real run refuses to clobber an existing Assets/.
    fs::create_dir_all(out.join("Assets")).unwrap();
    let mut opts = options(&out);
    opts.dry_run = false;
    let err = migrate(&opts).unwrap_err().to_string();
    assert!(err.contains("already exists"), "{err}");
    let _ = fs::remove_dir_all(&out);
}

#[test]
fn unknown_strip_target_is_an_error() {
    let out = fresh_out_dir("strip");
    let mut opts = options(&out);
    opts.strip = vec!["NoSuchObject".into()];
    opts.dry_run = true;
    let err = format!("{:#}", migrate(&opts).unwrap_err());
    assert!(err.contains("NoSuchObject"), "{err}");
}
