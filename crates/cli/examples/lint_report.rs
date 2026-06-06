//! Run `avatar-lint` on a Unity/VRChat project and pretty-print the report.
//!
//! This shows the library path the `avatar lint` subcommand uses: [`avatar_lint::run`] takes a path
//! (the project root, or any path inside it) and returns a [`avatar_lint::LintReport`] you can
//! inspect or serialize. The CLI wraps this; here we walk the diagnostics by hand.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p avatar-cli --example lint_report -- path/to/UnityProject
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use avatar_lint::Severity;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let path: PathBuf = std::env::args_os()
        .nth(1)
        .context("usage: lint_report <unity-project-path>")?
        .into();

    let report = avatar_lint::run(&path).context("linting the project")?;

    println!("Project: {}", report.project_root);
    println!(
        "  Unity {}, avatar SDK {}",
        report.unity_version.as_deref().unwrap_or("(unknown)"),
        report.avatar_sdk_version.as_deref().unwrap_or("(absent)")
    );
    println!(
        "  {} parameter asset(s), {} menu asset(s), {} descriptor(s), {} controller(s)",
        report.parameter_assets, report.menu_assets, report.descriptors, report.controllers,
    );

    if report.diagnostics.is_empty() {
        println!("\n  No issues found.");
    } else {
        println!();
        for d in &report.diagnostics {
            let tag = match d.severity {
                Severity::Error => "[X]",
                Severity::Warn => "[!]",
                Severity::Info => "[i]",
            };
            println!("  {tag} {} {}", d.code, d.message);
            if let Some(file) = &d.file {
                println!("        in {file}");
            }
            if let Some(hint) = &d.hint {
                println!("        hint: {hint}");
            }
        }
    }

    println!(
        "\n  => {} error(s), {} warning(s)",
        report.error_count(),
        report.warn_count()
    );

    // Mirror the CLI: fail when the report contains errors.
    if report.error_count() > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
