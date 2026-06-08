//! `avatar describe` — one consolidated snapshot of an avatar asset.
//!
//! The other read commands each answer one question (`fbx inspect`, `armature check`, `stats`,
//! `lint`). For an agent reasoning about an asset, running them all and stitching the output is
//! friction; `describe` runs the relevant set in one shot and emits a single report so one call
//! yields a full mental model:
//!
//!   * an **FBX** file  → structure summary + humanoid-armature analysis + geometry performance.
//!   * a **Unity project** (or any path inside one) → SDK3 lint report + per-avatar performance.
//!
//! Read-only; never writes. `--json` emits the machine-readable form (its shape is published via
//! `avatar schema describe`).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use avatar_armature::ArmatureReport;
use avatar_fbx::FbxScene;
use avatar_lint::LintReport;
use avatar_stats::{PerfReport, Platform};
use clap::Args;
use serde::Serialize;

use crate::cmd::fbx::{InspectSummary, inspect_summary};

#[derive(Args, Debug)]
pub struct DescribeArgs {
    /// Path to a binary FBX file, or a Unity project (or any path inside one).
    path: PathBuf,
    /// Emit the consolidated report as machine-readable JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
}

/// The consolidated snapshot. `target` discriminates the two shapes; exactly one of `fbx`/`project`
/// is populated to match.
#[derive(Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DescribeReport {
    /// The path that was described.
    pub path: String,
    /// `"fbx"` or `"project"`.
    pub target: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fbx: Option<FbxDescribe>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectDescribe>,
}

/// Everything `describe` knows about a single FBX file.
#[derive(Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FbxDescribe {
    pub inspect: InspectSummary,
    pub armature: ArmatureReport,
    /// Convenience mirror of `armature.is_humanoid_ready()` so consumers needn't recompute it.
    pub humanoid_ready: bool,
    pub performance: PerfReport,
}

/// Everything `describe` knows about a Unity/VRChat project.
#[derive(Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProjectDescribe {
    pub lint: LintReport,
    pub lint_errors: usize,
    pub lint_warnings: usize,
    /// One performance report per avatar (VRC Avatar Descriptor) found under `Assets/`.
    pub avatars: Vec<PerfReport>,
}

/// True when `path` is a file with an `.fbx` extension (mirrors `stats`' dispatch).
fn is_fbx(path: &std::path::Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
}

/// Build a describe report for `path`. Returns a failure exit code only when an FBX rig is not
/// humanoid-ready or a project lint found errors, so `describe` can also gate CI; informational
/// otherwise.
pub fn describe(args: &DescribeArgs) -> Result<ExitCode> {
    let report = build(&args.path)?;
    let code = exit_code(&report);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", format_human(&report));
    }
    Ok(code)
}

/// Build the consolidated describe report for `path`. Public so the MCP server (`avatar mcp serve`)
/// can return the same report the `describe --json` CLI emits.
pub fn build(path: &std::path::Path) -> Result<DescribeReport> {
    if is_fbx(path) {
        let scene = FbxScene::load(path)?;
        let armature = avatar_armature::analyze(&scene);
        let fbx = FbxDescribe {
            inspect: inspect_summary(&scene),
            humanoid_ready: armature.is_humanoid_ready(),
            armature,
            performance: avatar_stats::analyze_fbx(path)?,
        };
        Ok(DescribeReport {
            path: path.display().to_string(),
            target: "fbx",
            fbx: Some(fbx),
            project: None,
        })
    } else {
        let lint = avatar_lint::run(path)?;
        let project = ProjectDescribe {
            lint_errors: lint.error_count(),
            lint_warnings: lint.warn_count(),
            avatars: avatar_stats::analyze_project(path)?,
            lint,
        };
        Ok(DescribeReport {
            path: path.display().to_string(),
            target: "project",
            fbx: None,
            project: Some(project),
        })
    }
}

/// Failure when an FBX rig is missing a required humanoid bone, or a project lint reported errors.
fn exit_code(report: &DescribeReport) -> ExitCode {
    let ok = match (&report.fbx, &report.project) {
        (Some(f), _) => f.humanoid_ready,
        (_, Some(p)) => p.lint_errors == 0,
        _ => true,
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn format_human(report: &DescribeReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    if let Some(f) = &report.fbx {
        let _ = writeln!(out, "Describe (FBX): {}\n", report.path);
        let i = &f.inspect;
        let _ = writeln!(
            out,
            "  Structure  : FBX v{}, {} objects ({} models, {} geometries, {} materials, {} bone-like)",
            i.version, i.total_objects, i.models, i.geometries, i.materials, i.bone_like
        );

        let a = &f.armature;
        let _ = writeln!(
            out,
            "  Armature   : {} → {} humanoid bone(s) mapped, {} required missing, {} recommended missing",
            if f.humanoid_ready {
                "humanoid-ready"
            } else {
                "NOT humanoid-ready"
            },
            a.mapped.len(),
            a.missing_required.len(),
            a.missing_recommended.len(),
        );
        if !a.missing_required.is_empty() {
            let _ = writeln!(
                out,
                "               missing required: {}",
                a.missing_required.join(", ")
            );
        }

        let _ = writeln!(
            out,
            "  Performance: PC {} / Android {}",
            f.performance.overall(Platform::Pc).label(),
            f.performance.overall(Platform::Android).label(),
        );
        for s in &f.performance.stats {
            let _ = writeln!(out, "    - {:<20} {}", s.name, s.value);
        }
        let _ = writeln!(
            out,
            "  (component-side metrics need a project, not an FBX: {})",
            f.performance.not_evaluated.join(", ")
        );
    } else if let Some(p) = &report.project {
        let l = &p.lint;
        let _ = writeln!(out, "Describe (project): {}\n", report.path);
        let _ = writeln!(
            out,
            "  Project    : Unity {}, SDK {}, {} package(s)",
            l.unity_version.as_deref().unwrap_or("(unknown)"),
            l.avatar_sdk_version.as_deref().unwrap_or("(absent)"),
            l.packages.len(),
        );
        let _ = writeln!(
            out,
            "  Lint       : {} error(s), {} warning(s) across {} descriptor(s), {} controller(s)",
            p.lint_errors, p.lint_warnings, l.descriptors, l.controllers,
        );
        // Surface just the errors inline; full detail lives in `avatar lint` / `--json`.
        for d in l
            .diagnostics
            .iter()
            .filter(|d| matches!(d.severity, avatar_lint::Severity::Error))
        {
            let _ = writeln!(out, "    [{}] {}", d.code, d.message);
        }

        if p.avatars.is_empty() {
            let _ = writeln!(
                out,
                "  Avatars    : (none found — no VRC Avatar Descriptor under Assets/)"
            );
        }
        for av in &p.avatars {
            let _ = writeln!(
                out,
                "  Avatar     : {}  → PC {} / Android {}",
                av.source,
                av.overall(Platform::Pc).label(),
                av.overall(Platform::Android).label(),
            );
        }
    }

    out
}
