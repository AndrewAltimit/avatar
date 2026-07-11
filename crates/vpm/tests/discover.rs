//! Project discovery against the committed fixture corpus and temp directories: the up-walk to
//! `Packages/vpm-manifest.json`, the bare-`Assets/` fallback, `ProjectVersion.txt` parsing, and
//! locked-over-dependency version resolution — the paths `avatar lint`/`stats` rely on first.

use std::path::PathBuf;

use avatar_testkit::corpus;
use avatar_vpm::{AVATAR_SDK, UnityProject};

#[test]
fn discovers_project_root_from_a_nested_path() {
    let nested = corpus("projects/SampleProject").join("Assets/Avatar/Expressions");
    let project = UnityProject::discover(&nested).expect("walks up to the manifest");

    assert!(
        project.root.ends_with("SampleProject"),
        "root is the manifest's directory, not the start dir: {}",
        project.root.display()
    );
    assert_eq!(project.unity_version.as_deref(), Some("2022.3.22f1"));
    assert!(project.has_avatar_sdk());
    assert_eq!(project.package_version(AVATAR_SDK), Some("3.7.0"));
    // The manifest's locked section carries Modular Avatar; locked versions are authoritative.
    let ma = project
        .packages
        .iter()
        .find(|p| p.name == "nadena.dev.modular-avatar")
        .expect("locked-only package surfaces");
    assert_eq!(ma.version, "1.10.0");
    assert!(ma.locked);
    assert!(project.assets_dir().is_dir());
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("avatar-vpm-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn falls_back_to_a_bare_assets_project() {
    let root = temp_dir("bare");
    std::fs::create_dir_all(root.join("Assets")).unwrap();
    std::fs::create_dir_all(root.join("ProjectSettings")).unwrap();
    std::fs::write(
        root.join("ProjectSettings/ProjectVersion.txt"),
        "m_EditorVersion: 2019.4.31f1\n",
    )
    .unwrap();

    let project = UnityProject::discover(&root).expect("bare Assets/ project accepted");
    assert_eq!(project.unity_version.as_deref(), Some("2019.4.31f1"));
    assert!(project.packages.is_empty(), "no manifest -> no packages");
    assert!(!project.has_avatar_sdk());

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn locked_version_overrides_dependency_version() {
    let root = temp_dir("locked");
    std::fs::create_dir_all(root.join("Packages")).unwrap();
    std::fs::create_dir_all(root.join("Assets")).unwrap();
    std::fs::write(
        root.join("Packages/vpm-manifest.json"),
        r#"{
            "dependencies": { "com.vrchat.avatars": { "version": "^3.0.0" } },
            "locked": { "com.vrchat.avatars": { "version": "3.7.0" } }
        }"#,
    )
    .unwrap();

    let project = UnityProject::discover(&root).unwrap();
    assert_eq!(
        project.package_version(AVATAR_SDK),
        Some("3.7.0"),
        "locked is authoritative over the dependency range"
    );

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn no_project_is_an_actionable_error() {
    let root = temp_dir("empty");
    std::fs::create_dir_all(&root).unwrap();

    let err = UnityProject::discover(&root).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("vpm-manifest.json"),
        "names the manifest: {msg}"
    );
    assert!(msg.contains("Assets/"), "names the fallback: {msg}");

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn malformed_manifest_is_an_error_not_a_panic() {
    let root = temp_dir("malformed");
    std::fs::create_dir_all(root.join("Packages")).unwrap();
    std::fs::write(root.join("Packages/vpm-manifest.json"), "{ not json").unwrap();

    let err = UnityProject::discover(&root).unwrap_err();
    assert!(format!("{err:#}").contains("vpm-manifest.json"));

    std::fs::remove_dir_all(&root).unwrap();
}
