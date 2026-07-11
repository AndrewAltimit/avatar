//! `avatar toggle` — generate a complete, self-consistent toggle bundle: On/Off `.anim` clips,
//! a two-state FX `.controller`, a `VRCExpressionParameters` asset, a `VRCExpressionsMenu` asset,
//! and `.meta` sidecars pinning the GUIDs the bundle's cross-references use.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_anim_gen::{ToggleSpec, ToggleTarget, generate_toggle};
use clap::Args;
use serde_json::json;

use crate::cmd::WriteGuard;

#[derive(Args, Debug)]
pub struct ToggleArgs {
    /// Bundle name — seeds every file name, fileID range, and pinned GUID (e.g. `Hat`).
    #[arg(long)]
    name: String,
    /// A GameObject to toggle, by hierarchy path (e.g. `Armature/Head/Hat`). Repeatable.
    #[arg(long = "toggle", value_name = "PATH")]
    toggles: Vec<String>,
    /// A blendshape to drive as `PATH:SHAPE:VALUE` (VALUE when on, 0 when off). Repeatable.
    #[arg(long = "blendshape", value_name = "PATH:SHAPE:VALUE")]
    blendshapes: Vec<String>,
    /// The Bool animator + expression parameter (defaults to the bundle name).
    #[arg(long)]
    param: Option<String>,
    /// Label on the generated menu control (defaults to the bundle name).
    #[arg(long)]
    menu_label: Option<String>,
    /// Don't persist the parameter across avatar loads (`saved: 0`).
    #[arg(long)]
    unsaved: bool,
    /// Start toggled on: the parameter defaults to 1 and the On state is the layer default.
    #[arg(long)]
    default_on: bool,
    /// Directory to write the bundle into (created if missing). Required unless --json/--dry-run.
    #[arg(short, long, value_name = "DIR")]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (files, GUIDs, wiring note, contents) on stdout.
    /// With no `-o`, the bundle exists only inside the report.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

pub fn toggle(args: &ToggleArgs) -> Result<()> {
    let mut targets: Vec<ToggleTarget> = args
        .toggles
        .iter()
        .map(|path| ToggleTarget::GameObject { path: path.clone() })
        .collect();
    for spec in &args.blendshapes {
        let (path, shape, on_value) = super::anim_gen::parse_blendshape_spec(spec)?;
        targets.push(ToggleTarget::Blendshape {
            path,
            shape,
            on_value,
        });
    }
    if targets.is_empty() {
        bail!(
            "nothing to toggle: pass at least one --toggle PATH or --blendshape PATH:SHAPE:VALUE"
        );
    }
    if args.output.is_none() && !args.json && !args.guard.dry_run {
        bail!(
            "a toggle bundle is multiple files: pass -o DIR to write it, --json to embed it in a \
             report, or --dry-run to preview"
        );
    }

    let spec = ToggleSpec {
        name: args.name.clone(),
        parameter: args.param.clone().unwrap_or_else(|| args.name.clone()),
        targets,
        saved: !args.unsaved,
        default_on: args.default_on,
        menu_label: args.menu_label.clone().unwrap_or_else(|| args.name.clone()),
    };
    let bundle = generate_toggle(&spec);

    let mut written = Vec::new();
    if let Some(dir) = &args.output {
        if args.guard.dry_run {
            for f in &bundle.files {
                let path = dir.join(&f.file_name);
                let exists = if path.exists() {
                    " (would overwrite)"
                } else {
                    ""
                };
                eprintln!(
                    "dry run: would write {} byte(s) to {}{exists}",
                    f.content.len(),
                    path.display()
                );
            }
        } else {
            // Refuse-before-write across the whole bundle: either every file lands or none does.
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating output directory {}", dir.display()))?;
            if !args.guard.force
                && let Some(f) = bundle
                    .files
                    .iter()
                    .find(|f| dir.join(&f.file_name).exists())
            {
                bail!(
                    "refusing to overwrite existing file {} (pass --force to overwrite, or \
                     --dry-run to preview)",
                    dir.join(&f.file_name).display()
                );
            }
            for f in &bundle.files {
                let path = dir.join(&f.file_name);
                std::fs::write(&path, &f.content)
                    .with_context(|| format!("writing {}", path.display()))?;
                eprintln!("wrote {}", path.display());
                written.push(path.display().to_string());
            }
        }
    } else if args.guard.dry_run && !args.json {
        for f in &bundle.files {
            eprintln!(
                "dry run: would emit {} ({} byte(s))",
                f.file_name,
                f.content.len()
            );
        }
    }

    if args.json {
        let report = json!({
            "kind": "toggle",
            "name": args.name,
            "parameter": bundle.parameter,
            "sync_bits": bundle.sync_bits,
            "controller_guid": bundle.controller_guid,
            "params_guid": bundle.params_guid,
            "menu_guid": bundle.menu_guid,
            "wiring_note": bundle.wiring_note,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": written,
            "files": bundle.files,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eprintln!("note: {}", bundle.wiring_note);
    }
    Ok(())
}
