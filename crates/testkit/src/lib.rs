//! Shared test machinery for the workspace: a golden-file ("snapshot") harness and the in-code
//! synthetic-asset builders that feed it.
//!
//! # Golden tests
//!
//! A golden test runs a report-producing function over a fixture, serializes the *whole* report to
//! canonical JSON, and compares it byte-for-byte against a committed snapshot. Unlike the
//! assertion-based tests (which check the few fields someone thought to assert), a golden test
//! catches **any** change to the report surface — a new field, a reordered list, a changed
//! count — so silent regressions in what the agents consume surface as a failing diff.
//!
//! ```no_run
//! use avatar_testkit::golden;
//! # #[derive(serde::Serialize)] struct Report;
//! # fn run() -> Report { Report }
//! let report = run();
//! let mut value = serde_json::to_value(&report).unwrap();
//! golden::redact_roots(&mut value); // scrub machine-specific absolute paths
//! golden::assert_json("tests/golden/my-fixture.json", &value);
//! ```
//!
//! Regenerate every snapshot after an intentional change with `UPDATE_GOLDEN=1 cargo test`.

use std::path::{Path, PathBuf};

pub mod golden;

#[cfg(feature = "fbx")]
pub mod fbx;

/// The workspace root — the nearest ancestor of `CARGO_MANIFEST_DIR` that holds `Cargo.lock`.
///
/// Resolved at *runtime* (Cargo sets `CARGO_MANIFEST_DIR` for the running test binary to the
/// consuming crate's directory), so a helper here returns the caller's workspace root rather than
/// this crate's. Use it to reach the shared corpus regardless of which crate the test lives in.
pub fn workspace_root() -> PathBuf {
    let start =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set for cargo-run tests");
    let mut dir = PathBuf::from(start);
    loop {
        if dir.join("Cargo.lock").exists() {
            return dir;
        }
        if !dir.pop() {
            panic!("could not locate workspace root (no Cargo.lock above CARGO_MANIFEST_DIR)");
        }
    }
}

/// A path inside the committed fixture corpus (`<workspace>/fixtures/...`).
pub fn corpus(rel: impl AsRef<Path>) -> PathBuf {
    workspace_root().join("fixtures").join(rel)
}
