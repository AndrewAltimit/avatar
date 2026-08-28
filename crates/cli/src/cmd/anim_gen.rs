//! `avatar anim-gen` — generate Unity animation assets (`.anim` clips, FX analog-gesture
//! blend trees).

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_anim_gen::{
    AnimationClip, BlendTree, Emitter, ExpressionParamSpec, ExpressionParams, ExpressionsMenu,
    FloatCurve, IdGen, Keyframe, MenuControlSpec, ObjectRef, fx_blend_tree,
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
    /// Emit a `VRCExpressionParameters` asset declaring expression parameters.
    Params(ParamsArgs),
    /// Emit a `VRCExpressionsMenu` asset from toggle/button/radial/sub-menu controls.
    Menu(MenuArgs),
    /// Graft a float-driven **radial puppet** into an existing avatar: splice a gated blend-tree
    /// layer into an FX `.controller` (Off state writes nothing; the tree takes over while the
    /// dial is up), declare the float parameter, and append it to existing
    /// `VRCExpressionParameters` / `VRCExpressionsMenu` assets — e.g. an analog "Blink" dial
    /// blending Neutral → an eyes-closed clip.
    Puppet(PuppetArgs),
}

#[derive(Args, Debug)]
pub struct PuppetArgs {
    /// The FX `.controller` to splice the layer + parameter into (edited in place; pass
    /// `--force`).
    #[arg(long)]
    controller: PathBuf,
    /// A `VRCExpressionParameters` asset to append the float parameter to (edited in place).
    #[arg(long)]
    parameters: Option<PathBuf>,
    /// A `VRCExpressionsMenu` asset to append the radial control to (edited in place).
    #[arg(long)]
    menu: Option<PathBuf>,
    /// The float parameter the dial drives (animator + expression parameter, same name).
    #[arg(long)]
    param: String,
    /// Menu control label (default: the parameter name).
    #[arg(long)]
    menu_name: Option<String>,
    /// FX layer name (default: `<param> Puppet`).
    #[arg(long)]
    layer_name: Option<String>,
    /// A child clip as `GUID@THRESHOLD` (e.g. the neutral clip `@0` and the target pose `@1`).
    /// Repeatable; order is free.
    #[arg(long = "clip", value_name = "GUID@THRESHOLD")]
    clips: Vec<String>,
    /// The layer engages when the parameter rises above this…
    #[arg(long, default_value_t = 0.01)]
    on: f32,
    /// …and disengages below this (the gap is hysteresis).
    #[arg(long, default_value_t = 0.005)]
    off: f32,
    /// Don't persist the parameter's value across worlds/restarts.
    #[arg(long)]
    unsaved: bool,
    /// Default parameter value.
    #[arg(long, default_value_t = 0.0)]
    default_value: f32,
    /// Emit a machine-readable JSON report.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

pub fn puppet(args: &PuppetArgs) -> Result<()> {
    use avatar_unity_yaml::{EditableUnityFile, parse_path};

    if args.clips.len() < 2 {
        bail!("a puppet blend needs at least two --clip GUID@THRESHOLD children (e.g. @0 and @1)");
    }
    let clips: Vec<(String, f32)> = args
        .clips
        .iter()
        .map(|spec| parse_clip_spec(spec))
        .collect::<Result<_>>()?;
    let menu_name = args.menu_name.clone().unwrap_or_else(|| args.param.clone());
    let layer_name = args
        .layer_name
        .clone()
        .unwrap_or_else(|| format!("{} Puppet", args.param));

    // ---- FX controller: parameter + layer entry + gated fragment documents.
    let text = std::fs::read_to_string(&args.controller)
        .with_context(|| format!("reading {}", args.controller.display()))?;
    let mut fx = EditableUnityFile::parse(&text)?;
    let root = fx
        .documents()
        .iter()
        .position(|d| d.class_id == 91)
        .context("no AnimatorController (class 91) document in --controller")?;
    let params_path = parse_path("m_AnimatorParameters");
    let existing = fx.sequence_items(root, &params_path).unwrap_or_default();
    if existing
        .iter()
        .any(|it| it.contains(&format!("m_Name: {}", args.param)))
    {
        bail!(
            "parameter '{}' is already declared in {}",
            args.param,
            args.controller.display()
        );
    }

    let mut tree = BlendTree::analog_gesture(&menu_name, &args.param);
    for (guid, threshold) in &clips {
        tree = tree.clip(guid.clone(), *threshold);
    }
    let mut ids = IdGen::new(&format!("{} puppet", args.param));
    let (fragment, sm_id) = tree.to_gated_layer_fragment(&args.param, args.on, args.off, &mut ids);

    // Split the fragment into its documents and refuse (astronomically unlikely) id collisions.
    let frag = EditableUnityFile::parse(&format!(
        "{}{}",
        avatar_anim_gen::yaml_emit::UNITY_PREAMBLE,
        fragment
    ))?;
    let frag_docs: Vec<(u32, i64, String)> = frag
        .documents()
        .iter()
        .map(|d| {
            (
                d.class_id,
                d.file_id,
                frag.text()[d.body_range()].to_string(),
            )
        })
        .collect();
    for (_, id, _) in &frag_docs {
        if fx.doc_by_file_id(*id).is_some() {
            bail!("generated fileID {id} collides with an existing document; rename --param");
        }
    }

    fx.append_sequence_item(
        root,
        &params_path,
        &format!(
            "m_Name: {}
m_Type: 1
m_DefaultFloat: 0
m_DefaultInt: 0
m_DefaultBool: 0
m_Controller: {{fileID: 0}}",
            args.param
        ),
    )?;
    fx.append_sequence_item(
        root,
        &parse_path("m_AnimatorLayers"),
        &format!(
            "serializedVersion: 5
m_Name: {layer_name}
m_StateMachine: {{fileID: {sm_id}}}
m_Mask: {{fileID: 0}}
m_Motions: []
m_Behaviours: []
m_BlendingMode: 0
m_SyncedLayerIndex: -1
m_DefaultWeight: 1
m_IKPass: 0
m_SyncedLayerAffectsTiming: 0
m_Controller: {{fileID: 0}}"
        ),
    )?;
    for (class_id, file_id, body) in &frag_docs {
        fx.append_document(*class_id, *file_id, body)?;
    }
    let mut written = vec![];
    if write_out_guarded(Some(&args.controller), &fx.into_string(), args.guard)? {
        written.push(args.controller.display().to_string());
    }

    // ---- Expression parameters asset.
    if let Some(path) = &args.parameters {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut f = EditableUnityFile::parse(&text)?;
        let doc = f
            .documents()
            .iter()
            .position(|d| d.class_id == 114)
            .context("no MonoBehaviour document in --parameters")?;
        f.append_sequence_item(
            doc,
            &parse_path("parameters"),
            &format!(
                "name: {}
valueType: 1
saved: {}
defaultValue: {}
networkSynced: 1",
                args.param,
                (!args.unsaved) as i32,
                args.default_value
            ),
        )?;
        if write_out_guarded(Some(path), &f.into_string(), args.guard)? {
            written.push(path.display().to_string());
        }
    }

    // ---- Expression menu asset.
    if let Some(path) = &args.menu {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut f = EditableUnityFile::parse(&text)?;
        let doc = f
            .documents()
            .iter()
            .position(|d| d.class_id == 114)
            .context("no MonoBehaviour document in --menu")?;
        f.append_sequence_item(
            doc,
            &parse_path("controls"),
            &format!(
                "name: {menu_name}
icon: {{fileID: 0}}
type: 203
parameter:
  name: ''
value: 1
subMenu: {{fileID: 0}}
subParameters:
- name: {}
labels: []",
                args.param
            ),
        )?;
        if write_out_guarded(Some(path), &f.into_string(), args.guard)? {
            written.push(path.display().to_string());
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "param": args.param,
                "layer": layer_name,
                "menu_control": menu_name,
                "state_machine_file_id": sm_id,
                "clips": clips.iter().map(|(g, t)| json!({"guid": g, "threshold": t})).collect::<Vec<_>>(),
                "gate": {"on": args.on, "off": args.off},
                "written": written,
            }))?
        );
    } else {
        eprintln!(
            "puppet '{menu_name}': float '{}' -> gated layer '{layer_name}' (sm {sm_id}); wrote {}",
            args.param,
            if written.is_empty() {
                "nothing (dry run?)".to_string()
            } else {
                written.join(", ")
            }
        );
    }
    Ok(())
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

#[derive(Args, Debug)]
pub struct ParamsArgs {
    /// Name for the generated asset (`m_Name`).
    #[arg(long, default_value = "Parameters")]
    name: String,
    /// A parameter as `NAME:TYPE[:DEFAULT][:unsaved][:local]` — TYPE is bool|int|float; `unsaved`
    /// clears the saved flag, `local` clears network sync. E.g. `Hat:bool`, `Dim:float:0.5:local`.
    /// Repeatable.
    #[arg(long = "param", value_name = "SPEC", required = true)]
    params: Vec<String>,
    /// Override the SDK `VRCExpressionParameters` script GUID (treated as a loose `.cs` script,
    /// fileID 11500000; the default is the class inside `VRCSDK3A.dll`).
    #[arg(long)]
    script_guid: Option<String>,
    /// Write the generated `.asset` YAML here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (parameters, sync bits, the YAML) on stdout instead
    /// of the raw YAML. `-o` still controls where the YAML *asset* is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

#[derive(Args, Debug)]
pub struct MenuArgs {
    /// Name for the generated asset (`m_Name`).
    #[arg(long, default_value = "Menu")]
    name: String,
    /// A toggle control as `LABEL:PARAM[:VALUE]`. Repeatable.
    #[arg(long = "toggle", value_name = "LABEL:PARAM[:VALUE]")]
    toggles: Vec<String>,
    /// A momentary button control as `LABEL:PARAM[:VALUE]`. Repeatable.
    #[arg(long = "button", value_name = "LABEL:PARAM[:VALUE]")]
    buttons: Vec<String>,
    /// A radial-puppet control as `LABEL:PARAM` (PARAM is the float axis). Repeatable.
    #[arg(long = "radial", value_name = "LABEL:PARAM")]
    radials: Vec<String>,
    /// A sub-menu control as `LABEL:GUID` (the child menu asset's guid). Repeatable.
    #[arg(long = "submenu", value_name = "LABEL:GUID")]
    submenus: Vec<String>,
    /// Override the SDK `VRCExpressionsMenu` script GUID (treated as a loose `.cs` script,
    /// fileID 11500000; the default is the class inside `VRCSDK3A.dll`).
    #[arg(long)]
    script_guid: Option<String>,
    /// Write the generated `.asset` YAML here instead of stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (controls, the YAML) on stdout instead of the raw
    /// YAML. `-o` still controls where the YAML *asset* is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

/// Generate a `VRCExpressionParameters` asset from `--param NAME:TYPE[...]` specs.
pub fn params(args: &ParamsArgs) -> Result<()> {
    let specs: Vec<ExpressionParamSpec> = args
        .params
        .iter()
        .map(|s| parse_param_spec(s))
        .collect::<Result<_>>()?;

    let mut asset = ExpressionParams::new(&args.name);
    if let Some(g) = &args.script_guid {
        asset = asset.script_guid(g);
    }
    for p in &specs {
        asset = asset.parameter(p.clone());
    }
    let sync_bits = asset.synced_bits();
    let yaml = asset.to_unity_yaml(avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID);

    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &yaml, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "params",
            "name": args.name,
            "parameters": specs.iter().map(|p| json!({
                "name": p.name,
                "type": match p.value_type {
                    avatar_anim_gen::ExpressionValueType::Int => "int",
                    avatar_anim_gen::ExpressionValueType::Float => "float",
                    avatar_anim_gen::ExpressionValueType::Bool => "bool",
                },
                "default": p.default_value,
                "saved": p.saved,
                "synced": p.synced,
            })).collect::<Vec<_>>(),
            "sync_bits": sync_bits,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "yaml": yaml,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

/// Generate a `VRCExpressionsMenu` asset from `--toggle`/`--button`/`--radial`/`--submenu`
/// controls. Controls are emitted grouped by kind, in flag order within each kind.
pub fn menu(args: &MenuArgs) -> Result<()> {
    let mut asset = ExpressionsMenu::new(&args.name);
    if let Some(g) = &args.script_guid {
        asset = asset.script_guid(g);
    }
    let mut described: Vec<serde_json::Value> = Vec::new();
    for spec in &args.toggles {
        let (label, param, value) = parse_control_spec(spec, "toggle")?;
        let mut c = MenuControlSpec::toggle(&label, &param);
        if let Some(v) = value {
            c = c.value(v);
        }
        described.push(json!({"kind": "toggle", "label": label, "parameter": param}));
        asset = asset.control(c);
    }
    for spec in &args.buttons {
        let (label, param, value) = parse_control_spec(spec, "button")?;
        let mut c = MenuControlSpec::button(&label, &param);
        if let Some(v) = value {
            c = c.value(v);
        }
        described.push(json!({"kind": "button", "label": label, "parameter": param}));
        asset = asset.control(c);
    }
    for spec in &args.radials {
        let (label, param, _) = parse_control_spec(spec, "radial")?;
        described.push(json!({"kind": "radial", "label": label, "parameter": param}));
        asset = asset.control(MenuControlSpec::radial(&label, &param));
    }
    for spec in &args.submenus {
        let (label, guid) = spec.split_once(':').with_context(|| {
            format!("submenu '{spec}' must be LABEL:GUID (the child menu asset's guid)")
        })?;
        if guid.len() != 32 || !guid.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("submenu '{spec}': '{guid}' is not a 32-hex-char Unity guid");
        }
        described.push(json!({"kind": "submenu", "label": label, "guid": guid}));
        asset = asset.control(MenuControlSpec::sub_menu(
            label,
            ObjectRef::external(
                avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID,
                guid,
                2,
            ),
        ));
    }
    if asset.controls.is_empty() {
        bail!("nothing to generate: pass at least one --toggle/--button/--radial/--submenu");
    }
    if asset.controls.len() > 8 {
        bail!(
            "a VRChat expressions menu holds at most 8 controls; got {} (split into sub-menus)",
            asset.controls.len()
        );
    }

    let yaml = asset.to_unity_yaml(avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID);
    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &yaml, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "menu",
            "name": args.name,
            "controls": described,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "yaml": yaml,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

/// Parse a `NAME:TYPE[:DEFAULT][:unsaved][:local]` expression-parameter spec.
pub(crate) fn parse_param_spec(spec: &str) -> Result<ExpressionParamSpec> {
    let mut parts = spec.split(':');
    let name = parts.next().unwrap_or_default();
    let ty = parts.next().unwrap_or_default();
    if name.is_empty() || ty.is_empty() {
        bail!("param '{spec}' must be NAME:TYPE[:DEFAULT][:unsaved][:local], e.g. Hat:bool");
    }
    let mut p = match ty.to_ascii_lowercase().as_str() {
        "bool" => ExpressionParamSpec::bool(name),
        "int" => ExpressionParamSpec::int(name),
        "float" => ExpressionParamSpec::float(name),
        other => bail!("param '{spec}': unknown type '{other}' (bool|int|float)"),
    };
    for tok in parts {
        match tok.to_ascii_lowercase().as_str() {
            "unsaved" => p = p.saved(false),
            "saved" => p = p.saved(true),
            "local" => p = p.synced(false),
            "synced" => p = p.synced(true),
            num => {
                let v: f32 = num.parse().with_context(|| {
                    format!("param '{spec}': '{tok}' is neither a default value nor a flag")
                })?;
                p = p.default_value(v);
            }
        }
    }
    Ok(p)
}

/// Parse a `LABEL:PARAM[:VALUE]` menu-control spec.
pub(crate) fn parse_control_spec(spec: &str, kind: &str) -> Result<(String, String, Option<f32>)> {
    let fail = || format!("{kind} '{spec}' must be LABEL:PARAM[:VALUE], e.g. Hat:Hat");
    let mut parts = spec.splitn(3, ':');
    let label = parts.next().unwrap_or_default();
    let param = parts.next().with_context(fail)?;
    if label.is_empty() || param.is_empty() {
        bail!("{}", fail());
    }
    let value = match parts.next() {
        Some(v) => Some(
            v.parse::<f32>()
                .with_context(|| format!("{kind} '{spec}': value '{v}' is not a number"))?,
        ),
        None => None,
    };
    Ok((label.to_string(), param.to_string(), value))
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
pub(crate) fn parse_clip_spec(spec: &str) -> Result<(String, f32)> {
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
/// value then the shape off the right. Shared with `avatar toggle`.
pub(crate) fn parse_blendshape_spec(spec: &str) -> Result<(String, String, f32)> {
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
