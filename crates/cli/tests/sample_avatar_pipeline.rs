//! Ground-truth end-to-end test over a **real** `.unitypackage`, driven through the real `avatar`
//! binary (`CARGO_BIN_EXE_avatar`) exactly as CI / a user would invoke it.
//!
//! Set `AVATAR_SAMPLE_UNITYPACKAGE` to an avatar `.unitypackage` to run it; if unset the test prints
//! a one-line skip notice and returns OK, so machines without the sample stay green. Never commit
//! user packages (see `.gitignore`).
//!
//! Why this exists: the lint / project-stats *read* paths are already covered on every push by the
//! committed synthetic fixture projects (`fixtures/projects/*`), but two paths could only
//! be exercised against real data and otherwise self-skipped — **binary FBX parsing** of a
//! real-world (ripped / MMD-exported) rig, and **`.unitypackage` open/extract**. This walks the full
//! pipeline on real bytes: extract the package, then run the actual `describe` / `lint` / `stats`
//! commands over the reconstructed Unity tree and assert their `--json` contracts hold. On the
//! self-hosted runner (where the local sample is always present) this turns those skips into real,
//! every-push coverage. See `.github/workflows/main-ci.yml`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Run the `avatar` binary with `args`, returning (success, stdout). Panics if the process can't be
/// spawned at all.
fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_avatar"))
        .args(args)
        .output()
        .expect("spawn avatar binary");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

/// Collect up to `limit` `.fbx` files under `root` (case-insensitive extension), in walk order.
fn find_fbx(root: &Path, limit: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
            {
                found.push(p);
                if found.len() >= limit {
                    return found;
                }
            }
        }
    }
    found
}

#[test]
fn real_package_drives_the_full_read_pipeline() {
    let Ok(pkg) = std::env::var("AVATAR_SAMPLE_UNITYPACKAGE") else {
        eprintln!("skip: AVATAR_SAMPLE_UNITYPACKAGE not set");
        return;
    };

    let proj = std::env::temp_dir().join(format!("avatar-gt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&proj);

    // 1. Extract the real package through the real CLI.
    let (ok, _) = run(&[
        "unitypackage",
        "extract",
        &pkg,
        "-o",
        proj.to_str().unwrap(),
        "--force",
    ]);
    assert!(ok, "extracting {pkg} should succeed");
    assert!(proj.exists(), "extraction produced a project tree");

    // 2. Real FBX ground truth: every FBX in the tree must parse + describe without error. This is
    //    the path that otherwise only ran behind AVATAR_SAMPLE_FBX. We don't require any particular
    //    rig shape (package contents vary), only that the parser handles real-world bytes; we log
    //    whether a humanoid avatar was among them.
    let fbxs = find_fbx(&proj, 16);
    assert!(
        !fbxs.is_empty(),
        "a real avatar package contains at least one FBX"
    );
    let mut humanoid_seen = 0;
    for fbx in &fbxs {
        let (_, json) = run(&["describe", fbx.to_str().unwrap(), "--json"]);
        let v: Value = serde_json::from_str(&json).unwrap_or_else(|e| {
            panic!(
                "describe --json on {} was not JSON: {e}\n{json}",
                fbx.display()
            )
        });
        assert_eq!(v["target"], "fbx", "describe of an .fbx reports target=fbx");
        // The structure summary and at least one performance metric must be present.
        assert!(
            v["fbx"]["inspect"]["version"].is_number(),
            "inspect.version for {}",
            fbx.display()
        );
        assert!(
            v["fbx"]["performance"]["stats"]
                .as_array()
                .is_some_and(|s| !s.is_empty()),
            "performance.stats non-empty for {}",
            fbx.display()
        );
        if v["fbx"]["humanoid_ready"] == Value::Bool(true) {
            humanoid_seen += 1;
        }
    }
    eprintln!(
        "ground truth: parsed {} real FBX file(s), {humanoid_seen} humanoid-ready",
        fbxs.len()
    );

    // 3. Real project ground truth: lint + stats over the reconstructed tree must run and emit their
    //    documented JSON shape (an SDK2 avatar / world legitimately yields few or zero diagnostics —
    //    we assert the *contract*, not specific findings).
    let (_, lint_json) = run(&["lint", proj.to_str().unwrap(), "--json"]);
    let lint: Value =
        serde_json::from_str(&lint_json).expect("lint --json on real project is valid JSON");
    assert!(
        lint["diagnostics"].is_array(),
        "lint report carries a diagnostics array"
    );

    let (stats_ok, stats_json) = run(&["stats", proj.to_str().unwrap(), "--json"]);
    assert!(stats_ok, "stats over a real project exits 0");
    let stats: Value =
        serde_json::from_str(&stats_json).expect("stats --json on real project is valid JSON");
    assert!(
        stats.is_array(),
        "project stats is an array of per-avatar reports"
    );

    let _ = std::fs::remove_dir_all(&proj);
}
