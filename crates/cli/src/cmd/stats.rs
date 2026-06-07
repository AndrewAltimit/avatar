//! `avatar stats` — estimate the VRChat performance ranking of an FBX or a project's avatar(s).

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::cmd::emit_report;

#[derive(Args, Debug)]
pub struct StatsArgs {
    /// Path to a binary FBX file, or a Unity project (or any path inside one).
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Write the report to this file instead of stdout (honors `--json`).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Report the VRChat performance ranking of an FBX file (geometry side) or every avatar in a Unity
/// project (component side). Informational — always exits 0 on success.
pub fn stats(args: &StatsArgs) -> Result<()> {
    let is_fbx = args.path.is_file()
        && args
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("fbx"));

    if is_fbx {
        let report = avatar_stats::analyze_fbx(&args.path)?;
        let text = if args.json {
            serde_json::to_string_pretty(&report)?
        } else {
            format_perf_report(&report)
        };
        return emit_report(args.output.as_deref(), &text);
    }

    let reports = avatar_stats::analyze_project(&args.path)?;
    if args.json {
        let text = serde_json::to_string_pretty(&reports)?;
        return emit_report(args.output.as_deref(), &text);
    }
    let mut out = String::new();
    if reports.is_empty() {
        out.push_str(
            "No avatars found (no VRC Avatar Descriptor in any prefab/scene under Assets/).",
        );
    }
    for (i, report) in reports.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format_perf_report(report));
    }
    emit_report(args.output.as_deref(), &out)
}

/// Render a performance report to a human-readable string (the body of `avatar stats`).
fn format_perf_report(report: &avatar_stats::PerfReport) -> String {
    use avatar_stats::Platform;
    use std::fmt::Write;

    let mut out = String::new();
    let kind = match report.kind {
        "fbx" => "FBX geometry",
        _ => "avatar components",
    };
    let _ = writeln!(out, "Performance: {}  [{kind}]\n", report.source);

    // A single metric row; a free macro (not a closure) so the separator `writeln!`s below can also
    // borrow `out` without a double-mutable-borrow conflict.
    macro_rules! row {
        ($name:expr, $value:expr, $pc:expr, $android:expr) => {
            let _ = writeln!(
                out,
                "  {:<30} {:>15}  {:<11} {:<11}",
                $name, $value, $pc, $android
            );
        };
    }
    row!("Metric", "Value", "PC", "Android");
    let _ = writeln!(out, "  {:-<30} {:->15}  {:-<11} {:-<11}", "", "", "", "");
    let mut shows_dual = false;
    for s in &report.stats {
        shows_dual |= s.value != s.android_value;
        row!(
            s.name,
            &metric_value(s),
            rank_label(s.pc),
            rank_label(s.android)
        );
    }
    let _ = writeln!(out, "  {:-<30} {:->15}  {:-<11} {:-<11}", "", "", "", "");
    row!(
        "Overall",
        "",
        report.overall(Platform::Pc).label(),
        report.overall(Platform::Android).label()
    );

    if shows_dual {
        let _ = write!(
            out,
            "\n  (Texture Memory value shown as PC/Android — textures recompress differently per platform.)"
        );
    }
    if !report.not_evaluated.is_empty() {
        let _ = write!(
            out,
            "\n  Not evaluated for this source (could lower the real rank):\n    {}",
            report.not_evaluated.join(", ")
        );
    }
    // Trim the trailing newline left by the last `row` so `emit_report` controls final spacing.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

/// A rank label, or a dash for a metric not ranked on that platform.
fn rank_label(rank: Option<avatar_stats::Rank>) -> &'static str {
    rank.map_or("-", |r| r.label())
}

/// Display a metric's value. Texture Memory is a byte count shown in MB — and as `PC/Android` when
/// the two platforms differ (the usual case); the rest are plain counts, identical across platforms.
fn metric_value(stat: &avatar_stats::MetricStat) -> String {
    if stat.metric == avatar_stats::Metric::TextureMemory {
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        if stat.value == stat.android_value {
            format!("{:.1} MB", mb(stat.value))
        } else {
            format!("{:.1}/{:.1} MB", mb(stat.value), mb(stat.android_value))
        }
    } else {
        stat.value.to_string()
    }
}
