//! `avatar fbx` and `avatar armature` — inspect FBX structure and validate/repair the armature.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use avatar_armature::{RepairPlan, apply_plan, plan_repairs};
use avatar_fbx::{FbxDocument, FbxScene};
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum FbxCommand {
    /// Print the structure of an FBX file (objects, hierarchy, global settings).
    Inspect(FileArgs),
}

#[derive(Subcommand, Debug)]
pub enum ArmatureCommand {
    /// Validate the skeleton against VRChat humanoid rig requirements.
    Check(FileArgs),
    /// Plan (and optionally write) repairs: canonical humanoid bone names + parent topology.
    Fix(FixArgs),
}

#[derive(Args, Debug)]
pub struct FixArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// Write the repaired FBX here. Without this, runs as a dry run (prints the plan only).
    #[arg(short, long)]
    output: Option<std::path::PathBuf>,
    /// Allow `--output` to overwrite the input file or any existing output file.
    #[arg(long)]
    force: bool,
    /// Emit a machine-readable JSON plan instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Also write a headless-Blender Python script here that applies the WHOLE plan — including
    /// the flagged geometry repairs (reparents, scale/orientation) this tool won't apply natively.
    /// Run it with `blender --background --python <script>`.
    #[arg(long, value_name = "SCRIPT.py")]
    blender_script: Option<std::path::PathBuf>,
    /// Output FBX path the Blender script exports to (default: `<input>.fixed.fbx`).
    #[arg(long, value_name = "OUT.fbx", requires = "blender_script")]
    blender_output: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct FileArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

pub fn inspect(args: &FileArgs) -> Result<()> {
    let scene = FbxScene::load(&args.path)?;

    if args.json {
        let summary = inspect_summary(&scene);
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("FBX: {}", args.path.display());
    println!("  format version : {}", scene.version);

    let gs = &scene.global_settings;
    match gs.unit_scale_factor {
        Some(s) if (s - 100.0).abs() < 0.001 => {
            println!("  unit scale     : {s} (cm — matches Unity's 1:1 expectation)")
        }
        Some(s) => println!(
            "  unit scale     : {s}  [!] Unity imports cm (100.0); other values often scale the avatar wrong"
        ),
        None => println!("  unit scale     : (unspecified)"),
    }
    match gs.up_axis {
        Some(1) => println!("  up axis        : Y (matches Unity)"),
        Some(a) => println!("  up axis        : {a}  [!] Unity expects Y-up (1)"),
        None => println!("  up axis        : (unspecified)"),
    }

    let counts = ObjectCounts::of(&scene);
    println!(
        "  objects        : {} total ({} models, {} geometries, {} materials, {} bone-like)",
        counts.total, counts.models, counts.geometries, counts.materials, counts.bone_like
    );

    println!("\nModel hierarchy:");
    print_model_tree(&scene);

    Ok(())
}

/// Validate an armature. Returns a failure exit code when the rig is not humanoid-ready (a
/// Unity-required bone is missing), so the command can gate CI.
pub fn armature_check(args: &FileArgs) -> Result<ExitCode> {
    let scene = FbxScene::load(&args.path)?;
    let report = avatar_armature::analyze(&scene);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(armature_exit_code(&report));
    }

    println!("Armature check: {}", args.path.display());
    println!(
        "  {} models, {} bone-like, armature root(s): {}",
        report.total_models,
        report.bone_like_count,
        if report.armature_roots.is_empty() {
            "(none)".to_string()
        } else {
            report.armature_roots.join(", ")
        }
    );
    if !report.mesh_roots.is_empty() {
        println!("  mesh/other roots: {}", report.mesh_roots.join(", "));
    }

    if report.armature_roots.len() > 1 {
        println!(
            "  [!] {} armature roots — VRChat expects a single armature root",
            report.armature_roots.len()
        );
    }

    println!("\n  Mapped humanoid bones ({}):", report.mapped.len());
    for (bone, sources) in &report.mapped {
        println!("    {bone:<16} <- {}", sources.join(", "));
    }

    if !report.duplicate_mappings.is_empty() {
        println!("\n  [!] Duplicate mappings (one slot, multiple bones):");
        for (bone, sources) in &report.duplicate_mappings {
            println!("    {bone:<16} <- {}", sources.join(", "));
        }
    }

    if !report.missing_required.is_empty() {
        println!("\n  [X] Missing REQUIRED bones (avatar will not import as humanoid):");
        for b in &report.missing_required {
            println!("    - {b}");
        }
    }
    if !report.missing_recommended.is_empty() {
        println!("\n  [!] Missing recommended bones (VRChat expects a full spine + shoulders):");
        for b in &report.missing_recommended {
            println!("    - {b}");
        }
    }

    if !report.unmapped_bones.is_empty() {
        println!(
            "\n  Unmapped bone-like nodes ({}): {}",
            report.unmapped_bones.len(),
            report.unmapped_bones.join(", ")
        );
    }

    if report.ignored_finger_bones > 0 || report.ignored_leaf_bones > 0 {
        println!(
            "\n  Ignored: {} finger bone(s), {} leaf '_End' bone(s) (not humanoid body bones)",
            report.ignored_finger_bones, report.ignored_leaf_bones
        );
    }

    println!();
    if report.is_humanoid_ready() {
        println!("  => OK: all Unity-required humanoid bones are present.");
    } else {
        println!(
            "  => NOT humanoid-ready: {} required bone(s) missing.",
            report.missing_required.len()
        );
    }

    Ok(armature_exit_code(&report))
}

/// Failure when the rig is missing a Unity-required humanoid bone. Missing *recommended* or
/// *optional* bones are surfaced in the report but do not fail the command.
fn armature_exit_code(report: &avatar_armature::ArmatureReport) -> ExitCode {
    if report.is_humanoid_ready() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

pub fn armature_fix(args: &FixArgs) -> Result<()> {
    let mut doc = FbxDocument::load(&args.path)?;
    let plan = plan_repairs(&doc.scene());

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print_plan(&args.path, &plan);
    }

    // The Blender-script route covers the whole plan (including the flagged geometry repairs),
    // so it is independent of -o: emitting a script is not a write to the FBX.
    if let Some(script_path) = &args.blender_script {
        let default_out = args.path.with_extension("fixed.fbx");
        let blender_out = args.blender_output.as_deref().unwrap_or(&default_out);
        match avatar_armature::blender_script(
            &plan,
            &args.path.display().to_string(),
            &blender_out.display().to_string(),
        ) {
            Some(script) => {
                if script_path.exists() && !args.force {
                    bail!(
                        "refusing to overwrite existing file {} (pass --force to overwrite)",
                        script_path.display()
                    );
                }
                std::fs::write(script_path, script)
                    .with_context(|| format!("writing {}", script_path.display()))?;
                if !args.json {
                    println!(
                        "\n  wrote Blender repair script {} — run: blender --background --python {}",
                        script_path.display(),
                        script_path.display()
                    );
                }
            }
            None => {
                if !args.json {
                    println!("\n  no Blender script written — the plan has no repairs at all");
                }
            }
        }
    }

    match &args.output {
        None => {
            if !args.json && plan.native().count() > 0 {
                println!("\n  (dry run — pass -o <file> to write the repaired FBX)");
            }
        }
        Some(out) => {
            if !args.force {
                if overwrites_input(&args.path, out) {
                    bail!(
                        "refusing to overwrite the input file {}; choose a different -o path or pass --force",
                        args.path.display()
                    );
                }
                if out.exists() {
                    bail!(
                        "refusing to overwrite existing file {} (pass --force to overwrite)",
                        out.display()
                    );
                }
            }
            let applied = apply_plan(&mut doc, &plan)?;
            doc.write(out)?;
            if !args.json {
                println!("\n  applied {applied} edit(s); wrote {}", out.display());
                let flagged = plan.flagged().count();
                if flagged > 0 {
                    println!(
                        "  note: {flagged} flagged item(s) were NOT applied (need a geometry transform — see above)"
                    );
                }
            }
        }
    }

    Ok(())
}

/// True if writing to `out` would clobber the input `path` (same file on disk, or identical path).
fn overwrites_input(path: &Path, out: &Path) -> bool {
    match (path.canonicalize(), out.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => path == out,
    }
}

fn print_plan(path: &Path, plan: &RepairPlan) {
    println!("Armature fix: {}", path.display());
    if plan.is_empty() {
        println!("  No repairs needed — the armature already looks Unity-ready.");
        return;
    }

    let native: Vec<_> = plan.native().collect();
    if !native.is_empty() {
        println!("\n  {} repair(s) to apply:", native.len());
        for e in native {
            println!("    {}", e.summary());
        }
    }

    let flagged: Vec<_> = plan.flagged().collect();
    if !flagged.is_empty() {
        println!(
            "\n  {} item(s) flagged (not auto-applied — these need a geometry transform / Blender, \
             not a metadata relabel):",
            flagged.len()
        );
        for e in flagged {
            println!("    {}", e.summary());
        }
    }
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub(crate) struct InspectSummary {
    pub(crate) version: u32,
    pub(crate) unit_scale_factor: Option<f64>,
    pub(crate) up_axis: Option<i32>,
    pub(crate) total_objects: usize,
    pub(crate) models: usize,
    pub(crate) geometries: usize,
    pub(crate) materials: usize,
    pub(crate) bone_like: usize,
    pub(crate) roots: Vec<String>,
}

pub(crate) fn inspect_summary(scene: &FbxScene) -> InspectSummary {
    let counts = ObjectCounts::of(scene);
    let roots = model_roots(scene)
        .into_iter()
        .map(|id| scene.object(id).map(|o| o.name.clone()).unwrap_or_default())
        .collect();
    InspectSummary {
        version: scene.version,
        unit_scale_factor: scene.global_settings.unit_scale_factor,
        up_axis: scene.global_settings.up_axis,
        total_objects: counts.total,
        models: counts.models,
        geometries: counts.geometries,
        materials: counts.materials,
        bone_like: counts.bone_like,
        roots,
    }
}

struct ObjectCounts {
    total: usize,
    models: usize,
    geometries: usize,
    materials: usize,
    bone_like: usize,
}

impl ObjectCounts {
    fn of(scene: &FbxScene) -> Self {
        ObjectCounts {
            total: scene.objects.len(),
            models: scene
                .objects
                .iter()
                .filter(|o| o.node_name == "Model")
                .count(),
            geometries: scene
                .objects
                .iter()
                .filter(|o| o.node_name == "Geometry")
                .count(),
            materials: scene
                .objects
                .iter()
                .filter(|o| o.node_name == "Material")
                .count(),
            bone_like: scene.objects.iter().filter(|o| o.is_bone_like()).count(),
        }
    }
}

/// Ids of `Model` objects whose parent is not itself a model (the hierarchy roots).
fn model_roots(scene: &FbxScene) -> Vec<i64> {
    let model_ids: HashSet<i64> = scene.models().map(|m| m.id).collect();
    scene
        .models()
        .filter(|m| match scene.parent_of(m.id) {
            Some(pid) => !model_ids.contains(&pid),
            None => true,
        })
        .map(|m| m.id)
        .collect()
}

fn print_model_tree(scene: &FbxScene) {
    let model_ids: HashSet<i64> = scene.models().map(|m| m.id).collect();
    let mut visited = HashSet::new();
    for root in model_roots(scene) {
        print_model_node(scene, root, &model_ids, 0, &mut visited);
    }
}

fn print_model_node(
    scene: &FbxScene,
    id: i64,
    model_ids: &HashSet<i64>,
    depth: usize,
    visited: &mut HashSet<i64>,
) {
    // Guard against malformed cyclic parent links so a bad FBX can't drive infinite recursion.
    if !visited.insert(id) {
        return;
    }
    if let Some(obj) = scene.object(id) {
        let tag = if obj.subclass.is_empty() {
            String::new()
        } else {
            format!(" [{}]", obj.subclass)
        };
        let label = if obj.name.is_empty() {
            "(unnamed)"
        } else {
            &obj.name
        };
        println!("{}{}{}", "  ".repeat(depth + 1), label, tag);
    }
    // Recurse into model children only, in file order.
    for child in scene.children_of(id) {
        if model_ids.contains(&child) {
            print_model_node(scene, child, model_ids, depth + 1, visited);
        }
    }
}
