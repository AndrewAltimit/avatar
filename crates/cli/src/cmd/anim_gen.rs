//! `avatar anim-gen` — generate Unity animation assets (`.anim` clips, FX analog-gesture
//! blend trees).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_anim_gen::{AnimationClip, BlendTree, Emitter, FloatCurve, IdGen, Keyframe};
use clap::{Args, Subcommand};

use crate::cmd::write_out;

#[derive(Subcommand, Debug)]
pub enum AnimGenCommand {
    /// Emit a 1D analog-gesture blend tree (blends `GestureLeftWeight`/`…Right` over child clips).
    Blendtree(BlendtreeArgs),
    /// Emit a `.anim` clip from blendshape and/or GameObject-active (toggle) curves.
    Clip(ClipArgs),
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
    /// Write the generated YAML here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
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
    /// Write the generated `.anim` YAML here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

/// Generate an analog-gesture blend tree. By default emits a self-contained
/// `AnimatorStateMachine` + `AnimatorState` + `BlendTree` fragment (splice into an FX `.controller`
/// and point a layer's `m_StateMachine` at the printed state-machine fileID); `--tree-only` emits
/// just the `BlendTree` document for grafting onto an existing Fist state.
pub fn blendtree(args: &BlendtreeArgs) -> Result<()> {
    let mut tree = BlendTree::analog_gesture(&args.name, &args.parameter);
    for spec in &args.clips {
        let (guid, threshold) = parse_clip_spec(spec)?;
        tree = tree.clip(guid, threshold);
    }

    let mut ids = IdGen::new(&args.name);
    let yaml = if args.tree_only {
        let tree_id = ids.alloc();
        let mut e = Emitter::new();
        tree.emit_tree(&mut e, tree_id);
        eprintln!("note: {}", tree.wiring_note(tree_id));
        format!(
            "{}{}",
            avatar_anim_gen::yaml_emit::UNITY_PREAMBLE,
            e.into_string()
        )
    } else {
        let (fragment, sm_id) = tree.to_state_fragment(&mut ids);
        eprintln!(
            "note: paste these documents into your FX `.controller` and point a layer's \
             `m_StateMachine` at {{fileID: {sm_id}}}; declare the float parameter `{}` if absent.",
            args.parameter
        );
        format!("{}{}", avatar_anim_gen::yaml_emit::UNITY_PREAMBLE, fragment)
    };

    write_out(args.output.as_deref(), &yaml)
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

    let mut clip = AnimationClip::new(&args.name);
    for spec in &args.blendshapes {
        let (path, shape, value) = parse_blendshape_spec(spec)?;
        clip.add_float_curve(FloatCurve::blendshape(
            path,
            &shape,
            vec![Keyframe::flat(0.0, value)],
        ));
    }
    for path in &args.toggles {
        clip.add_float_curve(FloatCurve::game_object_active(
            path.clone(),
            vec![Keyframe::flat(0.0, 1.0)],
        ));
    }

    let mut ids = IdGen::new(&args.name);
    let yaml = clip.to_unity_yaml(ids.alloc());
    write_out(args.output.as_deref(), &yaml)
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
