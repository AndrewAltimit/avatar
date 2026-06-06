//! Run `avatar-stats` on an FBX file or a Unity project and print the performance rank.
//!
//! This shows the library path behind the `avatar stats` subcommand. The argument can be:
//!   - a binary `.fbx` — [`avatar_stats::analyze_fbx`] returns one geometry-side [`PerfReport`];
//!   - a Unity project (or any path inside one) — [`avatar_stats::analyze_project`] returns a
//!     component-side [`PerfReport`] per avatar found.
//!
//! Each report ranks every metric on both PC and Android; the overall rank is the worst measured
//! metric on that platform. Run it with:
//!
//! ```sh
//! cargo run -p avatar-cli --example perf_stats -- path/to/model.fbx
//! cargo run -p avatar-cli --example perf_stats -- path/to/UnityProject
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use avatar_stats::{PerfReport, Platform};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let path: PathBuf = std::env::args_os()
        .nth(1)
        .context("usage: perf_stats <fbx-or-project-path>")?
        .into();

    let is_fbx = path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("fbx"));

    if is_fbx {
        let report = avatar_stats::analyze_fbx(&path).context("analyzing the FBX")?;
        print_report(&report);
        return Ok(());
    }

    let reports = avatar_stats::analyze_project(&path).context("analyzing the project")?;
    if reports.is_empty() {
        println!("No avatars found (no VRC Avatar Descriptor in any prefab/scene under Assets/).");
    }
    for (i, report) in reports.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_report(report);
    }
    Ok(())
}

fn print_report(report: &PerfReport) {
    println!("{}  [{}]", report.source, report.kind);
    for s in &report.stats {
        let rank = |r: Option<avatar_stats::Rank>| r.map_or("-", |r| r.label());
        println!(
            "  {:<24} pc={:<10} android={:<10}",
            s.name,
            rank(s.pc),
            rank(s.android),
        );
    }
    println!(
        "  => overall: PC {}, Android {}",
        report.overall(Platform::Pc).label(),
        report.overall(Platform::Android).label(),
    );
    if !report.not_evaluated.is_empty() {
        println!("  not evaluated here: {}", report.not_evaluated.join(", "));
    }
}
