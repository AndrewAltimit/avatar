//! `avatar anim-gen` — generate Unity animation assets (`.anim` clips, FX analog-gesture
//! blend trees).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_anim_gen::{
    AnimationClip, BlendTree, Emitter, FloatCurve, IdGen, Keyframe, fx_blend_tree,
};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cmd::{WriteGuard, write_out_guarded};

#[derive(Subcommand, Debug)]
pub enum AnimGenCommand {
    /// Emit a 1D analog-gesture blend tree (blends `GestureLeftWeight`/`…Right` over child clips).
    Blendtree(BlendtreeArgs),
    /// Emit a `.anim` clip from blendshape and/or GameObject-active (toggle) curves.
    Clip(ClipArgs),
    /// Emit a complete, Unity-importable FX `AnimatorController` (class 91) wrapping an analog-
    /// gesture blend tree in a single layer — the full asset, not the splice-in fragment.
    Controller(ControllerArgs),
}

#[derive(Args, Debug)]
pub struct BlendtreeArgs {
    /// Name for the generated blend tree / state.
    #[arg(long, default_value = "GestureBlend")]
    name: String,
    /// The analog blend parameter VRChat populates from the trigger.
    #[arg(long, default_value = "GestureLeftWeight")]
    parameter: String,
    /// A child clip as `GUID@THRESHOLD` (e.g. `1a2b…@0.0`). Repeatable; order is free.
    #[arg(long = "clip", value_name = "GUID@THRESHOLD")]
    clips: Vec<String>,
    /// Emit only the `BlendTree` (class 206) document, not the surrounding state machine + state.
    #[arg(long)]
    tree_only: bool,
    /// Write the generated YAML asset here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (allocated fileIDs, wiring note, the YAML) on stdout
    /// instead of the raw YAML. `-o` still controls where the YAML *asset* is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

#[derive(Args, Debug)]
pub struct ClipArgs {
    /// Name for the generated clip.
    #[arg(long)]
    name: String,
    /// A blendshape curve as `PATH:SHAPE:VALUE` (e.g. `Body:Smile:100`). Repeatable.
    #[arg(long = "blendshape", value_name = "PATH:SHAPE:VALUE")]
    blendshapes: Vec<String>,
    /// A GameObject active-toggle curve, held on, by hierarchy `PATH` (e.g. `Armature/Hat`).
    /// Repeatable.
    #[arg(long = "toggle", value_name = "PATH")]
    toggles: Vec<String>,
    /// Write the generated `.anim` YAML asset here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (allocated fileID, curves, the YAML) on stdout instead
    /// of the raw YAML. `-o` still controls where the YAML *asset* is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

#[derive(Args, Debug)]
pub struct ControllerArgs {
    /// Name for the generated controller.
    #[arg(long, default_value = "FX")]
    name: String,
    /// Name for the controller's single animator layer.
    #[arg(long, default_value = "Base Layer")]
    layer: String,
    /// The analog blend parameter (auto-declared as a Float on the controller).
    #[arg(long, default_value = "GestureLeftWeight")]
    parameter: String,
    /// A child clip as `GUID@THRESHOLD` (e.g. `1a2b…@0.0`). Repeatable; order is free.
    #[arg(long = "clip", value_name = "GUID@THRESHOLD")]
    clips: Vec<String>,
    /// Write the generated `.controller` YAML asset here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (the YAML + metadata) on stdout instead of raw YAML.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

/// Generate a complete FX `AnimatorController` wrapping an analog-gesture blend tree. Unlike
/// `blendtree` (which emits a fragment to splice into an existing controller), this emits the whole
/// class-91 asset — importable into Unity on its own. This is the asset the Unity-acceptance job
/// imports to validate the generator against a real editor.
pub fn controller(args: &ControllerArgs) -> Result<()> {
    let clips: Vec<(String, f32)> = args
        .clips
        .iter()
        .map(|spec| parse_clip_spec(spec))
        .collect::<Result<_>>()?;

    let mut tree = BlendTree::analog_gesture(&args.name, &args.parameter);
    for (guid, threshold) in &clips {
        tree = tree.clip(guid.clone(), *threshold);
    }

    let mut ids = IdGen::new(&args.name);
    let yaml = fx_blend_tree(&args.name, &args.layer, &tree, &mut ids);

    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &yaml, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "controller",
            "name": args.name,
            "layer": args.layer,
            "parameter": args.parameter,
            "clips": clips.iter().map(|(g, t)| json!({"guid": g, "threshold": t})).collect::<Vec<_>>(),
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "yaml": yaml,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    Ok(())
}

/// Generate an analog-gesture blend tree. By default emits a self-contained
/// `AnimatorStateMachine` + `AnimatorState` + `BlendTree` fragment (splice into an FX `.controller`
/// and point a layer's `m_StateMachine` at the printed state-machine fileID); `--tree-only` emits
/// just the `BlendTree` document for grafting onto an existing Fist state.
pub fn blendtree(args: &BlendtreeArgs) -> Result<()> {
    // Parse the clip specs up front so they can be reported structurally under `--json`.
    let clips: Vec<(String, f32)> = args
        .clips
        .iter()
        .map(|spec| parse_clip_spec(spec))
        .collect::<Result<_>>()?;

    let mut tree = BlendTree::analog_gesture(&args.name, &args.parameter);
    for (guid, threshold) in &clips {
        tree = tree.clip(guid.clone(), *threshold);
    }

    let mut ids = IdGen::new(&args.name);
    // `wiring` is the fileID an agent must point a layer at; previously only an stderr note.
    let (yaml, wiring_key, wiring_id, note) = if args.tree_only {
        let tree_id = ids.alloc();
        let mut e = Emitter::new();
        tree.emit_tree(&mut e, tree_id);
        let yaml = format!(
            "{}{}",
            avatar_anim_gen::yaml_emit::UNITY_PREAMBLE,
            e.into_string()
        );
        (
            yaml,
            "blend_tree_file_id",
            tree_id,
            tree.wiring_note(tree_id),
        )
    } else {
        let (fragment, sm_id) = tree.to_state_fragment(&mut ids);
        let note = format!(
            "paste these documents into your FX `.controller` and point a layer's `m_StateMachine` \
             at {{fileID: {sm_id}}}; declare the float parameter `{}` if absent.",
            args.parameter
        );
        let yaml = format!("{}{}", avatar_anim_gen::yaml_emit::UNITY_PREAMBLE, fragment);
        (yaml, "state_machine_file_id", sm_id, note)
    };

    // In `--json` mode the YAML is carried inside the report, so only emit it separately when a
    // file target is given (`-o`); never duplicate it onto stdout alongside the JSON.
    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &yaml, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "blendtree",
            "name": args.name,
            "parameter": args.parameter,
            "tree_only": args.tree_only,
            "clips": clips.iter().map(|(g, t)| json!({"guid": g, "threshold": t})).collect::<Vec<_>>(),
            wiring_key: wiring_id,
            "wiring_note": note,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "yaml": yaml,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        // YAML is on stdout (or a file); the wiring note always goes to stderr so it never
        // contaminates a piped asset.
        eprintln!("note: {note}");
    }

    Ok(())
}

/// Generate a `.anim` clip from `--blendshape PATH:SHAPE:VALUE` and/or `--toggle PATH` curves. Each
/// curve is a single held keyframe (a static "pose"/"on" clip), the common case for FX expressions
/// and toggles.
pub fn clip(args: &ClipArgs) -> Result<()> {
    if args.blendshapes.is_empty() && args.toggles.is_empty() {
        bail!(
            "nothing to generate: pass at least one --blendshape PATH:SHAPE:VALUE or --toggle PATH"
        );
    }

    let blendshapes: Vec<(String, String, f32)> = args
        .blendshapes
        .iter()
        .map(|spec| parse_blendshape_spec(spec))
        .collect::<Result<_>>()?;

    let mut clip = AnimationClip::new(&args.name);
    for (path, shape, value) in &blendshapes {
        clip.add_float_curve(FloatCurve::blendshape(
            path.clone(),
            shape,
            vec![Keyframe::flat(0.0, *value)],
        ));
    }
    for path in &args.toggles {
        clip.add_float_curve(FloatCurve::game_object_active(
            path.clone(),
            vec![Keyframe::flat(0.0, 1.0)],
        ));
    }

    let mut ids = IdGen::new(&args.name);
    let clip_id = ids.alloc();
    let yaml = clip.to_unity_yaml(clip_id);
    // See `blendtree`: under `--json` the YAML lives in the report; only emit separately for `-o`.
    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &yaml, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "clip",
            "name": args.name,
            "clip_file_id": clip_id,
            "blendshapes": blendshapes
                .iter()
                .map(|(p, s, v)| json!({"path": p, "shape": s, "value": v}))
                .collect::<Vec<_>>(),
            "toggles": args.toggles,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "yaml": yaml,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }

    Ok(())
}

/// Parse a `GUID@THRESHOLD` child-clip spec. The guid is hex (no `@`), so split on the last `@`.
fn parse_clip_spec(spec: &str) -> Result<(String, f32)> {
    let (guid, thr) = spec
        .rsplit_once('@')
        .with_context(|| format!("clip '{spec}' must be GUID@THRESHOLD, e.g. 1a2b...@0.0"))?;
    let threshold: f32 = thr
        .parse()
        .with_context(|| format!("threshold '{thr}' in clip '{spec}' is not a number"))?;
    if guid.is_empty() {
        bail!("clip '{spec}' has an empty guid");
    }
    Ok((guid.to_string(), threshold))
}

/// Parse a `PATH:SHAPE:VALUE` blendshape spec. The hierarchy path uses `/` (never `:`), so peel the
/// value then the shape off the right.
fn parse_blendshape_spec(spec: &str) -> Result<(String, String, f32)> {
    let fail = || format!("blendshape '{spec}' must be PATH:SHAPE:VALUE, e.g. Body:Smile:100");
    let (left, value_str) = spec.rsplit_once(':').with_context(fail)?;
    let (path, shape) = left.rsplit_once(':').with_context(fail)?;
    if path.is_empty() || shape.is_empty() {
        bail!("{}", fail());
    }
    let value: f32 = value_str
        .parse()
        .with_context(|| format!("value '{value_str}' in blendshape '{spec}' is not a number"))?;
    Ok((path.to_string(), shape.to_string(), value))
}
