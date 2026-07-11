//! Golden snapshots of the full lint report over each corpus project.
//!
//! The assertion tests (`sample_project` / `avatar_project` / `dynamics_project`) check the handful
//! of rule codes each fixture was built to exercise. These pin the *entire* serialized `LintReport`
//! instead — so any change to a count, a new or reworded diagnostic, a reordered field, or a
//! resolved-reference regression shows up as a reviewable diff rather than passing silently.
//!
//! Regenerate after an intentional change: `UPDATE_GOLDEN=1 cargo test -p avatar-lint`.

use avatar_testkit::{corpus, golden};

fn snapshot(project: &str) {
    let mut report =
        avatar_lint::run(&corpus(format!("projects/{project}"))).expect("lint runs clean");

    // Diagnostic ordering is not part of the report contract; sort for a stable snapshot.
    report.diagnostics.sort_by(|a, b| {
        (a.code, a.file.as_deref(), a.message.as_str()).cmp(&(
            b.code,
            b.file.as_deref(),
            b.message.as_str(),
        ))
    });

    let mut value = serde_json::to_value(&report).expect("report serializes to JSON");
    golden::redact_roots(&mut value); // scrub the absolute project_root prefix
    golden::assert_json(format!("tests/golden/{project}.lint.json"), &value);
}

#[test]
fn golden_sample_project() {
    snapshot("SampleProject");
}

#[test]
fn golden_avatar_project() {
    snapshot("AvatarProject");
}

#[test]
fn golden_dynamics_project() {
    snapshot("DynamicsProject");
}

#[test]
fn golden_clip_project() {
    snapshot("ClipProject");
}

#[test]
fn golden_quest_project() {
    snapshot("QuestProject");
}
