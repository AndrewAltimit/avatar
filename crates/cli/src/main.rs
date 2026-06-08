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
use cmd::fbx::{ArmatureCommand, FbxCommand};
use cmd::lint::LintArgs;
use cmd::osc::OscCommand;
use cmd::render::{RenderArgs, ViewArgs};
use cmd::stats::StatsArgs;
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
        Command::Armature(ArmatureCommand::Check(args)) => cmd::fbx::armature_check(&args),
        Command::Armature(ArmatureCommand::Fix(args)) => {
            cmd::fbx::armature_fix(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::Lint(args) => cmd::lint::lint(&args),
        Command::Stats(args) => cmd::stats::stats(&args).map(|()| ExitCode::SUCCESS),
        Command::AnimGen(AnimGenCommand::Blendtree(args)) => {
            cmd::anim_gen::blendtree(&args).map(|()| ExitCode::SUCCESS)
        }
        Command::AnimGen(AnimGenCommand::Clip(args)) => {
            cmd::anim_gen::clip(&args).map(|()| ExitCode::SUCCESS)
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
    }
}
