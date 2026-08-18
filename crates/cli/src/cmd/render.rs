//! `avatar render` (offscreen PNG) and `avatar view` (interactive window) — assemble an avatar
//! and/or world scene and draw it via the GPU pipeline.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::Args;

use crate::render_scene::{AvatarPose, BoneStretch};
use crate::{render_scene, texture};

#[derive(Args, Debug)]
pub struct ViewArgs {
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
    /// Preview a chain-length change: `HINGE:FACTOR` scales the offsets of every bone below the
    /// bones named HINGE (`*` wildcards) — what `avatar physbone stretch` does to the prefab —
    /// e.g. `Skirt_0_*:1.5`. Repeatable. FBX avatars only.
    #[arg(long, value_name = "HINGE:FACTOR", value_parser = BoneStretch::parse)]
    stretch: Vec<BoneStretch>,
    /// Pose the avatar from a Unity prefab: every bone's local transform is taken from the
    /// GameObject of the same name (Unity's mirrored import undone), so the render shows what
    /// the prefab's transforms — stretched/re-angled chains, posed bones — will look like in
    /// Unity. FBX avatars only.
    #[arg(long, value_name = "PREFAB")]
    pose: Option<PathBuf>,
}

#[derive(Args, Debug)]
pub struct RenderArgs {
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
    /// Preview a chain-length change: `HINGE:FACTOR` scales the offsets of every bone below the
    /// bones named HINGE (`*` wildcards) — what `avatar physbone stretch` does to the prefab —
    /// e.g. `Skirt_0_*:1.5`. Repeatable. FBX avatars only.
    #[arg(long, value_name = "HINGE:FACTOR", value_parser = BoneStretch::parse)]
    stretch: Vec<BoneStretch>,
    /// Pose the avatar from a Unity prefab: every bone's local transform is taken from the
    /// GameObject of the same name (Unity's mirrored import undone), so the render shows what
    /// the prefab's transforms — stretched/re-angled chains, posed bones — will look like in
    /// Unity. FBX avatars only.
    #[arg(long, value_name = "PREFAB")]
    pose: Option<PathBuf>,
}

/// Camera framing target for `avatar render`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum FrameTarget {
    /// Frame on the avatar, with the surrounding map visible.
    Avatar,
    /// Frame on the entire scene's bounds.
    World,
}

/// Build the renderable [`avatar_render::Scene`]: load the world (if any), drop the avatar at the
/// world's player-spawn point at human scale (or render it standalone), then frame the camera. The
/// `width`/`height` set the framing aspect. Shared by `render` (offscreen PNG) and `view`
/// (interactive window); prints a short progress summary as it goes.
#[allow(clippy::too_many_arguments)]
fn assemble_scene(
    avatar: Option<&Path>,
    world: Option<&Path>,
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    frame: Option<FrameTarget>,
    how: &AvatarPose,
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
                let (av, bounds) =
                    render_scene::load_avatar_in_world(avatar, p, &mut textures, how)?;
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
                let av = render_scene::load_avatar(avatar, &mut textures, how)?;
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
pub fn render(args: &RenderArgs) -> Result<()> {
    let scene = assemble_scene(
        args.avatar.as_deref(),
        args.world.as_deref(),
        args.width,
        args.height,
        args.yaw,
        args.pitch,
        args.frame,
        &AvatarPose {
            stretch: args.stretch.clone(),
            pose_prefab: args.pose.clone(),
        },
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
pub fn view(args: &ViewArgs) -> Result<()> {
    let scene = assemble_scene(
        args.avatar.as_deref(),
        args.world.as_deref(),
        args.width,
        args.height,
        args.yaw,
        args.pitch,
        args.frame,
        &AvatarPose {
            stretch: args.stretch.clone(),
            pose_prefab: args.pose.clone(),
        },
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
