//! `avatar mcp serve` — expose the read/diagnose surface as Model Context Protocol tools over stdio.
//!
//! An agent host can `tools/list` to discover what this binary can answer and `tools/call` each tool
//! with typed JSON arguments, getting back the same JSON reports the `--json` CLI flags emit — without
//! spawning a subprocess per question or parsing free-form stdout. The protocol plumbing lives in the
//! domain-agnostic [`avatar_mcp`] crate; this module is just the *wiring*: it maps each tool name to
//! the library call that produces its report.
//!
//! **Non-writing by design.** Nothing here touches the filesystem for output. The diagnose/inspect
//! tools read assets and return reports; the **generation tools** (`avatar_gen_*`) run the same
//! generators as `avatar anim-gen …` / `avatar toggle` but return the generated YAML (and `.meta`
//! sidecars) as *text in the report* — the agent host decides what, if anything, lands on disk.
//! Repairs (`armature fix`) stay on the explicit CLI behind
//! [`WriteGuard`](crate::cmd::WriteGuard).
//!
//! Each handler validates its path argument *up front* with an actionable message — "path does not
//! exist", "expected an FBX file but … is a directory" — so a wrong argument comes back as guidance
//! the agent can act on rather than a deep parser error.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_mcp::{Server, Tool};
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Subcommand, Debug)]
pub enum McpCommand {
    /// Run the MCP server over stdio (newline-delimited JSON-RPC on stdin/stdout).
    Serve(ServeArgs),
}

#[derive(Args, Debug)]
pub struct ServeArgs {}

/// Run the stdio MCP server until stdin reaches EOF.
pub fn serve(_args: &ServeArgs) -> Result<()> {
    build_server().serve_stdio()
}

/// Assemble the avatar tool registry. Factored out so a test can introspect the published tools.
pub fn build_server() -> Server {
    let server = Server::new("avatar", env!("CARGO_PKG_VERSION"))
        .tool(Tool::new(
            "avatar_describe",
            "One-shot consolidated snapshot of an avatar asset. For an FBX: structure + humanoid-\
             armature analysis + geometry performance. For a Unity project (or a path inside one): \
             SDK3 lint + per-avatar performance. Read-only. Best first call to understand an asset.",
            path_schema("Path to a binary .fbx file, or a Unity project directory (or any path inside one)."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                let report = crate::cmd::describe::build(&path)
                    .with_context(|| format!("describing {}", path.display()))?;
                to_json(&report)
            }),
        ))
        .tool(Tool::new(
            "avatar_lint",
            "Lint a Unity/VRChat project for SDK3 compliance (VRC001–VRC052): expression \
             parameters/menus, the avatar descriptor, animator controllers, PhysBones, and VPM info. \
             Returns the structured LintReport (diagnostics with code, severity, file, and hint).",
            path_schema("Path to a Unity project directory (or any path inside one)."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                let report = avatar_lint::run(&path)
                    .with_context(|| format!("linting project at {}", path.display()))?;
                to_json(&report)
            }),
        ))
        .tool(Tool::new(
            "avatar_stats",
            "Estimate the VRChat performance ranking. For an .fbx: geometry side (triangles, meshes, \
             material slots, bones). For a project: one report per avatar including component-side \
             metrics (PhysBones, particles, constraints, texture memory).",
            path_schema("Path to a binary .fbx file, or a Unity project directory (or any path inside one)."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                if is_fbx(&path) {
                    let report = avatar_stats::analyze_fbx(&path)
                        .with_context(|| format!("analyzing FBX geometry of {}", path.display()))?;
                    to_json(&report)
                } else {
                    let reports = avatar_stats::analyze_project(&path)
                        .with_context(|| format!("analyzing project avatars at {}", path.display()))?;
                    to_json(&reports)
                }
            }),
        ))
        .tool(Tool::new(
            "avatar_armature_check",
            "Validate an FBX skeleton against VRChat humanoid requirements: which humanoid bones are \
             mapped, which required/recommended bones are missing, and topology/orientation flags. \
             Returns the ArmatureReport.",
            path_schema("Path to a binary .fbx file."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                require_fbx(&path)?;
                let scene = avatar_fbx::FbxScene::load(&path)
                    .with_context(|| format!("loading FBX {}", path.display()))?;
                let report = avatar_armature::analyze(&scene);
                to_json(&report)
            }),
        ))
        .tool(Tool::new(
            "avatar_fbx_inspect",
            "Summarize an FBX file's structure: version, object/model/geometry/material counts, and \
             bone-like node count, plus unit/orientation flags. Returns the InspectSummary.",
            path_schema("Path to a binary .fbx file."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                require_fbx(&path)?;
                let scene = avatar_fbx::FbxScene::load(&path)
                    .with_context(|| format!("loading FBX {}", path.display()))?;
                to_json(&crate::cmd::fbx::inspect_summary(&scene))
            }),
        ))
        .tool(Tool::new(
            "avatar_physbone_list",
            "List every VRCPhysBone component in an SDK3 prefab: the object and root transform, \
             the chains it drives (leaf path, bone count, length), colliders, ignore list, and the \
             full tuning (pull/spring/stiffness/gravity/immobile/limits, with per-chain curves). \
             Read-only; the retune/split/stretch edits are `avatar physbone set|split|stretch` on \
             the CLI. Returns PhysBoneInfo[].",
            path_schema("Path to a Unity .prefab (an SDK3 avatar)."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let rw = avatar_migrate::rewrite::PrefabRewriter::new(&text)
                    .with_context(|| format!("parsing {} as a Unity prefab", path.display()))?;
                to_json(&avatar_migrate::physbone::list(rw.scene()))
            }),
        ))
        .tool(Tool::new(
            "avatar_gen_clip",
            "Generate a Unity .anim AnimationClip (blendshape and/or GameObject-active curves) and \
             return its YAML as text — nothing is written to disk; write the `yaml` to a file \
             yourself if wanted. Deterministic fileIDs.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Clip name (m_Name; also seeds the fileID)." },
                    "blendshapes": spec_array("Blendshape curves as PATH:SHAPE:VALUE (e.g. Body:Smile:100)."),
                    "toggles": spec_array("GameObject active-toggle curves, held on, by hierarchy PATH."),
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            Box::new(|args| {
                let name = arg_str(args, "name")?;
                let blendshapes = arg_str_array(args, "blendshapes")?;
                let toggles = arg_str_array(args, "toggles")?;
                if blendshapes.is_empty() && toggles.is_empty() {
                    bail!("nothing to generate: pass at least one entry in `blendshapes` or `toggles`");
                }
                let mut clip = avatar_anim_gen::AnimationClip::new(name);
                for spec in &blendshapes {
                    let (path, shape, value) = crate::cmd::anim_gen::parse_blendshape_spec(spec)?;
                    clip.add_float_curve(avatar_anim_gen::FloatCurve::blendshape(
                        path,
                        &shape,
                        vec![avatar_anim_gen::Keyframe::flat(0.0, value)],
                    ));
                }
                for path in &toggles {
                    clip.add_float_curve(avatar_anim_gen::FloatCurve::game_object_active(
                        path.clone(),
                        vec![avatar_anim_gen::Keyframe::flat(0.0, 1.0)],
                    ));
                }
                let mut ids = avatar_anim_gen::IdGen::new(name);
                let clip_id = ids.alloc();
                Ok(json!({
                    "clip_file_id": clip_id,
                    "suggested_file_name": format!("{name}.anim"),
                    "yaml": clip.to_unity_yaml(clip_id),
                })
                .to_string())
            }),
        ))
        .tool(Tool::new(
            "avatar_gen_controller",
            "Generate a complete FX AnimatorController (class 91) wrapping a 1D analog-gesture \
             blend tree, returned as YAML text — nothing is written to disk. Child clips are \
             referenced by their .anim asset guids.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Controller name (default FX)." },
                    "layer": { "type": "string", "description": "Layer name (default 'Base Layer')." },
                    "parameter": { "type": "string", "description": "Float blend parameter (default GestureLeftWeight)." },
                    "clips": spec_array("Child clips as GUID@THRESHOLD (e.g. 1a2b…@0.0)."),
                },
                "required": ["clips"],
                "additionalProperties": false
            }),
            Box::new(|args| {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("FX");
                let layer = args.get("layer").and_then(Value::as_str).unwrap_or("Base Layer");
                let parameter = args
                    .get("parameter")
                    .and_then(Value::as_str)
                    .unwrap_or("GestureLeftWeight");
                let clips = arg_str_array(args, "clips")?;
                if clips.is_empty() {
                    bail!("`clips` must contain at least one GUID@THRESHOLD entry");
                }
                let mut tree = avatar_anim_gen::BlendTree::analog_gesture(name, parameter);
                for spec in &clips {
                    let (guid, threshold) = crate::cmd::anim_gen::parse_clip_spec(spec)?;
                    tree = tree.clip(guid, threshold);
                }
                let mut ids = avatar_anim_gen::IdGen::new(name);
                Ok(json!({
                    "parameter": parameter,
                    "suggested_file_name": format!("{name}.controller"),
                    "yaml": avatar_anim_gen::fx_blend_tree(name, layer, &tree, &mut ids),
                })
                .to_string())
            }),
        ))
        .tool(Tool::new(
            "avatar_gen_params",
            "Generate a VRCExpressionParameters asset, returned as YAML text — nothing is written \
             to disk. Reports the 256-bit sync-budget cost.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Asset name (default Parameters)." },
                    "params": spec_array("Parameters as NAME:TYPE[:DEFAULT][:unsaved][:local]; TYPE is bool|int|float."),
                },
                "required": ["params"],
                "additionalProperties": false
            }),
            Box::new(|args| {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("Parameters");
                let specs = arg_str_array(args, "params")?;
                if specs.is_empty() {
                    bail!("`params` must contain at least one NAME:TYPE entry");
                }
                let mut asset = avatar_anim_gen::ExpressionParams::new(name);
                for spec in &specs {
                    asset = asset.parameter(crate::cmd::anim_gen::parse_param_spec(spec)?);
                }
                Ok(json!({
                    "sync_bits": asset.synced_bits(),
                    "suggested_file_name": format!("{name}.asset"),
                    "yaml": asset.to_unity_yaml(avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID),
                })
                .to_string())
            }),
        ))
        .tool(Tool::new(
            "avatar_gen_menu",
            "Generate a VRCExpressionsMenu asset (toggles, buttons, radial puppets, sub-menus; max \
             8 controls), returned as YAML text — nothing is written to disk.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Asset name (default Menu)." },
                    "toggles": spec_array("Toggle controls as LABEL:PARAM[:VALUE]."),
                    "buttons": spec_array("Momentary button controls as LABEL:PARAM[:VALUE]."),
                    "radials": spec_array("Radial-puppet controls as LABEL:PARAM (PARAM is the float axis)."),
                    "submenus": spec_array("Sub-menu controls as LABEL:GUID (the child menu asset's guid)."),
                },
                "additionalProperties": false
            }),
            Box::new(|args| {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("Menu");
                let mut asset = avatar_anim_gen::ExpressionsMenu::new(name);
                for spec in &arg_str_array(args, "toggles")? {
                    let (label, param, value) =
                        crate::cmd::anim_gen::parse_control_spec(spec, "toggle")?;
                    let mut c = avatar_anim_gen::MenuControlSpec::toggle(label, param);
                    if let Some(v) = value {
                        c = c.value(v);
                    }
                    asset = asset.control(c);
                }
                for spec in &arg_str_array(args, "buttons")? {
                    let (label, param, value) =
                        crate::cmd::anim_gen::parse_control_spec(spec, "button")?;
                    let mut c = avatar_anim_gen::MenuControlSpec::button(label, param);
                    if let Some(v) = value {
                        c = c.value(v);
                    }
                    asset = asset.control(c);
                }
                for spec in &arg_str_array(args, "radials")? {
                    let (label, param, _) =
                        crate::cmd::anim_gen::parse_control_spec(spec, "radial")?;
                    asset = asset.control(avatar_anim_gen::MenuControlSpec::radial(label, param));
                }
                for spec in &arg_str_array(args, "submenus")? {
                    let (label, guid) = spec.split_once(':').with_context(|| {
                        format!("submenu '{spec}' must be LABEL:GUID")
                    })?;
                    asset = asset.control(avatar_anim_gen::MenuControlSpec::sub_menu(
                        label,
                        avatar_anim_gen::ObjectRef::external(
                            avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID,
                            guid,
                            2,
                        ),
                    ));
                }
                if asset.controls.is_empty() {
                    bail!("pass at least one control in `toggles`/`buttons`/`radials`/`submenus`");
                }
                if asset.controls.len() > 8 {
                    bail!(
                        "a VRChat expressions menu holds at most 8 controls; got {}",
                        asset.controls.len()
                    );
                }
                Ok(json!({
                    "controls": asset.controls.len(),
                    "suggested_file_name": format!("{name}.asset"),
                    "yaml": asset.to_unity_yaml(avatar_anim_gen::expressions::EXPRESSIONS_MAIN_FILE_ID),
                })
                .to_string())
            }),
        ))
        .tool(Tool::new(
            "avatar_gen_toggle",
            "Generate the complete, internally-consistent toggle bundle: On/Off .anim clips, a \
             two-state FX .controller on a Bool parameter, VRCExpressionParameters + \
             VRCExpressionsMenu assets, and .meta sidecars pinning deterministic guids so the \
             cross-references resolve on first import. Returns every file's name + content as text \
             plus a wiring note — nothing is written to disk.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Bundle name; seeds file names, fileIDs, and guids (e.g. Hat)." },
                    "toggles": spec_array("GameObjects to toggle, by hierarchy PATH."),
                    "blendshapes": spec_array("Blendshapes to drive as PATH:SHAPE:VALUE (VALUE when on, 0 when off)."),
                    "parameter": { "type": "string", "description": "The Bool parameter (defaults to the bundle name)." },
                    "menu_label": { "type": "string", "description": "Menu control label (defaults to the bundle name)." },
                    "default_on": { "type": "boolean", "description": "Start toggled on (default false)." },
                    "saved": { "type": "boolean", "description": "Persist across avatar loads (default true)." },
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            Box::new(|args| {
                let name = arg_str(args, "name")?;
                let mut targets: Vec<avatar_anim_gen::ToggleTarget> = arg_str_array(args, "toggles")?
                    .into_iter()
                    .map(|path| avatar_anim_gen::ToggleTarget::GameObject { path })
                    .collect();
                for spec in &arg_str_array(args, "blendshapes")? {
                    let (path, shape, on_value) = crate::cmd::anim_gen::parse_blendshape_spec(spec)?;
                    targets.push(avatar_anim_gen::ToggleTarget::Blendshape { path, shape, on_value });
                }
                if targets.is_empty() {
                    bail!("nothing to toggle: pass at least one entry in `toggles` or `blendshapes`");
                }
                let spec = avatar_anim_gen::ToggleSpec {
                    name: name.to_string(),
                    parameter: args
                        .get("parameter")
                        .and_then(Value::as_str)
                        .unwrap_or(name)
                        .to_string(),
                    targets,
                    saved: args.get("saved").and_then(Value::as_bool).unwrap_or(true),
                    default_on: args.get("default_on").and_then(Value::as_bool).unwrap_or(false),
                    menu_label: args
                        .get("menu_label")
                        .and_then(Value::as_str)
                        .unwrap_or(name)
                        .to_string(),
                };
                to_json(&avatar_anim_gen::generate_toggle(&spec))
            }),
        ))
        .tool(Tool::new(
            "avatar_unitypackage_info",
            "Summarize a .unitypackage archive without extracting it: entry/size counts, size by \
             extension, detected VRChat SDK, and whether it looks like an avatar or a world. Use to \
             triage a package before extracting it for lint/stats.",
            path_schema("Path to a .unitypackage file."),
            Box::new(|args| {
                let path = arg_existing_path(args, "path")?;
                let pkg = avatar_unitypackage::UnityPackage::open(&path)
                    .with_context(|| format!("opening package {}", path.display()))?;
                to_json(&pkg.summary())
            }),
        ));

    // The schema tool needs the report types' JSON Schemas, which only exist under the `schema`
    // feature (on by default in the cli). Register it only when available so a `--no-default-features`
    // build still serves the rest of the tools.
    #[cfg(feature = "schema")]
    let server = server.tool(Tool::new(
        "avatar_schema",
        "Return the JSON Schema for a report type, so a consumer can introspect the exact output \
         shape of the other tools. Pass `name` = one of the published schemas, or omit it to list \
         the available names, or `all` for every schema as one object.",
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Schema name (e.g. describe/lint/stats/armature/fbx-inspect), or 'all'. Omit to list names."
                }
            },
            "additionalProperties": false
        }),
        Box::new(|args| {
            let name = args.get("name").and_then(Value::as_str);
            match name {
                None => Ok(json!({ "available": crate::cmd::schema::schema_names() }).to_string()),
                Some("all") => {
                    let map: serde_json::Map<String, Value> = crate::cmd::schema::schema_names()
                        .into_iter()
                        .map(|n| (n.to_string(), crate::cmd::schema::schema_value(n).unwrap()))
                        .collect();
                    Ok(Value::Object(map).to_string())
                }
                Some(n) => {
                    let schema = crate::cmd::schema::schema_value(n)?;
                    Ok(schema.to_string())
                }
            }
        }),
    ));

    server
}

/// Standard single-`path`-argument schema with a description for that path.
fn path_schema(path_desc: &str) -> Value {
    json!({
        "type": "object",
        "properties": { "path": { "type": "string", "description": path_desc } },
        "required": ["path"],
        "additionalProperties": false
    })
}

/// JSON Schema for an array-of-spec-strings argument, with a description of the spec format.
fn spec_array(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

/// Extract an optional array-of-strings argument (absent = empty). Non-string members error.
fn arg_str_array(args: &Value, key: &str) -> Result<Vec<String>> {
    let Some(v) = args.get(key) else {
        return Ok(Vec::new());
    };
    let list = v
        .as_array()
        .with_context(|| format!("argument `{key}` must be an array of strings"))?;
    list.iter()
        .map(|e| {
            e.as_str()
                .map(str::to_string)
                .with_context(|| format!("argument `{key}` must contain only strings"))
        })
        .collect()
}

/// Extract a required, non-empty string argument.
fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    let s = args
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing required string argument `{key}`"))?;
    if s.trim().is_empty() {
        bail!("argument `{key}` must not be empty");
    }
    Ok(s)
}

/// Extract a `path` argument and verify it exists, with an actionable message if not. This is the
/// first thing an agent gets wrong, so the message names the fix (paths resolve against the server's
/// working directory).
fn arg_existing_path(args: &Value, key: &str) -> Result<PathBuf> {
    let path = PathBuf::from(arg_str(args, key)?);
    if !path.exists() {
        bail!(
            "path does not exist: {} — paths are resolved relative to the MCP server's working \
             directory; pass an absolute path or check the spelling",
            path.display()
        );
    }
    Ok(path)
}

/// True when `path` is a file with an `.fbx` extension (mirrors the CLI's dispatch).
fn is_fbx(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
}

/// For FBX-only tools: fail with guidance when handed a directory or a non-`.fbx` file, rather than
/// letting the FBX parser report a low-level "bad magic"/"is a directory" error.
fn require_fbx(path: &Path) -> Result<()> {
    if path.is_dir() {
        bail!(
            "expected a single FBX file but {} is a directory — for a Unity project use avatar_lint, \
             avatar_stats, or avatar_describe instead",
            path.display()
        );
    }
    if !is_fbx(path) {
        bail!(
            "expected a .fbx file but {} has a different extension",
            path.display()
        );
    }
    Ok(())
}

/// Serialize a report as pretty JSON for the tool's text content.
fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string_pretty(value).context("serializing report to JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server advertises the core read tools (and the schema tool under the default features).
    #[test]
    fn registers_expected_tools() {
        let server = build_server();
        let names: Vec<&str> = server.tools().iter().map(|t| t.name.as_str()).collect();
        for expected in [
            "avatar_describe",
            "avatar_lint",
            "avatar_stats",
            "avatar_armature_check",
            "avatar_fbx_inspect",
            "avatar_physbone_list",
            "avatar_unitypackage_info",
            "avatar_gen_clip",
            "avatar_gen_controller",
            "avatar_gen_params",
            "avatar_gen_menu",
            "avatar_gen_toggle",
        ] {
            assert!(
                names.contains(&expected),
                "missing tool {expected}; have {names:?}"
            );
        }
        // Every tool's input schema is an object schema requiring/allowing a `path` (or `name`).
        for t in server.tools() {
            assert_eq!(t.input_schema["type"], "object", "tool {} schema", t.name);
        }
    }

    /// Call a tool through the pure JSON-RPC dispatch and return the response value.
    fn call(server: &Server, tool: &str, arguments: Value) -> Value {
        server
            .handle(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": tool, "arguments": arguments },
            }))
            .expect("a request gets a response")
    }

    /// The generation tools return the generated YAML in their result and never touch the
    /// filesystem: calling one must not create any file.
    #[test]
    fn gen_toggle_returns_bundle_without_writing() {
        let server = build_server();
        let resp = call(
            &server,
            "avatar_gen_toggle",
            json!({ "name": "Hat", "toggles": ["Armature/Head/Hat"] }),
        );
        assert_ne!(
            resp["result"]["isError"],
            json!(true),
            "call succeeds: {resp}"
        );
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("text content");
        let v: Value = serde_json::from_str(text).expect("result is JSON");
        let files = v["files"].as_array().expect("bundle files present");
        assert_eq!(files.len(), 10, "5 assets + 5 .meta sidecars");
        assert!(v["wiring_note"].as_str().is_some_and(|s| !s.is_empty()));
        // Nothing landed on disk under the current directory.
        assert!(!Path::new("Hat_FX.controller").exists());
        assert!(!Path::new("Hat_On.anim").exists());
    }

    #[test]
    fn gen_clip_requires_a_curve() {
        let server = build_server();
        let resp = call(&server, "avatar_gen_clip", json!({ "name": "Empty" }));
        assert_eq!(resp["result"]["isError"], json!(true));
        let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("nothing to generate"), "got: {text}");
    }

    #[test]
    fn missing_path_argument_is_actionable() {
        let err = arg_existing_path(&json!({}), "path").unwrap_err();
        assert!(format!("{err:#}").contains("missing required string argument `path`"));
    }

    #[test]
    fn nonexistent_path_names_the_fix() {
        let err = arg_existing_path(&json!({"path": "/no/such/file.fbx"}), "path").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("does not exist"), "got: {msg}");
        assert!(msg.contains("/no/such/file.fbx"), "names the path: {msg}");
    }

    #[test]
    fn require_fbx_rejects_directory_with_guidance() {
        let err = require_fbx(Path::new(".")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("directory"), "got: {msg}");
        assert!(
            msg.contains("avatar_lint"),
            "points at the project tools: {msg}"
        );
    }
}
