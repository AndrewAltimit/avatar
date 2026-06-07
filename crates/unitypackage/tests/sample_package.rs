//! End-to-end test over a real `.unitypackage`.
//!
//! Set `AVATAR_SAMPLE_UNITYPACKAGE` to the absolute path of a `.unitypackage` (an avatar or a
//! world export) to exercise the full open -> summarize -> extract pipeline against real data. If
//! the var is unset the test prints a one-line skip notice and returns OK, so CI (and other
//! machines) without a sample stay green. Never commit user packages — see `.gitignore`.
//!
//! Optionally set `AVATAR_SAMPLE_UNITYPACKAGE_WORLD` as well to additionally exercise the cross-
//! package `overlap` (testbed) path.

use std::path::PathBuf;

use avatar_unitypackage::UnityPackage;

#[test]
fn opens_and_extracts_sample_package() {
    let Ok(path) = std::env::var("AVATAR_SAMPLE_UNITYPACKAGE") else {
        eprintln!("skip: AVATAR_SAMPLE_UNITYPACKAGE not set");
        return;
    };
    let path = PathBuf::from(path);

    let pkg = UnityPackage::open(&path).expect("open sample unitypackage");
    assert!(!pkg.is_empty(), "package has entries");
    let summary = pkg.summary();
    assert_eq!(
        summary.entry_count,
        summary.file_count + summary.folder_count
    );
    // Per-extension file counts must sum to the file total.
    let ext_files: usize = summary.by_extension.values().map(|s| s.count).sum();
    assert_eq!(ext_files, summary.file_count);

    // Extract into a temp dir and confirm the reconstructed tree is consistent with the report,
    // and that no unsafe (absolute / drive-letter / traversal) path was written outside the dest.
    let dir = std::env::temp_dir().join(format!("avatar-upkg-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let report = pkg.extract(&dir).expect("extract sample");
    assert!(dir.exists());
    assert!(
        !dir.join("C:").exists(),
        "drive-letter path leaked into dest"
    );

    let on_disk = count_files(&dir);
    assert_eq!(
        on_disk,
        report.files_written + report.meta_written,
        "files on disk match report (assets + .meta sidecars)"
    );

    eprintln!(
        "sample {}: {} entries, {} files written, {} meta, {} skipped (absolute/non-project); sdk {:?}, avatar={} world={}",
        path.display(),
        summary.entry_count,
        report.files_written,
        report.meta_written,
        report.skipped_unsafe.len(),
        summary.traits.vrc_sdk,
        summary.traits.looks_like_avatar,
        summary.traits.looks_like_world,
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cross_checks_avatar_against_world() {
    let (Ok(av), Ok(world)) = (
        std::env::var("AVATAR_SAMPLE_UNITYPACKAGE"),
        std::env::var("AVATAR_SAMPLE_UNITYPACKAGE_WORLD"),
    ) else {
        eprintln!(
            "skip: set AVATAR_SAMPLE_UNITYPACKAGE and AVATAR_SAMPLE_UNITYPACKAGE_WORLD to run the testbed path"
        );
        return;
    };
    let avatar = UnityPackage::open(&PathBuf::from(av)).expect("open avatar");
    let world = UnityPackage::open(&PathBuf::from(world)).expect("open world");
    let overlap = avatar.overlap(&world);

    // Conflicting collisions are a subset of all GUID collisions.
    assert!(overlap.conflicting().count() <= overlap.guid_collisions.len());
    eprintln!(
        "overlap: {} shared GUID(s), {} conflicting, {} path collision(s), clean={}",
        overlap.guid_collisions.len(),
        overlap.conflicting().count(),
        overlap.path_collisions.len(),
        overlap.is_clean(),
    );
}

fn count_files(dir: &std::path::Path) -> usize {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            n += count_files(&p);
        } else {
            n += 1;
        }
    }
    n
}
