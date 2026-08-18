//! `avatar migrate sdk3` — rewrite an SDK2 avatar project into an SDK3 (Avatars 3.0) one:
//! descriptor, PhysBones, FX from gesture overrides, clutter removal, and a fresh project tree
//! around the migrated prefab. Behaviour lives in `avatar-migrate`; this is the arg surface.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_migrate::eyelook::EyeLookAngles;
use avatar_migrate::{MigrateOptions, MigrationReport, PhysBoneRootSpec, migrate, summarize};
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum MigrateCommand {
    /// Migrate an SDK2 avatar project (extracted `.unitypackage`) to SDK3 / Avatars 3.0.
    Sdk3(MigrateSdk3Args),
}

#[derive(Args, Debug)]
pub struct MigrateSdk3Args {
    /// Source project root (the directory containing `Assets/`), e.g. an `avatar unitypackage
    /// extract` output.
    project: PathBuf,
    /// Output project directory (created; must not already have an `Assets/`).
    #[arg(short, long, value_name = "DIR")]
    output: PathBuf,
    /// Name for the migrated prefab and its `<NAME>_SDK3/` folder of generated assets.
    #[arg(long)]
    name: String,
    /// The SDK2 avatar prefab (Assets-relative). Default: the only prefab with an SDK2 descriptor.
    #[arg(long, value_name = "PATH")]
    prefab: Option<PathBuf>,
    /// Remove a GameObject and its whole subtree (name or `A/B/C` path). Repeatable.
    #[arg(long, value_name = "NAME|PATH")]
    strip: Vec<String>,
    /// Remove Unity `Cloth` components (a Cloth skirt/cape becomes plain skinning).
    #[arg(long)]
    drop_cloth: bool,
    /// Retype Unity `CapsuleCollider`s (Cloth support) as `VRCPhysBoneCollider`s.
    #[arg(long)]
    capsules_to_physbone_colliders: bool,
    /// Add a PhysBone chain: `ROOT|IGNORE1,IGNORE2|COLLIDER_OBJ1,COLLIDER_OBJ2` (names or
    /// paths; ignore/collider parts optional — no collider list = every converted capsule).
    /// Repeatable. Example: `Hips|Spine,Left leg,Right leg` for a skirt hanging off Hips.
    #[arg(long = "physbone", value_name = "SPEC")]
    physbones: Vec<String>,
    /// Eye bones as `LEFT,RIGHT` (names or paths) — enables SDK3 eye look derived from the rig.
    #[arg(long, value_name = "LEFT,RIGHT")]
    eyes: Option<String>,
    /// Eye-look angles in degrees as `UP,DOWN,LEFT,RIGHT` (default 10,10,12,12).
    #[arg(long, value_name = "U,D,L,R")]
    eye_angles: Option<String>,
    /// Blink blendshape name on the viseme mesh (default: auto-detect `Blink`/`blink`/…).
    #[arg(long, value_name = "SHAPE")]
    blink: Option<String>,
    /// Don't build an FX layer from the SDK2 gesture overrides.
    #[arg(long)]
    no_fx: bool,
    /// Assets-relative directory not to copy into the output. Repeatable. `VRCSDK` and
    /// `VRChat Examples` (SDK2) are always excluded.
    #[arg(long, value_name = "DIR")]
    exclude: Vec<String>,
    /// Bundle a VPM package (a directory with `package.json`, or a `.zip` of one — e.g. a shader
    /// package's release zip) into the output project's `Packages/`. Repeatable.
    #[arg(long = "vpm-package", value_name = "PATH")]
    vpm_packages: Vec<PathBuf>,
    /// Re-point materials sitting on a locker's generated `Hidden/…` shader copy back to their
    /// original shader (by the material's `OriginalShader` tag; found among the project's shaders
    /// and bundled packages), and drop the generated copies.
    #[arg(long)]
    relink_locked_shaders: bool,
    /// `com.vrchat.avatars` version to pin in the output `vpm-manifest.json`.
    #[arg(long, default_value = "3.10.4")]
    sdk_version: String,
    /// Unity editor version for the output `ProjectVersion.txt`.
    #[arg(long, default_value = "2022.3.22f1")]
    unity_version: String,
    /// Plan and report without writing anything.
    #[arg(long)]
    dry_run: bool,
    /// Print the full machine-readable JSON report on stdout.
    #[arg(long)]
    json: bool,
}

pub fn sdk3(args: &MigrateSdk3Args) -> Result<()> {
    let mut opts = MigrateOptions::new(&args.project, &args.output, &args.name);
    opts.prefab = args.prefab.clone();
    opts.strip = args.strip.clone();
    opts.drop_cloth = args.drop_cloth;
    opts.capsules_to_physbone_colliders = args.capsules_to_physbone_colliders;
    for s in &args.physbones {
        opts.physbone_roots
            .push(PhysBoneRootSpec::parse(s).with_context(|| format!("--physbone {s}"))?);
    }
    if let Some(e) = &args.eyes {
        let (l, r) = e.split_once(',').context("--eyes expects LEFT,RIGHT")?;
        opts.eye_bones = Some((l.trim().to_string(), r.trim().to_string()));
    }
    if let Some(a) = &args.eye_angles {
        let v: Vec<f64> = a
            .split(',')
            .map(|x| x.trim().parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .context("--eye-angles expects four numbers U,D,L,R")?;
        if v.len() != 4 {
            bail!("--eye-angles expects four numbers U,D,L,R");
        }
        opts.eye_look_angles = EyeLookAngles {
            up: v[0],
            down: v[1],
            left: v[2],
            right: v[3],
        };
    }
    opts.blink_shape = args.blink.clone();
    opts.fx_from_overrides = !args.no_fx;
    for e in &args.exclude {
        let e = e.trim_start_matches("Assets/").to_string();
        if !opts.exclude.contains(&e) {
            opts.exclude.push(e);
        }
    }
    opts.vpm_packages = args.vpm_packages.clone();
    opts.relink_locked_shaders = args.relink_locked_shaders;
    opts.sdk_version = args.sdk_version.clone();
    opts.unity_version = args.unity_version.clone();
    opts.dry_run = args.dry_run;

    let report = migrate(&opts)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn print_report(r: &MigrationReport) {
    let mode = if r.dry_run {
        "dry run — nothing written"
    } else {
        "written"
    };
    println!("SDK2 -> SDK3 migration ({mode})");
    println!("  source prefab : {}", r.source_prefab);
    println!(
        "  avatar root   : {} (scale {:?})",
        r.avatar_root, r.root_scale
    );
    println!(
        "  output        : {} / {}",
        r.output_project, r.output_prefab
    );
    if let Some(vp) = r.view_position {
        println!("  view position : ({}, {}, {})", vp[0], vp[1], vp[2]);
    }
    if !r.stripped.is_empty() {
        println!("  stripped      : {}", r.stripped.join(", "));
    }
    if !r.converted.is_empty() {
        println!("  converted:");
        for c in &r.converted {
            let at = if c.object_path.is_empty() {
                String::new()
            } else {
                format!(" @ {}", c.object_path)
            };
            println!("    - {}{at}", c.what);
        }
    }
    if !r.added.is_empty() {
        println!("  added:");
        for c in &r.added {
            println!("    - {} @ {}", c.what, c.object_path);
        }
    }
    if !r.removed.is_empty() {
        println!("  removed:");
        for c in &r.removed {
            println!("    - {} @ {}", c.what, c.object_path);
        }
    }
    if let Some(e) = &r.eye_look {
        println!("  eye look      : {e}");
    }
    if let Some((name, idx)) = &r.blink_blendshape {
        println!("  blink shape   : {name} (index {idx})");
    }
    if let Some(fx) = &r.fx {
        println!(
            "  FX gestures   : {} migrated, {} slot(s) skipped",
            fx.gestures.len(),
            fx.skipped.len()
        );
        for g in &fx.gestures {
            println!(
                "    - {} <- {} ({} blendshape(s), {} curve(s) dropped)",
                g.gesture_name,
                g.source_clip,
                g.blendshapes.len(),
                g.dropped_curves
            );
        }
    }
    println!(
        "  files         : {} generated, {} copied, {} skipped, {} deduped against bundled packages",
        r.generated.len(),
        r.assets_copied,
        r.assets_skipped,
        r.assets_deduped
    );
    for (name, version) in &r.bundled_packages {
        println!("  bundled       : {name} {version}");
    }
    if !r.relinked_materials.is_empty() {
        println!("  relinked materials:");
        for m in &r.relinked_materials {
            println!(
                "    - {} : '{}' -> '{}'",
                m.material, m.original_shader, m.relinked_to
            );
        }
    }
    if !r.warnings.is_empty() {
        println!("  warnings:");
        for w in &r.warnings {
            println!("    ! {w}");
        }
    }
    println!("  next steps:");
    for s in &r.next_steps {
        println!("    * {s}");
    }
    println!("{}", summarize(r));
}
