//! End-to-end lint over a committed synthetic project that contains a VRC Avatar Descriptor in a
//! prefab. The descriptor's Expression Parameters / Menu references resolve (their `.meta` guids
//! are present), but its FX playable layer points at a missing animator-controller guid (VRC032)
//! and its lip-sync is set to Viseme Blend Shape with no mesh and only 3 of 15 visemes (VRC033).

use std::path::PathBuf;

fn codes(report: &avatar_lint::LintReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn lints_avatar_descriptor_project() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/AvatarProject");
    let report = avatar_lint::run(&fixture).expect("lint avatar project");

    assert_eq!(report.descriptors, 1, "should find the descriptor prefab");
    assert_eq!(report.parameter_assets, 1);
    assert_eq!(report.menu_assets, 1);

    let codes = codes(&report);

    // Expression Parameters / Menu references resolve to recognized assets -> clean.
    assert!(!codes.contains(&"VRC030"), "unexpected VRC030: {codes:?}");
    assert!(!codes.contains(&"VRC031"), "unexpected VRC031: {codes:?}");

    // FX layer points at a guid not present in the project.
    assert!(codes.contains(&"VRC032"), "expected VRC032 in {codes:?}");
    let missing_controller: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC032")
        .collect();
    assert_eq!(missing_controller.len(), 1);
    assert!(missing_controller[0].message.contains("FX"));

    // Viseme blend-shape mode: no mesh assigned + wrong viseme count = two findings.
    let visemes: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC033")
        .collect();
    assert_eq!(visemes.len(), 2, "expected two VRC033 findings: {codes:?}");

    // A descriptor is present, so the "no descriptor" notice must not fire.
    assert!(!codes.contains(&"VRC002"), "unexpected VRC002: {codes:?}");

    // Eye Look enabled but no eye bones (VRC034) and eyelid blendshapes with no mesh (VRC035).
    assert!(codes.contains(&"VRC034"), "expected VRC034 in {codes:?}");
    assert!(codes.contains(&"VRC035"), "expected VRC035 in {codes:?}");

    // Parameter wiring against the resolved Gesture-layer controller (Hands.controller):
    //  - 'FloatThing' is declared but used by no controller -> VRC036
    //  - 'Toggle1' is Bool here but Int in the controller -> VRC037
    //  - 'VRCEmote' is a default-layer expression param -> must NOT be flagged
    let vrc036: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC036")
        .collect();
    assert_eq!(vrc036.len(), 1, "expected one VRC036: {codes:?}");
    assert!(vrc036[0].message.contains("FloatThing"));
    assert!(!vrc036.iter().any(|d| d.message.contains("VRCEmote")));

    let vrc037: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC037")
        .collect();
    assert_eq!(vrc037.len(), 1, "expected one VRC037: {codes:?}");
    assert!(vrc037[0].message.contains("Toggle1"));

    // --- Animator controller (Hands.controller) ---
    assert_eq!(report.controllers, 1, "should find the controller");
    // Undeclared condition parameter, undeclared blend parameter, no default state, duplicate
    // parameter, and mixed Write Defaults — one of each.
    for code in ["VRC040", "VRC041", "VRC042", "VRC043", "VRC044"] {
        assert!(codes.contains(&code), "expected {code} in {codes:?}");
    }
    let vrc040: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC040")
        .collect();
    assert_eq!(vrc040.len(), 1);
    assert!(vrc040[0].message.contains("Undeclared"));
    let vrc041: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC041")
        .collect();
    assert!(vrc041[0].message.contains("MissingBlend"));
}
