//! Unit tests built on an in-memory `.unitypackage` synthesized with the gzip+tar writers, so they
//! run with no external fixtures. An env-gated integration test lives in `tests/`.

use super::*;

/// One asset to bake into a synthetic package.
struct Member<'a> {
    guid: &'a str,
    pathname: Option<&'a str>,
    asset: Option<&'a [u8]>,
    meta: Option<&'a [u8]>,
}

/// Build a gzip-compressed tar in memory shaped like a real `.unitypackage`.
fn build_package(members: &[Member]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);

    let mut append = |name: String, data: &[u8]| {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, data).unwrap();
    };

    for m in members {
        if let Some(p) = m.pathname {
            append(format!("{}/pathname", m.guid), p.as_bytes());
        }
        if let Some(a) = m.asset {
            append(format!("{}/asset", m.guid), a);
        }
        if let Some(meta) = m.meta {
            append(format!("{}/asset.meta", m.guid), meta);
        }
    }

    let gz = builder.into_inner().unwrap();
    gz.finish().unwrap()
}

#[test]
fn parses_files_and_folders() {
    let pkg_bytes = build_package(&[
        Member {
            guid: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/Avatar"),
            asset: None, // folder entry
            meta: Some(b"guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"),
        },
        Member {
            guid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            pathname: Some("Assets/Avatar/final.fbx"),
            asset: Some(b"FBX-BYTES"),
            meta: Some(b"fileFormatVersion: 2\nguid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n"),
        },
    ]);

    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    assert_eq!(pkg.len(), 2);
    assert_eq!(pkg.files().count(), 1);

    let fbx = pkg.find_by_path("Assets/Avatar/final.fbx").unwrap();
    assert!(fbx.is_file());
    assert_eq!(fbx.extension().as_deref(), Some("fbx"));
    assert_eq!(fbx.size(), 9);
    assert!(fbx.meta.is_some());

    let folder = pkg.get("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    assert!(!folder.is_file());
    assert!(folder.extension().is_none());
}

#[test]
fn pathname_keeps_only_first_line() {
    let pkg_bytes = build_package(&[Member {
        guid: "cccccccccccccccccccccccccccccccc",
        pathname: Some("Assets/Foo.cs\n00\n"),
        asset: Some(b"x"),
        meta: None,
    }]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    assert!(pkg.find_by_path("Assets/Foo.cs").is_some());
}

#[test]
fn summary_counts_by_extension_and_detects_sdk2() {
    let pkg_bytes = build_package(&[
        Member {
            guid: "0aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/VRCSDK/Plugins/VRCSDK2.dll"),
            asset: Some(b"MZ"),
            meta: None,
        },
        Member {
            guid: "1aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/VRCSDK/version.txt"),
            asset: Some(b"2021.04.21.11.58\n"),
            meta: None,
        },
        Member {
            guid: "2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/prefab-id-v1_avtr_xyz_1.prefab"),
            asset: Some(b"%YAML"),
            meta: None,
        },
        Member {
            guid: "3aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/a.fbx"),
            asset: Some(b"aaaa"),
            meta: None,
        },
        Member {
            guid: "4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/b.fbx"),
            asset: Some(b"bbbbbb"),
            meta: None,
        },
    ]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    let s = pkg.summary();
    assert_eq!(s.file_count, 5);
    assert_eq!(s.by_extension["fbx"].count, 2);
    assert_eq!(s.by_extension["fbx"].bytes, 10);
    assert_eq!(s.traits.vrc_sdk, Some(VrcSdk::Sdk2));
    assert_eq!(
        s.traits.sdk_version_txt.as_deref(),
        Some("2021.04.21.11.58")
    );
    assert!(s.traits.looks_like_avatar);
    assert_eq!(s.traits.prefab_count, 1);
}

#[test]
fn detects_world_when_scene_and_no_avatar() {
    let pkg_bytes = build_package(&[Member {
        guid: "5aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        pathname: Some("Assets/Cabin/Main.unity"),
        asset: Some(b"%YAML"),
        meta: None,
    }]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    let t = pkg.summary().traits;
    assert!(t.looks_like_world);
    assert!(!t.looks_like_avatar);
    assert_eq!(t.scene_count, 1);
}

#[test]
fn extracts_tree_with_meta_sidecars() {
    let pkg_bytes = build_package(&[
        Member {
            guid: "6aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/Avatar/final.fbx"),
            asset: Some(b"FBX"),
            meta: Some(b"guid: 6aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n"),
        },
        Member {
            guid: "7aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/EmptyDir"),
            asset: None,
            meta: None,
        },
    ]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();

    let dir = std::env::temp_dir().join(format!("upkg-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let report = pkg.extract(&dir).unwrap();
    assert_eq!(report.files_written, 1);
    assert_eq!(report.meta_written, 1);
    assert_eq!(report.folders_created, 1);

    let fbx = dir.join("Assets/Avatar/final.fbx");
    assert_eq!(std::fs::read(&fbx).unwrap(), b"FBX");
    let meta = dir.join("Assets/Avatar/final.fbx.meta");
    assert!(meta.is_file());
    assert!(dir.join("Assets/EmptyDir").is_dir());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rejects_path_traversal_on_extract() {
    let pkg_bytes = build_package(&[Member {
        guid: "8aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        pathname: Some("../escape.txt"),
        asset: Some(b"nope"),
        meta: None,
    }]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    let dir = std::env::temp_dir().join(format!("upkg-test-esc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let report = pkg.extract(&dir).unwrap();
    assert_eq!(report.files_written, 0);
    assert_eq!(report.skipped_unsafe.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overlap_finds_guid_and_path_collisions() {
    // Package A: shared GUID with different bytes, a unique asset, and a path that B also claims.
    let a = build_package(&[
        Member {
            guid: "dddddddddddddddddddddddddddddddd",
            pathname: Some("Assets/Shared/Poiyomi.shader"),
            asset: Some(b"VERSION-A"),
            meta: None,
        },
        Member {
            guid: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            pathname: Some("Assets/Common.cs"),
            asset: Some(b"x"),
            meta: None,
        },
    ]);
    // Package B: same GUID dddd with different bytes (conflict); the same path Common.cs under a
    // different GUID (path collision); plus an identical-bytes shared guid (harmless).
    let b = build_package(&[
        Member {
            guid: "dddddddddddddddddddddddddddddddd",
            pathname: Some("Assets/Shared/Poiyomi.shader"),
            asset: Some(b"VERSION-B-DIFFERENT"),
            meta: None,
        },
        Member {
            guid: "ffffffffffffffffffffffffffffffff",
            pathname: Some("Assets/Common.cs"),
            asset: Some(b"y"),
            meta: None,
        },
    ]);
    let pa = UnityPackage::read(&a[..]).unwrap();
    let pb = UnityPackage::read(&b[..]).unwrap();
    let report = pa.overlap(&pb);

    assert_eq!(report.guid_collisions.len(), 1);
    assert!(!report.guid_collisions[0].identical);
    assert_eq!(report.conflicting().count(), 1);
    assert_eq!(report.path_collisions.len(), 1);
    assert_eq!(report.path_collisions[0].path, "Assets/Common.cs");
    assert!(!report.is_clean());
}

#[test]
fn overlap_is_clean_for_disjoint_packages() {
    let a = build_package(&[Member {
        guid: "1111111111111111111111111111111a",
        pathname: Some("Assets/A.fbx"),
        asset: Some(b"a"),
        meta: None,
    }]);
    let b = build_package(&[Member {
        guid: "2222222222222222222222222222222b",
        pathname: Some("Assets/B.fbx"),
        asset: Some(b"b"),
        meta: None,
    }]);
    let pa = UnityPackage::read(&a[..]).unwrap();
    let pb = UnityPackage::read(&b[..]).unwrap();
    assert!(pa.overlap(&pb).is_clean());
}

#[test]
fn rejects_absolute_and_drive_letter_paths_on_extract() {
    // Old SDK exports leak absolute editor paths like `C:/Program Files/Unity/...UI.dll`.
    let pkg_bytes = build_package(&[
        Member {
            guid: "9aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("C:/Program Files/Unity/Editor/UnityEngine.UI.dll"),
            asset: Some(b"MZ"),
            meta: None,
        },
        Member {
            guid: "9baaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("/etc/passwd"),
            asset: Some(b"nope"),
            meta: None,
        },
        Member {
            guid: "9caaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            pathname: Some("Assets/keep.txt"),
            asset: Some(b"ok"),
            meta: None,
        },
    ]);
    let pkg = UnityPackage::read(&pkg_bytes[..]).unwrap();
    let dir = std::env::temp_dir().join(format!("upkg-test-abs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let report = pkg.extract(&dir).unwrap();
    assert_eq!(report.files_written, 1);
    assert_eq!(report.skipped_unsafe.len(), 2);
    assert!(dir.join("Assets/keep.txt").is_file());
    assert!(!dir.join("C:").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_archive_is_an_error() {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let builder = tar::Builder::new(gz);
    let gz = builder.into_inner().unwrap();
    let bytes = gz.finish().unwrap();
    assert!(UnityPackage::read(&bytes[..]).is_err());
}
