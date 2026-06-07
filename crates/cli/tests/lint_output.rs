//! End-to-end coverage for `avatar lint`'s human report: that remediation hints render as indented
//! `hint:` lines, that `-o/--output` writes the same report to a file instead of stdout, and that a
//! `Diagnostic`'s hint survives JSON serialization (the `--json` path).
//!
//! Driven through the real `avatar` binary against the committed lint fixture in the sibling
//! `avatar-lint` crate (a synthetic project with a duplicate parameter, an oversized menu, etc.).

use std::path::PathBuf;
use std::process::Command;

const AVATAR: &str = env!("CARGO_BIN_EXE_avatar");

/// The committed SampleProject fixture lives in the `avatar-lint` crate's tests/ tree.
fn sample_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../lint/tests/fixtures/SampleProject")
}

#[test]
fn lint_human_report_renders_hints() {
    let out = Command::new(AVATAR)
        .args(["lint", sample_project().to_str().unwrap()])
        .output()
        .expect("run avatar lint");
    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");

    // The fixture trips at least one rule that now carries a remediation hint (e.g. VRC020's
    // "Move extra controls into a sub-menu."), so the human report must include an indented
    // `hint:` line.
    assert!(
        stdout.contains("hint:"),
        "expected an indented hint line in the lint report:\n{stdout}"
    );
}

#[test]
fn lint_output_flag_writes_report_to_file() {
    let dir = std::env::temp_dir();
    let file = dir.join(format!("avatar-lint-{}-report.txt", std::process::id()));

    // `-o <file>` writes the report to the file; stdout stays quiet (just the "wrote …" note goes
    // to stderr).
    let out = Command::new(AVATAR)
        .args([
            "lint",
            sample_project().to_str().unwrap(),
            "-o",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("run avatar lint -o");

    let written = std::fs::read_to_string(&file).expect("report file written");
    let _ = std::fs::remove_file(&file);

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    assert!(
        stdout.trim().is_empty(),
        "with -o, the report should not also go to stdout:\n{stdout}"
    );
    // The file holds the full human report, hints included.
    assert!(
        written.contains("Lint:"),
        "report header missing:\n{written}"
    );
    assert!(
        written.contains("hint:"),
        "hint lines missing from the written report:\n{written}"
    );
}

#[test]
fn lint_json_output_flag_writes_json_to_file() {
    let dir = std::env::temp_dir();
    let file = dir.join(format!("avatar-lint-{}-report.json", std::process::id()));

    Command::new(AVATAR)
        .args([
            "lint",
            "--json",
            sample_project().to_str().unwrap(),
            "-o",
            file.to_str().unwrap(),
        ])
        .output()
        .expect("run avatar lint --json -o");

    let written = std::fs::read_to_string(&file).expect("json report file written");
    let _ = std::fs::remove_file(&file);

    // `-o` honors `--json`: the file parses as JSON and the diagnostics carry `hint` fields.
    let value: serde_json::Value = serde_json::from_str(&written).expect("file holds valid JSON");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["hint"].is_string()),
        "at least one diagnostic should carry a hint string in JSON output"
    );
}

#[test]
fn diagnostic_hint_serde_roundtrip() {
    // A populated hint survives JSON serialization and parsing — the contract `--json` relies on.
    let d = avatar_lint::Diagnostic {
        severity: avatar_lint::Severity::Error,
        code: "VRC010",
        message: "over budget".into(),
        file: Some("Assets/Params.asset".into()),
        hint: Some("Unsync some parameters.".into()),
    };
    let json = serde_json::to_string(&d).expect("serialize diagnostic");
    let back: serde_json::Value = serde_json::from_str(&json).expect("parse back");
    assert_eq!(back["hint"], "Unsync some parameters.");
    assert_eq!(back["code"], "VRC010");
}
