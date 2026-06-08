//! End-to-end lint over a committed synthetic Unity project fixture.
//!
//! The fixture (`fixtures/projects/SampleProject`) deliberately contains a few issues so the rules
//! are exercised: a duplicate parameter (VRC011), a 9-control menu (VRC020), and a menu control
//! referencing an undeclared parameter (VRC021). The avatar SDK is present, so VRC001 must NOT
//! fire, and references to built-ins (GestureLeft) and declared params (VRCEmote) must be clean.

fn codes(report: &avatar_lint::LintReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code).collect()
}

#[test]
fn lints_sample_project() {
    let fixture = avatar_testkit::corpus("projects/SampleProject");
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

    // VRC022: the DeadControl drives nothing (no parameter, sub-parameters, or sub-menu).
    let empty: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC022")
        .collect();
    assert_eq!(empty.len(), 1, "expected one VRC022: {codes:?}");
    assert!(empty[0].message.contains("DeadControl"));
    assert!(empty[0].hint.is_some());

    // VRC012 (project-wide unused expression params): DupName and LocalOnlyFloat are referenced by
    // no menu control or controller anywhere; VRCEmote (default-layer) and the menu-wired
    // Toggle1/Toggle2 are excluded. (DupName is declared twice but reported once.)
    let unused: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "VRC012")
        .collect();
    let unused_names: Vec<&str> = unused.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(unused.len(), 2, "expected two VRC012: {unused_names:?}");
    assert!(unused.iter().any(|d| d.message.contains("DupName")));
    assert!(unused.iter().any(|d| d.message.contains("LocalOnlyFloat")));
    assert!(!unused.iter().any(|d| d.message.contains("VRCEmote")));
    assert!(!unused.iter().any(|d| d.message.contains("Toggle1")));
    assert!(unused.iter().all(|d| d.hint.is_some()));
}
