//! `avatar` — command-line entry point for the VRChat avatar tools.
//!
//! Subcommands:
//!   - `avatar fbx inspect <path>`        — dump an FBX's structure and flag unit/orientation issues
//!   - `avatar armature check <path>`     — validate the skeleton against VRChat humanoid requirements
//!   - `avatar armature fix <path> -o …`  — write a repaired FBX (canonical bone names, topology)
//!   - `avatar lint <project>`            — SDK3-compliance report over a Unity/VRChat project
//!   - `avatar stats <path>`              — VRChat performance ranking of an FBX or project avatar
//!   - `avatar anim-gen blendtree …`      — generate an analog-gesture FX blend-tree (Unity YAML)
//!   - `avatar anim-gen clip …`           — generate a `.anim` clip (blendshape / toggle curves)
//!   - `avatar osc send|input|monitor …`  — drive / observe a running VRChat over OSC
//!   - `avatar osc query <config.json>`   — list an avatar's parameters from its OSCQuery config
//!   - `avatar osc gestures`              — run the analog-gesture daemon (demo trigger sweep)
//!   - `avatar unitypackage info <pkg>`   — summarize a `.unitypackage` (contents, SDK, avatar/world)
//!   - `avatar unitypackage list <pkg>`   — list a package's assets (path, guid, size)
//!   - `avatar unitypackage extract …`    — extract a package into a Unity `Assets/` tree
//!   - `avatar unitypackage testbed …`    — cross-check an avatar package against a world/map package

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

mod render_scene;
mod texture;
mod world;

use anyhow::{Context, Result, bail};
use avatar_anim_gen::{AnimationClip, BlendTree, Emitter, FloatCurve, IdGen, Keyframe};
use avatar_armature::{RepairPlan, apply_plan, plan_repairs};
use avatar_fbx::{FbxDocument, FbxScene};
use avatar_osc::{AvatarConfig, InputAxis, InputButton, ParamClient, ParamValue};
use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "avatar",
    version,
    about = "Tools for working with VRChat avatars (Unity, SDK3 / Avatars 3.0)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect and operate on FBX files.
    #[command(subcommand)]
    Fbx(FbxCommand),
    /// Inspect and validate avatar armatures / skeletons.
    #[command(subcommand)]
    Armature(ArmatureCommand),
    /// Lint a Unity/VRChat project for SDK3 compliance.
    Lint(LintArgs),
    /// Estimate the VRChat performance ranking of an FBX or a project's avatar(s).
    Stats(StatsArgs),
    /// Generate Unity animation assets (`.anim` clips, FX analog-gesture blend trees).
    #[command(subcommand, name = "anim-gen")]
    AnimGen(AnimGenCommand),
    /// Drive or observe a running VRChat avatar over OSC.
    #[command(subcommand)]
    Osc(OscCommand),
    /// Inspect, extract, and cross-check `.unitypackage` archives (avatars, worlds/maps).
    #[command(subcommand)]
    Unitypackage(UnitypackageCommand),
    /// Render an avatar (and/or a world scene) to a PNG with an offscreen GPU pipeline.
    Render(RenderArgs),
    /// Open an interactive window onto an avatar dropped into a world (orbit / zoom / walk).
    View(ViewArgs),
}

#[derive(Args, Debug)]
struct ViewArgs {
    /// Avatar to view: an `.fbx`, `.gltf`, or `.glb` file (rest/bind pose).
    #[arg(long)]
    avatar: Option<PathBuf>,
    /// World/map to view: a `.unity` scene file, or a Unity project dir (its first scene is used).
    #[arg(long)]
    world: Option<PathBuf>,
    /// Initial window width in pixels.
    #[arg(long, default_value_t = 1280)]
    width: u32,
    /// Initial window height in pixels.
    #[arg(long, default_value_t = 720)]
    height: u32,
    /// Initial camera orbit yaw, in degrees.
    #[arg(long, default_value_t = 35.0)]
    yaw: f32,
    /// Initial camera orbit pitch, in degrees.
    #[arg(long, default_value_t = 18.0)]
    pitch: f32,
    /// What the camera initially frames on (`avatar` by default when one is present; `world`).
    #[arg(long, value_enum)]
    frame: Option<FrameTarget>,
}

#[derive(Args, Debug)]
struct RenderArgs {
    /// Avatar to render: an `.fbx`, `.gltf`, or `.glb` file (rest/bind pose).
    #[arg(long)]
    avatar: Option<PathBuf>,
    /// World/map to render: a `.unity` scene file, or a Unity project dir (its first scene is used).
    #[arg(long)]
    world: Option<PathBuf>,
    /// Output PNG path.
    #[arg(short, long, default_value = "render.png")]
    output: PathBuf,
    /// Image width in pixels.
    #[arg(long, default_value_t = 960)]
    width: u32,
    /// Image height in pixels.
    #[arg(long, default_value_t = 720)]
    height: u32,
    /// Camera orbit yaw around the scene, in degrees.
    #[arg(long, default_value_t = 35.0)]
    yaw: f32,
    /// Camera orbit pitch above the scene, in degrees.
    #[arg(long, default_value_t = 18.0)]
    pitch: f32,
    /// What the camera frames on. `avatar` (the default when an avatar is dropped into a world)
    /// fills the shot with the avatar, the map visible around it; `world` frames the whole scene.
    #[arg(long, value_enum)]
    frame: Option<FrameTarget>,
}

/// Camera framing target for `avatar render`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum FrameTarget {
    /// Frame on the avatar, with the surrounding map visible.
    Avatar,
    /// Frame on the entire scene's bounds.
    World,
}

#[derive(Subcommand, Debug)]
enum UnitypackageCommand {
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
struct UpInfoArgs {
    /// Path to a `.unitypackage` file.
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct UpListArgs {
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
struct UpExtractArgs {
    /// Path to a `.unitypackage` file.
    path: PathBuf,
    /// Destination directory (created if missing). The project tree is written under it.
    #[arg(short, long)]
    output: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct UpTestbedArgs {
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

#[derive(Subcommand, Debug)]
enum AnimGenCommand {
    /// Emit a 1D analog-gesture blend tree (blends `GestureLeftWeight`/`…Right` over child clips).
    Blendtree(BlendtreeArgs),
    /// Emit a `.anim` clip from blendshape and/or GameObject-active (toggle) curves.
    Clip(ClipArgs),
}

#[derive(Args, Debug)]
struct BlendtreeArgs {
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
struct ClipArgs {
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

#[derive(Subcommand, Debug)]
enum OscCommand {
    /// Set one avatar parameter: `/avatar/parameters/<NAME>`.
    Send(OscSendArgs),
    /// Send an input axis (`-1..1`) or button (`true`/`false`): `/input/<NAME>`.
    Input(OscInputArgs),
    /// Listen for the avatar parameters VRChat broadcasts and print each update.
    Monitor(OscMonitorArgs),
    /// Request VRChat switch avatars: `/avatar/change <blueprint-id>`.
    Change(OscChangeArgs),
    /// Parse an avatar's OSCQuery config JSON and list its parameters (offline; no socket).
    Query(OscQueryArgs),
    /// Run the analog-gesture daemon: map a controller trigger → `Gesture*`/`Gesture*Weight` over
    /// OSC. With no on-device input backend headless, this drives a synthetic demo trigger sweep.
    Gestures(OscGesturesArgs),
}

/// Where a running VRChat listens for our messages (its default OSC-in port is 9000).
#[derive(Args, Debug)]
struct OscTarget {
    /// Host VRChat is reachable at.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port VRChat listens on for incoming OSC.
    #[arg(long, default_value_t = avatar_osc::VRCHAT_RECV_PORT)]
    port: u16,
}

#[derive(Args, Debug)]
struct OscSendArgs {
    /// Parameter name (without the `/avatar/parameters/` prefix).
    name: String,
    /// Value: `true`/`false` (bool), an integer (int), or anything else parses as float.
    /// Override the auto-detected type with `--type`.
    value: String,
    /// Force the OSC value type instead of auto-detecting from `value`.
    #[arg(long = "type", value_enum)]
    value_type: Option<OscValueType>,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum OscValueType {
    Bool,
    Int,
    Float,
}

#[derive(Args, Debug)]
struct OscInputArgs {
    /// Input name — a VRChat axis (e.g. `Vertical`) or button (e.g. `Jump`).
    name: String,
    /// Axis value (`-1..1`) or button state (`true`/`false`), matching the input's kind.
    value: String,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Args, Debug)]
struct OscChangeArgs {
    /// Avatar blueprint id (`avtr_…`).
    id: String,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Args, Debug)]
struct OscMonitorArgs {
    /// Address to listen on (VRChat's default OSC-out port is 9001).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    /// Port to listen on for VRChat's outgoing parameters.
    #[arg(long, default_value_t = avatar_osc::VRCHAT_SEND_PORT)]
    port: u16,
    /// Stop after this many seconds. Without it, runs until interrupted (Ctrl-C).
    #[arg(long)]
    seconds: Option<u64>,
}

#[derive(Args, Debug)]
struct OscQueryArgs {
    /// Path to an avatar's OSCQuery config JSON (VRChat writes these under its OSC/ folder).
    path: PathBuf,
    /// Emit the parsed parameter list as JSON instead of a table.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct OscGesturesArgs {
    /// Tick rate in Hz (how often the daemon polls input and sends OSC).
    #[arg(long, default_value_t = 100)]
    hz: u32,
    /// Demo trigger-sweep period, in ticks (a full 0→1→0 pull). Default ~1.2s at 100 Hz.
    #[arg(long, default_value_t = 120)]
    period: u64,
    /// Stop after this many seconds. Without it, runs until interrupted (Ctrl-C).
    #[arg(long)]
    seconds: Option<u64>,
    #[command(flatten)]
    target: OscTarget,
}

#[derive(Args, Debug)]
struct StatsArgs {
    /// Path to a binary FBX file, or a Unity project (or any path inside one).
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct LintArgs {
    /// Path to a Unity project (or any path inside one).
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Exit non-zero if any warnings are found, not just errors (useful for gating CI).
    #[arg(long)]
    deny_warnings: bool,
}

#[derive(Subcommand, Debug)]
enum FbxCommand {
    /// Print the structure of an FBX file (objects, hierarchy, global settings).
    Inspect(FileArgs),
}

#[derive(Subcommand, Debug)]
enum ArmatureCommand {
    /// Validate the skeleton against VRChat humanoid rig requirements.
    Check(FileArgs),
    /// Plan (and optionally write) repairs: canonical humanoid bone names + parent topology.
    Fix(FixArgs),
}

#[derive(Args, Debug)]
struct FixArgs {
    /// Path to a binary FBX file.
    path: PathBuf,
    /// Write the repaired FBX here. Without this, runs as a dry run (prints the plan only).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Allow `--output` to overwrite the input file.
    #[arg(long)]
    force: bool,
    /// Emit a machine-readable JSON plan instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
struct FileArgs {
    /// Path to a binary FBX file.
    path: PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

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
    let cli = Cli::parse();
    match cli.command {
        Command::Fbx(FbxCommand::Inspect(args)) => fbx_inspect(&args).map(|()| ExitCode::SUCCESS),
        Command::Armature(ArmatureCommand::Check(args)) => armature_check(&args),
        Command::Armature(ArmatureCommand::Fix(args)) => {
            armature_fix(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Lint(args) => lint(&args),
        Command::Stats(args) => stats(&args).map(|()| ExitCode::SUCCESS),
        Command::AnimGen(AnimGenCommand::Blendtree(args)) => {
            anim_gen_blendtree(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Clip(args)) => {
            anim_gen_clip(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Osc(OscCommand::Send(args)) => osc_send(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Input(args)) => osc_input(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Monitor(args)) => osc_monitor(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Change(args)) => osc_change(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Query(args)) => osc_query(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Gestures(args)) => osc_gestures(&args).map(|()| ExitCode::SUCCESS),
        Command::Unitypackage(UnitypackageCommand::Info(args)) => {
            up_info(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::List(args)) => {
            up_list(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::Extract(args)) => {
            up_extract(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::Testbed(args)) => up_testbed(&args),
        Command::Render(args) => render(&args).map(|()| ExitCode::SUCCESS),
        Command::View(args) => view(&args).map(|()| ExitCode::SUCCESS),
    }
}

/// Build the renderable [`avatar_render::Scene`]: load the world (if any), drop the avatar at the
/// world's player-spawn point at human scale (or render it standalone), then frame the camera. The
/// `width`/`height` set the framing aspect. Shared by `render` (offscreen PNG) and `view`
/// (interactive window); prints a short progress summary as it goes.
fn assemble_scene(
    avatar: Option<&Path>,
    world: Option<&Path>,
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    frame: Option<FrameTarget>,
) -> Result<avatar_render::Scene> {
    if avatar.is_none() && world.is_none() {
        bail!("nothing to render: pass --avatar <model> and/or --world <scene|project>");
    }
    let mut meshes = Vec::new();
    let mut textures = texture::TextureSet::new();
    // Where to drop the avatar inside the world, and the bounds to frame on if framing on it.
    let mut spawn = None;
    let mut avatar_bounds = None;
    if let Some(world) = world {
        let wl = render_scene::load_world(world, &mut textures)?;
        println!(
            "world: {} prop(s) + {} prefab instance(s) placed from {} ({} built-in / {} unresolved mesh refs skipped)",
            wl.placed,
            wl.placed_prefabs,
            world.display(),
            wl.skipped_builtin,
            wl.skipped_unresolved
        );
        spawn = wl.spawn;
        meshes.extend(wl.meshes);
    }
    if let Some(avatar) = avatar {
        // With a world, drop the avatar at its spawn point at human scale; otherwise render it alone.
        let av = match spawn {
            Some(p) if world.is_some() => {
                let (av, bounds) = render_scene::load_avatar_in_world(avatar, p, &mut textures)?;
                println!(
                    "avatar: {} mesh(es) from {}, dropped at world spawn ({:.1}, {:.1}, {:.1})",
                    av.len(),
                    avatar.display(),
                    p.x,
                    p.y,
                    p.z
                );
                avatar_bounds = Some(bounds);
                av
            }
            _ => {
                if world.is_some() {
                    println!("note: world declares no spawn point; rendering avatar at the origin");
                }
                let av = render_scene::load_avatar(avatar, &mut textures)?;
                println!("avatar: {} mesh(es) from {}", av.len(), avatar.display());
                avatar_bounds = render_scene::mesh_bounds(&av);
                av
            }
        };
        meshes.extend(av);
    }

    let textures = textures.into_textures();
    if !textures.is_empty() {
        println!("textures: {} decoded", textures.len());
    }
    // Default to framing on the avatar when one is present; `--frame world` overrides.
    let frame = frame.unwrap_or(if avatar_bounds.is_some() {
        FrameTarget::Avatar
    } else {
        FrameTarget::World
    });
    let focus = match frame {
        // Pull back to show the map around a world-placed avatar; frame a standalone avatar tightly
        // (with no world, the scene bounds already equal the avatar's).
        FrameTarget::Avatar if world.is_some() => {
            avatar_bounds.map(|b| render_scene::expand_bounds(b, 2.4))
        }
        _ => None,
    };
    render_scene::scene_from_meshes(meshes, textures, width, height, yaw, pitch, focus)
}

/// Render an avatar and/or world scene to a PNG via the offscreen GPU pipeline.
fn render(args: &RenderArgs) -> Result<()> {
    let scene = assemble_scene(
        args.avatar.as_deref(),
        args.world.as_deref(),
        args.width,
        args.height,
        args.yaw,
        args.pitch,
        args.frame,
    )?;
    let tris: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
    println!(
        "rendering {} mesh(es), {tris} triangles at {}x{} ...",
        scene.meshes.len(),
        args.width,
        args.height
    );
    let rgba = avatar_render::render_to_rgba(&scene, args.width, args.height)?;
    avatar_render::save_png(&args.output, args.width, args.height, &rgba)?;
    println!("wrote {}", args.output.display());
    Ok(())
}

/// Open an interactive window onto the assembled scene (avatar in its world). Same geometry as
/// `render`, but live: drag to orbit, wheel to zoom, WASD/Space/Shift to walk, `R` to reset.
fn view(args: &ViewArgs) -> Result<()> {
    let scene = assemble_scene(
        args.avatar.as_deref(),
        args.world.as_deref(),
        args.width,
        args.height,
        args.yaw,
        args.pitch,
        args.frame,
    )?;
    let tris: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
    println!(
        "opening viewer: {} mesh(es), {tris} triangles — drag = orbit, wheel = zoom, WASD/Space/Shift = walk, R = reset, Esc = quit",
        scene.meshes.len()
    );
    #[cfg(feature = "viewer")]
    {
        avatar_render::view(scene, "avatar viewer")
    }
    #[cfg(not(feature = "viewer"))]
    {
        let _ = scene;
        bail!("this build was compiled without the viewer; rebuild with `--features viewer`")
    }
}

/// Report the VRChat performance ranking of an FBX file (geometry side) or every avatar in a Unity
/// project (component side). Informational — always exits 0 on success.
fn stats(args: &StatsArgs) -> Result<()> {
    let is_fbx = args.path.is_file()
        && args
            .path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("fbx"));

    if is_fbx {
        let report = avatar_stats::analyze_fbx(&args.path)?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_perf_report(&report);
        }
        return Ok(());
    }

    let reports = avatar_stats::analyze_project(&args.path)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    if reports.is_empty() {
        println!("No avatars found (no VRC Avatar Descriptor in any prefab/scene under Assets/).");
    }
    for (i, report) in reports.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_perf_report(report);
    }
    Ok(())
}

fn print_perf_report(report: &avatar_stats::PerfReport) {
    use avatar_stats::Platform;

    let kind = match report.kind {
        "fbx" => "FBX geometry",
        _ => "avatar components",
    };
    println!("Performance: {}  [{kind}]\n", report.source);

    let row = |name: &str, value: &str, pc: &str, android: &str| {
        println!("  {name:<30} {value:>15}  {pc:<11} {android:<11}");
    };
    row("Metric", "Value", "PC", "Android");
    println!("  {:-<30} {:->15}  {:-<11} {:-<11}", "", "", "", "");
    let mut shows_dual = false;
    for s in &report.stats {
        shows_dual |= s.value != s.android_value;
        row(
            s.name,
            &metric_value(s),
            rank_label(s.pc),
            rank_label(s.android),
        );
    }
    println!("  {:-<30} {:->15}  {:-<11} {:-<11}", "", "", "", "");
    row(
        "Overall",
        "",
        report.overall(Platform::Pc).label(),
        report.overall(Platform::Android).label(),
    );

    if shows_dual {
        println!(
            "\n  (Texture Memory value shown as PC/Android — textures recompress differently per platform.)"
        );
    }
    if !report.not_evaluated.is_empty() {
        println!("\n  Not evaluated for this source (could lower the real rank):");
        println!("    {}", report.not_evaluated.join(", "));
    }
}

/// A rank label, or a dash for a metric not ranked on that platform.
fn rank_label(rank: Option<avatar_stats::Rank>) -> &'static str {
    rank.map_or("-", |r| r.label())
}

/// Display a metric's value. Texture Memory is a byte count shown in MB — and as `PC/Android` when
/// the two platforms differ (the usual case); the rest are plain counts, identical across platforms.
fn metric_value(stat: &avatar_stats::MetricStat) -> String {
    if stat.metric == avatar_stats::Metric::TextureMemory {
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        if stat.value == stat.android_value {
            format!("{:.1} MB", mb(stat.value))
        } else {
            format!("{:.1}/{:.1} MB", mb(stat.value), mb(stat.android_value))
        }
    } else {
        stat.value.to_string()
    }
}

/// Lint a project. Returns a failure exit code when the report has errors (or warnings, with
/// `--deny-warnings`) so the command can gate CI.
fn lint(args: &LintArgs) -> Result<ExitCode> {
    let report = avatar_lint::run(&args.path)?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(lint_exit_code(&report, args.deny_warnings));
    }

    println!("Lint: {}", report.project_root);
    println!(
        "  Unity {}, avatar SDK {}",
        report.unity_version.as_deref().unwrap_or("(unknown)"),
        report.avatar_sdk_version.as_deref().unwrap_or("(absent)")
    );
    println!(
        "  {} parameter asset(s), {} menu asset(s), {} descriptor(s), {} controller(s), {} package(s)",
        report.parameter_assets,
        report.menu_assets,
        report.descriptors,
        report.controllers,
        report.packages.len()
    );

    if report.diagnostics.is_empty() {
        println!("\n  No issues found.");
    } else {
        println!();
        for d in &report.diagnostics {
            let tag = match d.severity {
                avatar_lint::Severity::Error => "[X]",
                avatar_lint::Severity::Warn => "[!]",
                avatar_lint::Severity::Info => "[i]",
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

    Ok(lint_exit_code(&report, args.deny_warnings))
}

/// Failure when the report has errors, or — with `--deny-warnings` — any warnings.
fn lint_exit_code(report: &avatar_lint::LintReport, deny_warnings: bool) -> ExitCode {
    let failed = report.error_count() > 0 || (deny_warnings && report.warn_count() > 0);
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

// ----- anim-gen --------------------------------------------------------------------------------

/// Generate an analog-gesture blend tree. By default emits a self-contained
/// `AnimatorStateMachine` + `AnimatorState` + `BlendTree` fragment (splice into an FX `.controller`
/// and point a layer's `m_StateMachine` at the printed state-machine fileID); `--tree-only` emits
/// just the `BlendTree` document for grafting onto an existing Fist state.
fn anim_gen_blendtree(args: &BlendtreeArgs) -> Result<()> {
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
fn anim_gen_clip(args: &ClipArgs) -> Result<()> {
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

/// Write generated text to `output` (a file) or, when `None`, stdout.
fn write_out(output: Option<&Path>, text: &str) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

// ----- osc -------------------------------------------------------------------------------------

/// Bind a send-only client (ephemeral local port) aimed at a running VRChat.
fn sender_client(target: &OscTarget) -> Result<ParamClient> {
    ParamClient::new(("0.0.0.0", 0), (target.host.as_str(), target.port))
        .context("opening OSC send socket")
}

/// Send one avatar parameter to VRChat.
fn osc_send(args: &OscSendArgs) -> Result<()> {
    let value = parse_param_value(&args.value, args.value_type)?;
    let client = sender_client(&args.target)?;
    client.send_param(&args.name, value)?;
    println!(
        "sent /avatar/parameters/{} = {value:?} -> {}",
        args.name,
        client.target()
    );
    Ok(())
}

/// Resolve a textual value to a [`ParamValue`], honoring an explicit `--type` or auto-detecting
/// `true`/`false` → bool, integer → int, else float.
fn parse_param_value(s: &str, forced: Option<OscValueType>) -> Result<ParamValue> {
    match forced {
        Some(OscValueType::Bool) => Ok(ParamValue::Bool(parse_bool(s)?)),
        Some(OscValueType::Int) => Ok(ParamValue::Int(
            s.parse()
                .with_context(|| format!("'{s}' is not an integer"))?,
        )),
        Some(OscValueType::Float) => Ok(ParamValue::Float(
            s.parse()
                .with_context(|| format!("'{s}' is not a number"))?,
        )),
        None => {
            if let Some(b) = parse_bool_opt(s) {
                Ok(ParamValue::Bool(b))
            } else if let Ok(i) = s.parse::<i32>() {
                Ok(ParamValue::Int(i))
            } else if let Ok(f) = s.parse::<f32>() {
                Ok(ParamValue::Float(f))
            } else {
                bail!("could not parse '{s}' as bool/int/float; pass --type to force a type")
            }
        }
    }
}

/// Send an `/input/<NAME>` axis or button, picking the kind from the canonical name.
fn osc_input(args: &OscInputArgs) -> Result<()> {
    let client = sender_client(&args.target)?;
    if let Some(axis) = InputAxis::from_name(&args.name) {
        let v: f32 = args
            .value
            .parse()
            .with_context(|| format!("axis value '{}' must be a float in -1..1", args.value))?;
        client.send_axis(axis, v)?;
        println!("sent /input/{} = {v}", axis.name());
    } else if let Some(button) = InputButton::from_name(&args.name) {
        let pressed = parse_bool(&args.value)?;
        client.send_button(button, pressed)?;
        println!("sent /input/{} = {pressed}", button.name());
    } else {
        bail!(
            "'{}' is not a known VRChat input axis or button (e.g. Vertical, Horizontal, Jump, Voice)",
            args.name
        );
    }
    Ok(())
}

/// Ask VRChat to load a different avatar.
fn osc_change(args: &OscChangeArgs) -> Result<()> {
    let client = sender_client(&args.target)?;
    client.send_avatar_change(&args.id)?;
    println!("sent /avatar/change {}", args.id);
    Ok(())
}

/// Listen for the avatar parameters VRChat broadcasts and print each update until the optional
/// time budget elapses (or forever, until Ctrl-C).
fn osc_monitor(args: &OscMonitorArgs) -> Result<()> {
    // We only receive here; the send target is unused but the constructor needs one.
    let mut client = ParamClient::new(
        (args.host.as_str(), args.port),
        ("127.0.0.1", avatar_osc::VRCHAT_RECV_PORT),
    )
    .context("binding OSC monitor socket")?;

    match args.seconds {
        Some(s) => println!(
            "listening for VRChat parameters on {}:{} for {s}s…",
            args.host, args.port
        ),
        None => println!(
            "listening for VRChat parameters on {}:{} (Ctrl-C to stop)…",
            args.host, args.port
        ),
    }

    let deadline = args
        .seconds
        .map(|s| Instant::now() + Duration::from_secs(s));
    loop {
        for update in client.poll()? {
            println!("{} = {:?}", update.name, update.value);
        }
        if let Some(d) = deadline
            && Instant::now() >= d
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Parse an avatar's OSCQuery config JSON and list its `/avatar/parameters/*` entries.
fn osc_query(args: &OscQueryArgs) -> Result<()> {
    let config = AvatarConfig::from_path(&args.path)?;
    let params: Vec<_> = config.avatar_parameters().collect();

    if args.json {
        let view: Vec<_> = params
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "full_path": p.full_path,
                    "type": p.type_tag,
                    "readable": p.access.is_readable(),
                    "writable": p.access.is_writable(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": config.name,
                "parameters": view,
            }))?
        );
        return Ok(());
    }

    println!("OSCQuery config: {}", config.name);
    println!("  {} avatar parameter(s):\n", params.len());
    let (h_name, h_type, h_access) = ("Parameter", "Type", "Access");
    println!("  {h_name:<36} {h_type:<6} {h_access}");
    println!("  {} {} {}", "-".repeat(36), "-".repeat(6), "-".repeat(9));
    for p in &params {
        let name = p.name.as_deref().unwrap_or(&p.full_path);
        let access = access_label(p.access);
        println!("  {name:<36} {:<6} {access}", p.type_tag);
    }
    Ok(())
}

/// Run the analog-gesture daemon. There is no on-device analog input backend headless yet (OpenXR
/// is the production path), so this drives a deterministic synthetic trigger sweep — pull it up in a
/// running VRChat and watch the Fist gesture blend, end-to-end proof of the pipeline.
fn osc_gestures(args: &OscGesturesArgs) -> Result<()> {
    use avatar_osc_gestures::{DemoSource, GestureDaemon};

    if args.hz == 0 {
        bail!("--hz must be at least 1");
    }
    let client = sender_client(&args.target)?;
    let mut daemon = GestureDaemon::new(DemoSource::new(args.period));
    let period = Duration::from_secs_f64(1.0 / args.hz as f64);

    println!(
        "gesture daemon (demo): synthesizing a trigger sweep at {} Hz -> {} \
         (real input backend: OpenXR, pending)",
        args.hz,
        client.target()
    );
    match args.seconds {
        Some(s) => {
            println!("running for {s}s…");
            daemon.run_for(&client, period, Duration::from_secs(s))?;
            println!("done.");
            Ok(())
        }
        None => {
            println!("running until interrupted (Ctrl-C)…");
            daemon.run(&client, period)
        }
    }
}

/// A compact label for an OSCQuery access mode.
fn access_label(access: avatar_osc::Access) -> &'static str {
    match (access.is_readable(), access.is_writable()) {
        (true, true) => "read/write",
        (true, false) => "read",
        (false, true) => "write",
        (false, false) => "none",
    }
}

/// Parse a bool from the usual textual spellings (`true`/`false`, `1`/`0`, `on`/`off`).
fn parse_bool(s: &str) -> Result<bool> {
    parse_bool_opt(s).with_context(|| format!("'{s}' is not a boolean (try true/false)"))
}

fn parse_bool_opt(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}

fn fbx_inspect(args: &FileArgs) -> Result<()> {
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
fn armature_check(args: &FileArgs) -> Result<ExitCode> {
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

fn armature_fix(args: &FixArgs) -> Result<()> {
    let mut doc = FbxDocument::load(&args.path)?;
    let plan = plan_repairs(&doc.scene());

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print_plan(&args.path, &plan);
    }

    match &args.output {
        None => {
            if !args.json && plan.native().count() > 0 {
                println!("\n  (dry run — pass -o <file> to write the repaired FBX)");
            }
        }
        Some(out) => {
            if overwrites_input(&args.path, out) && !args.force {
                bail!(
                    "refusing to overwrite the input file {}; choose a different -o path or pass --force",
                    args.path.display()
                );
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
struct InspectSummary {
    version: u32,
    unit_scale_factor: Option<f64>,
    up_axis: Option<i32>,
    total_objects: usize,
    models: usize,
    geometries: usize,
    materials: usize,
    bone_like: usize,
    roots: Vec<String>,
}

fn inspect_summary(scene: &FbxScene) -> InspectSummary {
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

// ---------------------------------------------------------------------------
// `avatar unitypackage` — read/extract/cross-check `.unitypackage` archives.
// ---------------------------------------------------------------------------

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

fn up_info(args: &UpInfoArgs) -> Result<()> {
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

fn up_list(args: &UpListArgs) -> Result<()> {
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

fn up_extract(args: &UpExtractArgs) -> Result<()> {
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
fn up_testbed(args: &UpTestbedArgs) -> Result<ExitCode> {
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
