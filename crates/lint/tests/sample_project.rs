//! End-to-end lint over a committed synthetic Unity project fixture.
//!
//! The fixture (`tests/fixtures/SampleProject`) deliberately contains a few issues so the rules
//! are exercised: a duplicate parameter (VRC011), a 9-control menu (VRC020), and a menu control
//! referencing an undeclared parameter (VRC021). The avatar SDK is present, so VRC001 must NOT
//! fire, and references to built-ins (GestureLeft) and declared params (VRCEmote) must be clean.

use std::path::PathBuf;

fn codes(report: &avatar_lint::LintReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn lints_sample_project() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/SampleProject");
    let report = avatar_lint::run(&fixture).expect("lint sample project");

    assert_eq!(report.unity_version.as_deref(), Some("2022.3.22f1"));
    assert_eq!(report.avatar_sdk_version.as_deref(), Some("3.7.0"));
    assert_eq!(report.parameter_assets, 1);
    assert_eq!(report.menu_assets, 1);

    let codes = codes(&report);
    // Avatar SDK is present -> no VRC001.
    assert!(!codes.contains(&"VRC001"), "unexpected VRC001: {codes:?}");
    // Duplicate parameter name.
    assert!(codes.contains(&"VRC011"), "expected VRC011 in {codes:?}");
    // Menu over 8 controls.
    assert!(codes.contains(&"VRC020"), "expected VRC020 in {codes:?}");
    // Undeclared parameter reference (GhostParam), but NOT for GestureLeft/VRCEmote.
    assert!(codes.contains(&"VRC021"), "expected VRC021 in {codes:?}");
    let dangling: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC021")
        .collect();
    assert_eq!(dangling.len(), 1, "only GhostParam should be dangling");
    assert!(dangling[0].message.contains("GhostParam"));

    // No budget error: only 4 synced (int 8 + 3 bool 1 + dup bool 1) = well under 256.
    assert!(!codes.contains(&"VRC010"), "unexpected VRC010: {codes:?}");
}
