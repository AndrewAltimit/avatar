//! `avatar mcp serve` — expose the read/diagnose surface as Model Context Protocol tools over stdio.
//!
//! An agent host can `tools/list` to discover what this binary can answer and `tools/call` each tool
//! with typed JSON arguments, getting back the same JSON reports the `--json` CLI flags emit — without
//! spawning a subprocess per question or parsing free-form stdout. The protocol plumbing lives in the
//! domain-agnostic [`avatar_mcp`] crate; this module is just the *wiring*: it maps each tool name to
//! the library call that produces its report.
//!
//! **Read-only by design.** Only the diagnose/inspect surface is exposed — nothing here writes to the
//! filesystem. The generators (`anim-gen …`) and repairs (`armature fix`) stay on the explicit CLI
//! behind [`WriteGuard`](crate::cmd::WriteGuard), so an agent can call every MCP tool freely with no
//! risk of mutating assets. (Exposing generation as text-returning, non-writing tools is a clean
//! follow-up.)
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
            "avatar_unitypackage_info",
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
