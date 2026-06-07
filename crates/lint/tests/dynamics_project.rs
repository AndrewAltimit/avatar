//! End-to-end lint over a committed synthetic project exercising the Avatar-Dynamics (PhysBone)
//! rules (VRC050–052), the avatar-level Write-Defaults rule (VRC045), and the viseme-entry rule
//! (VRC038).
//!
//! The descriptor's FX layer (write-defaults all-on) and Gesture layer (all-off) both resolve, so
//! the union mixes -> VRC045. Its 15 viseme entries include a duplicate and an empty slot ->
//! VRC038 (x2). The prefab carries four PhysBones: one well-formed, one with an unresolvable root
//! (VRC050), one that moves no transforms (VRC051), and one whose collider slots are all null
//! (VRC052) — plus the well-formed one with a wired collider, which must NOT trip VRC052.

use std::path::PathBuf;

fn codes(report: &avatar_lint::LintReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn lints_dynamics_project() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/DynamicsProject");
    let report = avatar_lint::run(&fixture).expect("lint dynamics project");

    assert_eq!(report.descriptors, 1, "should find the descriptor prefab");
    assert_eq!(report.controllers, 2, "should find both controllers");

    let codes = codes(&report);

    // Both layer controllers resolve and disagree on Write Defaults -> exactly one VRC045.
    let vrc045: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC045")
        .collect();
    assert_eq!(vrc045.len(), 1, "expected one VRC045: {codes:?}");
    assert!(vrc045[0].hint.is_some());

    // Each individual controller is internally consistent, so no VRC044.
    assert!(!codes.contains(&"VRC044"), "unexpected VRC044: {codes:?}");

    // Viseme entries: one duplicate ("vrc.v_aa") + one empty slot -> two VRC038.
    let vrc038: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC038")
        .collect();
    assert_eq!(vrc038.len(), 2, "expected two VRC038: {codes:?}");
    // Count is exactly 15 with the mesh assigned -> VRC033 must NOT fire.
    assert!(!codes.contains(&"VRC033"), "unexpected VRC033: {codes:?}");

    // PhysBones: one unresolvable root, one zero-transform, one all-null collider list.
    let vrc050: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC050")
        .collect();
    assert_eq!(vrc050.len(), 1, "expected one VRC050: {codes:?}");
    assert!(vrc050[0].hint.is_some());

    let vrc051: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC051")
        .collect();
    assert_eq!(vrc051.len(), 1, "expected one VRC051: {codes:?}");
    assert!(vrc051[0].hint.is_some());

    // Only the Skirt PhysBone (two null collider slots) trips VRC052; the wired Hair PhysBone and
    // the empty-list Tail PhysBone do not.
    let vrc052: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC052")
        .collect();
    assert_eq!(vrc052.len(), 1, "expected one VRC052: {codes:?}");
    assert!(vrc052[0].hint.is_some());
}
