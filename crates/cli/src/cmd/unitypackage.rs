//! `avatar unitypackage` — read/extract/cross-check `.unitypackage` archives (avatars, worlds/maps).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum UnitypackageCommand {
    /// Summarize a package: contents, detected SDK, and whether it looks like an avatar or a world.
    Info(UpInfoArgs),
    /// List the assets in a package (path, guid, size).
    List(UpListArgs),
    /// Extract a package into a Unity `Assets/` tree (asset bytes + `.meta` sidecars).
    Extract(UpExtractArgs),
    /// Test an avatar package against a world/map package: report co-import GUID/path conflicts.
    Testbed(UpTestbedArgs),
}

#[derive(Args, Debug)]
pub struct UpInfoArgs {
    /// Path to a `.unitypackage` file.
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct UpListArgs {
    /// Path to a `.unitypackage` file.
    path: PathBuf,
    /// Only list assets whose path contains this substring (case-insensitive).
    #[arg(long)]
    filter: Option<String>,
    /// Include folder entries (default: files only).
    #[arg(long)]
    folders: bool,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct UpExtractArgs {
    /// Path to a `.unitypackage` file.
    path: PathBuf,
    /// Destination directory (created if missing). The project tree is written under it.
    #[arg(short, long)]
    output: PathBuf,
    /// Extract into a non-empty destination anyway (otherwise the extraction is refused so an
    /// existing project tree is never silently merged into / clobbered).
    #[arg(long)]
    force: bool,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct UpTestbedArgs {
    /// The avatar `.unitypackage` to test.
    avatar: PathBuf,
    /// The world/map `.unitypackage` to drop it into.
    world: PathBuf,
    /// Exit non-zero if any conflicting (different-bytes) GUID or path collision is found.
    #[arg(long)]
    strict: bool,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

fn open_package(path: &Path) -> Result<avatar_unitypackage::UnityPackage> {
    avatar_unitypackage::UnityPackage::open(path)
}

/// Human-readable byte size (KB/MB/GB).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

fn sdk_label(sdk: Option<avatar_unitypackage::VrcSdk>) -> &'static str {
    use avatar_unitypackage::VrcSdk;
    match sdk {
        Some(VrcSdk::Sdk2) => "VRChat SDK2 (legacy)",
        Some(VrcSdk::Sdk3Avatars) => "VRChat SDK3 — Avatars",
        Some(VrcSdk::Sdk3Worlds) => "VRChat SDK3 — Worlds",
        Some(VrcSdk::Unknown) => "VRChat SDK (version unknown)",
        None => "none bundled",
    }
}

pub fn info(args: &UpInfoArgs) -> Result<()> {
    let pkg = open_package(&args.path)?;
    let summary = pkg.summary();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("Package: {}", args.path.display());
    println!(
        "  {} entries  ({} files, {} folders), {} of assets",
        summary.entry_count,
        summary.file_count,
        summary.folder_count,
        human_bytes(summary.total_asset_bytes)
    );
    let t = &summary.traits;
    let kind = if t.looks_like_avatar {
        "avatar"
    } else if t.looks_like_world {
        "world/map"
    } else {
        "assets"
    };
    println!("  Looks like: {kind}");
    println!("  Bundled SDK: {}", sdk_label(t.vrc_sdk));
    if let Some(v) = &t.sdk_version_txt {
        println!("  VRCSDK/version.txt: {v}");
    }
    println!("  {} prefab(s), {} scene(s)", t.prefab_count, t.scene_count);

    println!("\n  Top asset types:");
    let mut by_ext: Vec<_> = summary.by_extension.iter().collect();
    by_ext.sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes));
    for (ext, stat) in by_ext.into_iter().take(12) {
        println!(
            "    {:<14} {:>5} files  {:>10}",
            ext,
            stat.count,
            human_bytes(stat.bytes)
        );
    }
    Ok(())
}

pub fn list(args: &UpListArgs) -> Result<()> {
    let pkg = open_package(&args.path)?;
    let filter = args.filter.as_deref().map(str::to_ascii_lowercase);

    let mut rows: Vec<(&str, &str, u64, bool)> = Vec::new();
    for e in pkg.entries() {
        if !args.folders && !e.is_file() {
            continue;
        }
        let path = e.pathname.as_deref().unwrap_or("(no pathname)");
        if let Some(f) = &filter
            && !path.to_ascii_lowercase().contains(f)
        {
            continue;
        }
        rows.push((path, e.guid.as_str(), e.size(), e.is_file()));
    }
    rows.sort_by(|a, b| a.0.cmp(b.0));

    if args.json {
        let json: Vec<_> = rows
            .iter()
            .map(|(path, guid, size, is_file)| {
                serde_json::json!({"path": path, "guid": guid, "size": size, "file": is_file})
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    for (path, guid, size, is_file) in &rows {
        let tag = if *is_file { "" } else { "  [dir]" };
        println!("{guid}  {:>10}  {path}{tag}", human_bytes(*size));
    }
    println!(
        "\n{} entr{} listed",
        rows.len(),
        if rows.len() == 1 { "y" } else { "ies" }
    );
    Ok(())
}

pub fn extract(args: &UpExtractArgs) -> Result<()> {
    // Don't silently merge into an existing populated tree — that mixes two projects' assets.
    if !args.force
        && let Ok(mut entries) = std::fs::read_dir(&args.output)
        && entries.next().is_some()
    {
        anyhow::bail!(
            "destination {} is not empty (pass --force to extract into it anyway)",
            args.output.display()
        );
    }
    let pkg = open_package(&args.path)?;
    let report = pkg.extract(&args.output)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "Extracted {} into {}",
        args.path.display(),
        args.output.display()
    );
    println!(
        "  {} files ({}), {} .meta sidecars, {} folders",
        report.files_written,
        human_bytes(report.bytes_written),
        report.meta_written,
        report.folders_created
    );
    if report.skipped_no_pathname > 0 {
        println!(
            "  {} entr(ies) skipped (no pathname)",
            report.skipped_no_pathname
        );
    }
    if !report.skipped_unsafe.is_empty() {
        println!(
            "  {} entr(ies) skipped (absolute / non-project path, e.g. leaked editor DLLs):",
            report.skipped_unsafe.len()
        );
        for p in report.skipped_unsafe.iter().take(10) {
            println!("    {p}");
        }
    }
    println!(
        "\nNow runnable with the rest of the toolchain, e.g.:\n  avatar lint {0}\n  avatar stats {0}",
        args.output.display()
    );
    Ok(())
}

/// Test an avatar package against a world package: what happens when you import both into one
/// project to preview the avatar in the map. Returns a failure code under `--strict` if any
/// content-conflicting GUID or path collision exists.
pub fn testbed(args: &UpTestbedArgs) -> Result<ExitCode> {
    let avatar = open_package(&args.avatar)?;
    let world = open_package(&args.world)?;
    let av_sum = avatar.summary();
    let wd_sum = world.summary();
    let overlap = avatar.overlap(&world);

    if args.json {
        let out = serde_json::json!({
            "avatar": {"path": args.avatar.display().to_string(), "summary": av_sum},
            "world": {"path": args.world.display().to_string(), "summary": wd_sum},
            "overlap": overlap,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Testbed: avatar in world\n");
        println!(
            "  Avatar: {}\n    {} files, {}, {} | looks like {}",
            args.avatar.display(),
            av_sum.file_count,
            sdk_label(av_sum.traits.vrc_sdk),
            human_bytes(av_sum.total_asset_bytes),
            if av_sum.traits.looks_like_avatar {
                "an avatar"
            } else {
                "assets"
            }
        );
        println!(
            "  World:  {}\n    {} files, {}, {} | looks like {}",
            args.world.display(),
            wd_sum.file_count,
            sdk_label(wd_sum.traits.vrc_sdk),
            human_bytes(wd_sum.total_asset_bytes),
            if wd_sum.traits.looks_like_world {
                "a world/map"
            } else {
                "assets"
            }
        );

        if !av_sum.traits.looks_like_avatar {
            println!("\n  note: the first package doesn't look like an avatar.");
        }
        if !wd_sum.traits.looks_like_world {
            println!("  note: the second package doesn't look like a world/map.");
        }

        println!("\n  Co-import conflicts (importing the avatar into the world's project):");
        if overlap.is_clean() {
            println!(
                "    none — the two packages share no GUIDs or paths. Safe to import together."
            );
        } else {
            let conflicting = overlap.conflicting().count();
            let identical = overlap.guid_collisions.len() - conflicting;
            println!(
                "    {} shared GUID(s): {} with DIFFERENT content (one will be overwritten on import), {} identical (harmless)",
                overlap.guid_collisions.len(),
                conflicting,
                identical
            );
            println!(
                "    {} path collision(s) (same path, different GUID)",
                overlap.path_collisions.len()
            );

            for c in overlap.conflicting().take(15) {
                let path = c
                    .path_a
                    .as_deref()
                    .or(c.path_b.as_deref())
                    .unwrap_or("(unknown)");
                println!("      conflict  {}  {}", c.guid, path);
            }
            if conflicting > 15 {
                println!("      … and {} more", conflicting - 15);
            }
            for c in overlap.path_collisions.iter().take(10) {
                println!("      path      {}  ({} vs {})", c.path, c.guid_a, c.guid_b);
            }
        }
    }

    let has_conflict =
        overlap.conflicting().next().is_some() || !overlap.path_collisions.is_empty();
    if args.strict && has_conflict {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
