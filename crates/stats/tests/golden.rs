//! Golden snapshots of the performance reports: per-avatar component stats over each corpus
//! project, and geometry stats over the in-code synthetic humanoid skeleton.
//!
//! These pin the whole `PerfReport` surface — every metric's value, rank, per-platform split, and
//! the `not_evaluated` list — so a change to a threshold table, a metric, or the analysis shows up
//! as a diff. Regenerate: `UPDATE_GOLDEN=1 cargo test -p avatar-stats`.

use avatar_testkit::{corpus, golden};

fn project_snapshot(project: &str) {
    let mut reports = avatar_stats::analyze_project(&corpus(format!("projects/{project}")))
        .expect("analyze project");
    // One report per avatar; order is incidental, so sort by source for a stable snapshot.
    reports.sort_by(|a, b| a.source.cmp(&b.source));

    let mut value = serde_json::to_value(&reports).expect("reports serialize to JSON");
    golden::redact_roots(&mut value); // each report's `source` carries the absolute project path
    golden::assert_json(format!("tests/golden/{project}.project-stats.json"), &value);
}

#[test]
fn golden_sample_project_stats() {
    project_snapshot("SampleProject");
}

#[test]
fn golden_avatar_project_stats() {
    project_snapshot("AvatarProject");
}

#[test]
fn golden_dynamics_project_stats() {
    project_snapshot("DynamicsProject");
}

#[test]
fn golden_humanoid_skeleton_fbx_stats() {
    let bytes = avatar_testkit::fbx::humanoid_skeleton();
    let report = avatar_stats::analyze_fbx_bytes(&bytes, "humanoid_skeleton.fbx")
        .expect("analyze synthetic FBX");
    // `source` is the literal label we passed — no path to redact.
    let value = serde_json::to_value(&report).expect("report serializes to JSON");
    golden::assert_json("tests/golden/humanoid_skeleton.fbx-stats.json", &value);
}
