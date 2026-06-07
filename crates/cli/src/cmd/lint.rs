//! `avatar lint` — SDK3-compliance report over a Unity/VRChat project.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Args;

use crate::cmd::emit_report;

#[derive(Args, Debug)]
pub struct LintArgs {
    /// Path to a Unity project (or any path inside one).
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Exit non-zero if any warnings are found, not just errors (useful for gating CI).
    #[arg(long)]
    deny_warnings: bool,
    /// Write the report to this file instead of stdout (honors `--json`).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Lint a project. Returns a failure exit code when the report has errors (or warnings, with
/// `--deny-warnings`) so the command can gate CI.
pub fn lint(args: &LintArgs) -> Result<ExitCode> {
    let report = avatar_lint::run(&args.path)?;

    let text = if args.json {
        serde_json::to_string_pretty(&report)?
    } else {
        format_lint_report(&report)
    };
    emit_report(args.output.as_deref(), &text)?;

    Ok(lint_exit_code(&report, args.deny_warnings))
}

/// Render a lint report to a human-readable string (the body of `avatar lint`), including each
/// diagnostic's optional indented `hint:` line.
fn format_lint_report(report: &avatar_lint::LintReport) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(out, "Lint: {}", report.project_root);
    let _ = writeln!(
        out,
        "  Unity {}, avatar SDK {}",
        report.unity_version.as_deref().unwrap_or("(unknown)"),
        report.avatar_sdk_version.as_deref().unwrap_or("(absent)")
    );
    let _ = writeln!(
        out,
        "  {} parameter asset(s), {} menu asset(s), {} descriptor(s), {} controller(s), {} package(s)",
        report.parameter_assets,
        report.menu_assets,
        report.descriptors,
        report.controllers,
        report.packages.len()
    );

    if report.diagnostics.is_empty() {
        let _ = write!(out, "\n  No issues found.");
    } else {
        out.push('\n');
        for d in &report.diagnostics {
            let tag = match d.severity {
                avatar_lint::Severity::Error => "[X]",
                avatar_lint::Severity::Warn => "[!]",
                avatar_lint::Severity::Info => "[i]",
            };
            let _ = writeln!(out, "  {tag} {} {}", d.code, d.message);
            if let Some(file) = &d.file {
                let _ = writeln!(out, "        in {file}");
            }
            if let Some(hint) = &d.hint {
                let _ = writeln!(out, "        hint: {hint}");
            }
        }
        // Drop the trailing newline from the last diagnostic line.
        while out.ends_with('\n') {
            out.pop();
        }
    }

    let _ = write!(
        out,
        "\n\n  => {} error(s), {} warning(s)",
        report.error_count(),
        report.warn_count()
    );

    out
}

/// Failure when the report has errors, or — with `--deny-warnings` — any warnings.
fn lint_exit_code(report: &avatar_lint::LintReport, deny_warnings: bool) -> ExitCode {
    let failed = report.error_count() > 0 || (deny_warnings && report.warn_count() > 0);
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
