//! `avatar` — command-line entry point for the VRChat avatar tools.
//!
//! Subcommands:
//!   - `avatar describe <path>`           — one-shot snapshot: FBX structure+armature+perf, or project lint+perf
//!   - `avatar fbx inspect <path>`        — dump an FBX's structure and flag unit/orientation issues
//!   - `avatar fbx reslot <path> …`       — move a mesh region's polygons onto another material slot (writes FBX)
//!   - `avatar armature check <path>`     — validate the skeleton against VRChat humanoid requirements
//!   - `avatar armature fix <path> -o …`  — write a repaired FBX (canonical bone names, topology)
//!   - `avatar lint <project>`            — SDK3-compliance report over a Unity/VRChat project
//!   - `avatar stats <path>`              — VRChat performance ranking of an FBX or project avatar
//!   - `avatar anim-gen blendtree …`      — generate an analog-gesture FX blend-tree (Unity YAML)
//!   - `avatar anim-gen clip …`           — generate a `.anim` clip (blendshape / toggle curves)
//!   - `avatar anim-gen controller …`     — generate a complete FX `.controller` (full M4 asset)
//!   - `avatar anim-gen params|menu …`    — generate VRC expression parameters / menu `.asset`s
//!   - `avatar toggle --name N …`         — generate a full toggle bundle (clips+FX+params+menu)
//!   - `avatar migrate sdk3 <project> …`  — SDK2 avatar project -> SDK3 (descriptor, PhysBones, FX)
//!   - `avatar physbone list|set|split|stretch|flare|nudge …` — inspect / retune / split / lengthen / re-angle / shift a prefab's PhysBones
//!   - `avatar asset set <file> …`        — surgically edit a value in a Unity YAML asset (round-trip)
//!   - `avatar schema [name]`             — JSON Schema for a `--json` report type (output contract)
//!   - `avatar mcp serve`                 — expose the read/diagnose tools over MCP (stdio JSON-RPC)
//!   - `avatar osc send|input|monitor …`  — drive / observe a running VRChat over OSC
//!   - `avatar osc query <config.json>`   — list an avatar's parameters from its OSCQuery config
//!   - `avatar osc gestures`              — run the analog-gesture daemon (demo trigger sweep)
//!   - `avatar unitypackage info <pkg>`   — summarize a `.unitypackage` (contents, SDK, avatar/world)
//!   - `avatar unitypackage list <pkg>`   — list a package's assets (path, guid, size)
//!   - `avatar unitypackage extract …`    — extract a package into a Unity `Assets/` tree
//!   - `avatar unitypackage testbed …`    — cross-check an avatar package against a world/map package
//!
//! `main.rs` is just the top-level [`Cli`]/[`Command`] enum and the [`run`] dispatcher; each
//! command group's args, handlers, and formatters live in its [`cmd`] submodule.

use std::process::ExitCode;

mod cmd;
mod render_scene;
mod texture;
mod world;

use anyhow::Result;
use clap::{Parser, Subcommand};

use cmd::anim_gen::AnimGenCommand;
use cmd::asset::AssetCommand;
use cmd::describe::DescribeArgs;
use cmd::fbx::{ArmatureCommand, FbxCommand};
use cmd::lint::LintArgs;
use cmd::mcp::McpCommand;
use cmd::migrate::MigrateCommand;
use cmd::osc::OscCommand;
use cmd::physbone::PhysBoneCommand;
use cmd::render::{RenderArgs, ViewArgs};
use cmd::schema::SchemaArgs;
use cmd::stats::StatsArgs;
use cmd::toggle::ToggleArgs;
use cmd::unitypackage::UnitypackageCommand;

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

// Built exactly once from argv; the size spread between arg structs is irrelevant here.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum Command {
    /// Inspect and operate on FBX files.
    #[command(subcommand)]
    Fbx(FbxCommand),
    /// Inspect and validate avatar armatures / skeletons.
    #[command(subcommand)]
    Armature(ArmatureCommand),
    /// Summarize an avatar asset in one shot: FBX structure+armature+geometry, or project lint+perf.
    Describe(DescribeArgs),
    /// Lint a Unity/VRChat project for SDK3 compliance.
    Lint(LintArgs),
    /// Estimate the VRChat performance ranking of an FBX or a project's avatar(s).
    Stats(StatsArgs),
    /// Generate Unity animation assets (`.anim` clips, FX analog-gesture blend trees).
    #[command(subcommand, name = "anim-gen")]
    AnimGen(AnimGenCommand),
    /// Generate a complete toggle bundle: On/Off clips, FX controller, expression params + menu.
    Toggle(ToggleArgs),
    /// Surgically edit a value in a Unity YAML asset, preserving fileIDs/refs/formatting.
    #[command(subcommand)]
    Asset(AssetCommand),
    /// Migrate an avatar project between VRChat SDK generations (SDK2 -> SDK3 / Avatars 3.0).
    #[command(subcommand)]
    Migrate(MigrateCommand),
    /// Inspect and retune the VRCPhysBone components of an SDK3 prefab (list/set/split/stretch).
    #[command(subcommand)]
    Physbone(PhysBoneCommand),
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
    /// Print the JSON Schema for a `--json` report type (for agents consuming the output).
    Schema(SchemaArgs),
    /// Run a Model Context Protocol server exposing the read/diagnose tools to an agent host.
    #[command(subcommand)]
    Mcp(McpCommand),
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
        Command::Fbx(FbxCommand::Inspect(args)) => {
            cmd::fbx::inspect(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Fbx(FbxCommand::Reslot(args)) => {
            cmd::fbx::reslot(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Fbx(FbxCommand::Blendshapes(args)) => {
            cmd::fbx::blendshapes(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Armature(ArmatureCommand::Check(args)) => cmd::fbx::armature_check(&args),
        Command::Armature(ArmatureCommand::Fix(args)) => {
            cmd::fbx::armature_fix(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Describe(args) => cmd::describe::describe(&args),
        Command::Lint(args) => cmd::lint::lint(&args),
        Command::Stats(args) => cmd::stats::stats(&args).map(|()| ExitCode::SUCCESS),
        Command::AnimGen(AnimGenCommand::Blendtree(args)) => {
            cmd::anim_gen::blendtree(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Clip(args)) => {
            cmd::anim_gen::clip(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Controller(args)) => {
            cmd::anim_gen::controller(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Params(args)) => {
            cmd::anim_gen::params(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Puppet(args)) => {
            cmd::anim_gen::puppet(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Menu(args)) => {
            cmd::anim_gen::menu(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Toggle(args) => cmd::toggle::toggle(&args).map(|()| ExitCode::SUCCESS),
        Command::Asset(AssetCommand::Set(args)) => {
            cmd::asset::set(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Migrate(MigrateCommand::Sdk3(args)) => {
            cmd::migrate::sdk3(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::List(args)) => {
            cmd::physbone::list(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::Set(args)) => {
            cmd::physbone::set(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::Split(args)) => {
            cmd::physbone::split(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::Stretch(args)) => {
            cmd::physbone::stretch(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::Flare(args)) => {
            cmd::physbone::flare(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Physbone(PhysBoneCommand::Nudge(args)) => {
            cmd::physbone::nudge(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Osc(OscCommand::Send(args)) => cmd::osc::send(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Input(args)) => cmd::osc::input(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Monitor(args)) => {
            cmd::osc::monitor(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Osc(OscCommand::Change(args)) => {
            cmd::osc::change(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Osc(OscCommand::Query(args)) => cmd::osc::query(&args).map(|()| ExitCode::SUCCESS),
        Command::Osc(OscCommand::Gestures(args)) => {
            cmd::osc::gestures(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::Info(args)) => {
            cmd::unitypackage::info(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::List(args)) => {
            cmd::unitypackage::list(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::Extract(args)) => {
            cmd::unitypackage::extract(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Unitypackage(UnitypackageCommand::Testbed(args)) => {
            cmd::unitypackage::testbed(&args)
        }
        Command::Render(args) => cmd::render::render(&args).map(|()| ExitCode::SUCCESS),
        Command::View(args) => cmd::render::view(&args).map(|()| ExitCode::SUCCESS),
        Command::Schema(args) => cmd::schema::schema(&args).map(|()| ExitCode::SUCCESS),
        Command::Mcp(McpCommand::Serve(args)) => cmd::mcp::serve(&args).map(|()| ExitCode::SUCCESS),
    }
}
