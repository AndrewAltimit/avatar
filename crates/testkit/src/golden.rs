//! Golden-file snapshot comparison.

use std::fs;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

/// True when `UPDATE_GOLDEN` is set to a non-empty, non-`0` value — i.e. snapshots should be
/// (re)written instead of compared. Run `UPDATE_GOLDEN=1 cargo test` after an intentional change.
pub fn update_enabled() -> bool {
    matches!(std::env::var("UPDATE_GOLDEN"), Ok(v) if !v.is_empty() && v != "0")
}

/// Compare `value` against the golden file at `path`, panicking on any mismatch.
///
/// The value is serialized to canonical pretty JSON (2-space indent, trailing newline). With
/// `UPDATE_GOLDEN` set the golden file is (re)written and the comparison is skipped; otherwise a
/// missing golden file is an error telling you to regenerate, and a content mismatch panics with a
/// readable diff.
///
/// `path` is resolved relative to the consuming crate's directory (`CARGO_MANIFEST_DIR`) when not
/// absolute, so `"tests/golden/x.json"` lands beside the test.
pub fn assert_json(path: impl AsRef<Path>, value: &impl Serialize) {
    let path = resolve(path.as_ref());
    let actual = canonical_json(value);

    if update_enabled() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("create golden dir {}: {e}", parent.display()));
        }
        fs::write(&path, &actual)
            .unwrap_or_else(|e| panic!("write golden {}: {e}", path.display()));
        eprintln!("golden: UPDATED {}", path.display());
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden file {} ({e}).\n  Regenerate with: UPDATE_GOLDEN=1 cargo test",
            path.display()
        )
    });

    if expected != actual {
        panic!("{}", diff_message(&path, &expected, &actual));
    }
}

/// Serialize to deterministic pretty JSON with a trailing newline (so the file is a clean,
/// editor-friendly, diffable artifact).
fn canonical_json(value: &impl Serialize) -> String {
    let mut s = serde_json::to_string_pretty(value).expect("value serializes to JSON");
    s.push('\n');
    s
}

fn resolve(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        let base = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for cargo-run tests");
        Path::new(&base).join(path)
    }
}

/// Replace every occurrence of `from` with `to` inside every string anywhere in `value`. Used to
/// scrub machine-specific values (absolute paths) out of a report before snapshotting.
pub fn redact(value: &mut Value, from: &str, to: &str) {
    if from.is_empty() {
        return;
    }
    match value {
        // `replace` returns an unchanged copy when `from` is absent, so this is correct either way.
        Value::String(s) => *s = s.replace(from, to),
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, from, to)),
        Value::Object(map) => map.values_mut().for_each(|v| redact(v, from, to)),
        _ => {}
    }
}

/// Scrub the absolute workspace-root prefix (and its canonicalized form) from `value`, replacing it
/// with the stable placeholder `<ROOT>`. Reports carry absolute paths (`project_root`, a report's
/// `source`); this makes the snapshot identical on every machine.
pub fn redact_roots(value: &mut Value) {
    let root = crate::workspace_root();
    // Replace the longer (canonicalized) form first so it can't be left half-substituted.
    if let Ok(canon) = root.canonicalize() {
        redact(value, &canon.to_string_lossy(), "<ROOT>");
    }
    redact(value, &root.to_string_lossy(), "<ROOT>");
}

/// A line-oriented diff message pointing at the first divergence. Kept dependency-free and small;
/// the goal is "show me where it changed", not a full Myers diff.
fn diff_message(path: &Path, expected: &str, actual: &str) -> String {
    use std::fmt::Write;

    let exp: Vec<&str> = expected.lines().collect();
    let act: Vec<&str> = actual.lines().collect();
    let first = (0..exp.len().max(act.len()))
        .find(|&i| exp.get(i) != act.get(i))
        .unwrap_or(0);

    let mut out = String::new();
    let _ = writeln!(
        out,
        "golden mismatch: {}\n  Re-run with UPDATE_GOLDEN=1 to accept the change after reviewing it.\n  First difference at line {} ({} expected lines, {} actual):",
        path.display(),
        first + 1,
        exp.len(),
        act.len(),
    );
    let lo = first.saturating_sub(2);
    let hi = (first + 3).min(exp.len().max(act.len()));
    for i in lo..hi {
        let marker = if exp.get(i) != act.get(i) { ">>" } else { "  " };
        if let Some(e) = exp.get(i) {
            let _ = writeln!(out, "  {marker} - {e}");
        }
        if let Some(a) = act.get(i) {
            let _ = writeln!(out, "  {marker} + {a}");
        }
    }
    out
}
