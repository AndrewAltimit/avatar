//! `avatar fbx` and `avatar armature` — inspect FBX structure and validate/repair the armature.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use avatar_armature::{RepairPlan, apply_plan, plan_repairs};
use avatar_fbx::{FbxDocument, FbxScene};
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum FbxCommand {
    /// Print the structure of an FBX file (objects, hierarchy, global settings).
    Inspect(FileArgs),
    /// Move a region of a mesh's polygons onto another material slot (e.g. a glowing hair patch
    /// onto the plain black slot), selecting by bone proximity / height / texture brightness.
    Reslot(ReslotArgs),
    /// List blendshape channels and the material slots each one's target vertices render with —
    /// which material an emote shape (a blush patch a shape slides into view) actually uses.
    Blendshapes(BlendshapesArgs),
}

#[derive(Args, Debug)]
pub struct BlendshapesArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// Only channels whose name contains this (case-insensitive).
    #[arg(long)]
    filter: Option<String>,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(serde::Serialize)]
struct BlendshapeReport {
    channel: String,
    mesh: Option<String>,
    /// Control points the shape moves (empty if the exporter wrote no index array).
    target_vertices: usize,
    /// Material slots those vertices' triangles render with: `(slot, material name, triangles)`.
    slots: Vec<BlendshapeSlot>,
}

#[derive(serde::Serialize)]
struct BlendshapeSlot {
    slot: u32,
    material: String,
    triangles: usize,
}

pub fn blendshapes(args: &BlendshapesArgs) -> Result<()> {
    let doc = FbxDocument::load(&args.path)?;
    let scene = doc.scene();
    let meshes = doc.meshes()?;
    let mut reports: Vec<BlendshapeReport> = Vec::new();
    for ch in scene.blendshape_channels() {
        if let Some(f) = &args.filter
            && !ch.name.to_lowercase().contains(&f.to_lowercase())
        {
            continue;
        }
        let targets: HashSet<u32> = doc
            .blendshape_target_indexes(&ch.name)?
            .into_iter()
            .collect();
        // Count, per material slot, the triangles of the channel's mesh that touch a target
        // control point.
        let mesh = meshes.iter().find(|m| {
            scene
                .object(m.model_id)
                .map(|o| Some(&o.name) == ch.mesh_model_name.as_ref())
                .unwrap_or(false)
        });
        let mut slot_tris: std::collections::BTreeMap<u32, usize> = Default::default();
        if let Some(m) = mesh {
            for tri in 0..m.indices.len() / 3 {
                let touched = (0..3).any(|c| {
                    let v = m.indices[tri * 3 + c] as usize;
                    targets.contains(&m.control_point_of_vertex[v])
                });
                if touched {
                    *slot_tris
                        .entry(m.triangle_material(tri) as u32)
                        .or_default() += 1;
                }
            }
        }
        let slots = slot_tris
            .into_iter()
            .map(|(slot, triangles)| BlendshapeSlot {
                slot,
                material: mesh
                    .and_then(|m| m.materials.get(slot as usize))
                    .map(|mat| mat.name.clone())
                    .unwrap_or_default(),
                triangles,
            })
            .collect();
        reports.push(BlendshapeReport {
            channel: ch.name.clone(),
            mesh: ch.mesh_model_name.clone(),
            target_vertices: targets.len(),
            slots,
        });
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
        return Ok(());
    }
    println!(
        "Blendshapes: {} ({} channel(s))",
        args.path.display(),
        reports.len()
    );
    for r in &reports {
        println!(
            "  {:<28} mesh {:<8} {:>5} vert(s)  slots: {}",
            r.channel,
            r.mesh.as_deref().unwrap_or("?"),
            r.target_vertices,
            if r.slots.is_empty() {
                "(none)".to_string()
            } else {
                r.slots
                    .iter()
                    .map(|s| format!("{} ({}, {} tris)", s.slot, s.material, s.triangles))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );
    }
    Ok(())
}

#[derive(Args, Debug)]
pub struct ReslotArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// The mesh `Model` node to edit, by name (or object id).
    #[arg(long)]
    mesh: String,
    /// Destination material slot (index into the mesh's material list; `avatar fbx inspect` /
    /// the Unity renderer's material list show the order).
    #[arg(long)]
    to_slot: u32,
    /// Only polygons currently on this slot.
    #[arg(long)]
    from_slot: Option<u32>,
    /// Only polygons whose triangles' centroids lie within `--radius` of this bone's bind
    /// position (bone by name).
    #[arg(long, requires = "radius")]
    near_bone: Option<String>,
    /// Test each triangle by its *nearest corner* (and highest/lowest corner for `--min-z` /
    /// `--max-z`) instead of its centroid — catches long, thin strand triangles whose centroid
    /// lies far from where they start.
    #[arg(long)]
    any_corner: bool,
    /// Radius (mesh units) for `--near-bone`.
    #[arg(long)]
    radius: Option<f32>,
    /// Only triangles whose centroid Z (mesh space, the file's up axis for Blender exports) is
    /// at least this.
    #[arg(long)]
    min_z: Option<f32>,
    /// Only triangles whose centroid Z is at most this.
    #[arg(long)]
    max_z: Option<f32>,
    /// Skip triangles that are skinned (weight ≥ `--exclude-weight`) to a bone matching this
    /// name glob (`*` allowed) — e.g. keep an ahoge strand as it is.
    #[arg(long, value_name = "GLOB")]
    exclude_bone: Option<String>,
    /// Weight threshold for `--exclude-bone`.
    #[arg(long, default_value_t = 0.5)]
    exclude_weight: f32,
    /// Only triangles whose UV centroid samples brighter than LUM (0..255 mean of RGB) in this
    /// texture — "the ones lit by this emission map".
    #[arg(long, value_name = "TEXTURE.png:LUM")]
    brighter_than: Option<String>,
    /// Write the edited FBX here. Without this, runs as a dry run (prints the selection only).
    #[arg(short, long)]
    output: Option<std::path::PathBuf>,
    /// Allow `--output` to overwrite the input file or any existing output file.
    #[arg(long)]
    force: bool,
    /// Emit a machine-readable JSON report.
    #[arg(long)]
    json: bool,
    /// Also write a PNG mask (size `--uv-mask-size`) of the selected triangles' UV footprint —
    /// white where they sample — and report every *unselected* triangle on the same slot that
    /// overlaps it (they would be hit too by any texture edit under the mask). The texture-side
    /// alternative when the FBX can't be rewritten.
    #[arg(long, value_name = "MASK.png")]
    uv_mask: Option<std::path::PathBuf>,
    /// Mask resolution (square).
    #[arg(long, default_value_t = 1024)]
    uv_mask_size: u32,
}

/// Visit every pixel a UV triangle (pixel coords, y down) covers, with a half-pixel tolerance so
/// thin slivers still register.
fn for_each_tri_pixel(w: u32, h: u32, p: [(f32, f32); 3], mut f: impl FnMut(u32, u32)) {
    let min_x = p
        .iter()
        .map(|q| q.0)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i64;
    let max_x = (p
        .iter()
        .map(|q| q.0)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i64)
        .min(w as i64 - 1);
    let min_y = p
        .iter()
        .map(|q| q.1)
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as i64;
    let max_y = (p
        .iter()
        .map(|q| q.1)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i64)
        .min(h as i64 - 1);
    let edge = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (c.0 - a.0) * (b.1 - a.1) - (c.1 - a.1) * (b.0 - a.0)
    };
    let area = edge(p[0], p[1], p[2]);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            // Sample the pixel centre, with a half-pixel tolerance so thin slivers still mark.
            let c = (x as f32 + 0.5, y as f32 + 0.5);
            let (e0, e1, e2) = (
                edge(p[1], p[2], c),
                edge(p[2], p[0], c),
                edge(p[0], p[1], c),
            );
            let tol = 0.75 * (area.abs().sqrt().max(1.0));
            let inside = if area >= 0.0 {
                e0 >= -tol && e1 >= -tol && e2 >= -tol
            } else {
                e0 <= tol && e1 <= tol && e2 <= tol
            };
            if inside {
                f(x as u32, y as u32);
            }
        }
    }
}

/// Rasterize a UV triangle into `mask` (row-major `w*h`).
fn raster_tri(mask: &mut [u8], w: u32, h: u32, p: [(f32, f32); 3]) {
    for_each_tri_pixel(w, h, p, |x, y| mask[(y * w + x) as usize] = 255);
}

/// `*`-wildcard match (no other metacharacters).
fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(part) else {
                return false;
            };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if part.is_empty() {
            continue;
        } else {
            let Some(pos) = rest.find(part) else {
                return false;
            };
            rest = &rest[pos + part.len()..];
        }
    }
    true
}

pub fn reslot(args: &ReslotArgs) -> Result<()> {
    use avatar_armature::Skeleton;
    let mut doc = FbxDocument::load(&args.path)?;
    let scene = doc.scene();
    let meshes = doc.meshes()?;
    let model_id = match args.mesh.parse::<i64>() {
        Ok(id) => id,
        Err(_) => scene
            .models()
            .find(|o| o.name == args.mesh)
            .map(|o| o.id)
            .with_context(|| format!("no Model named '{}'", args.mesh))?,
    };
    let mesh = meshes
        .iter()
        .find(|m| m.model_id == model_id)
        .with_context(|| format!("model '{}' has no geometry", args.mesh))?;
    if (args.to_slot as usize) >= mesh.material_slot_count().max(1) {
        bail!(
            "--to-slot {} out of range: mesh has {} material slot(s) ({})",
            args.to_slot,
            mesh.material_slot_count(),
            mesh.materials
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if mesh.polygon_of_triangle.is_empty() {
        bail!("mesh has no polygon map (not an FBX geometry?)");
    }
    let skeleton = Skeleton::from_scene(&scene);
    // Bone bind position (mesh space): G⁻¹ · TransformLink of its cluster on this mesh.
    let near = match (&args.near_bone, args.radius) {
        (Some(name), Some(r)) => {
            let bone = skeleton
                .bones
                .iter()
                .find(|b| b.name == *name)
                .with_context(|| format!("no bone named '{name}'"))?;
            let cluster = mesh
                .skin
                .as_ref()
                .and_then(|s| s.clusters.iter().find(|c| c.bone_id == bone.id))
                .with_context(|| format!("bone '{name}' does not skin this mesh"))?;
            let g = avatar_pose::model_global_matrix(&scene, mesh.model_id);
            let link = avatar_pose::mat4_from_fbx(&cluster.transform_link);
            let base = (g.inverse() * link).w_axis.truncate();
            Some((base, r))
        }
        _ => None,
    };
    // Control points excluded by bone weight.
    let mut excluded_cp: HashSet<usize> = HashSet::new();
    if let (Some(glob), Some(skin)) = (&args.exclude_bone, &mesh.skin) {
        for c in &skin.clusters {
            let name = skeleton
                .bone(c.bone_id)
                .map(|b| b.name.as_str())
                .unwrap_or("");
            if glob_match(glob, name) {
                for (&cp, &w) in c.indexes.iter().zip(&c.weights) {
                    if w >= args.exclude_weight {
                        excluded_cp.insert(cp as usize);
                    }
                }
            }
        }
    }
    // Brightness probe.
    let bright = match &args.brighter_than {
        Some(spec) => {
            let (tex, lum) = spec
                .rsplit_once(':')
                .context("--brighter-than must be TEXTURE:LUM")?;
            let lum: u32 = lum.trim().parse().context("--brighter-than LUM")?;
            let img = image::open(tex.trim())
                .with_context(|| format!("loading texture {tex}"))?
                .to_rgba8();
            Some((img, lum))
        }
        None => None,
    };
    if bright.is_some() && mesh.uvs.is_none() {
        bail!("--brighter-than needs UVs, and this mesh has none");
    }

    let n_tri = mesh.indices.len() / 3;
    let mut polys: std::collections::BTreeSet<u32> = Default::default();
    let mut selected_tris = 0usize;
    let mut selected: Vec<usize> = Vec::new();
    for t in 0..n_tri {
        if let Some(from) = args.from_slot
            && mesh.triangle_material(t) as u32 != from
        {
            continue;
        }
        let vi = [
            mesh.indices[t * 3] as usize,
            mesh.indices[t * 3 + 1] as usize,
            mesh.indices[t * 3 + 2] as usize,
        ];
        if vi
            .iter()
            .any(|&v| excluded_cp.contains(&(mesh.control_point_of_vertex[v] as usize)))
        {
            continue;
        }
        let corners: Vec<glam::Vec3> = vi
            .iter()
            .map(|&v| glam::Vec3::from_array(mesh.positions[v]))
            .collect();
        let c = corners.iter().copied().sum::<glam::Vec3>() / 3.0;
        let (dist, z_hi, z_lo) = if args.any_corner {
            (
                corners
                    .iter()
                    .map(|p| near.map_or(0.0, |(b, _)| p.distance(b)))
                    .fold(f32::INFINITY, f32::min),
                corners
                    .iter()
                    .map(|p| p.z)
                    .fold(f32::NEG_INFINITY, f32::max),
                corners.iter().map(|p| p.z).fold(f32::INFINITY, f32::min),
            )
        } else {
            (near.map_or(0.0, |(b, _)| c.distance(b)), c.z, c.z)
        };
        if let Some((_, r)) = near
            && dist > r
        {
            continue;
        }
        if let Some(z) = args.min_z
            && z_hi < z
        {
            continue;
        }
        if let Some(z) = args.max_z
            && z_lo > z
        {
            continue;
        }
        if let Some((img, lum)) = &bright {
            // Brightest of the three corners and the centroid: a triangle straddling a
            // gradient counts if any part of it is lit.
            let uvs = mesh.uvs.as_ref().unwrap();
            let (w, h) = img.dimensions();
            let sample = |u: f32, v: f32| -> u32 {
                let px = ((u.rem_euclid(1.0)) * w as f32) as u32;
                let py = ((1.0 - v.rem_euclid(1.0)) * h as f32) as u32;
                let p = img.get_pixel(px.min(w - 1), py.min(h - 1));
                (p[0] as u32 + p[1] as u32 + p[2] as u32) / 3
            };
            let (mut u, mut v) = (0.0f32, 0.0f32);
            let mut best = 0;
            for &i in &vi {
                u += uvs[i][0];
                v += uvs[i][1];
                best = best.max(sample(uvs[i][0], uvs[i][1]));
            }
            best = best.max(sample(u / 3.0, v / 3.0));
            if best <= *lum {
                continue;
            }
        }
        selected_tris += 1;
        selected.push(t);
        polys.insert(mesh.polygon_of_triangle[t]);
    }
    let changes: Vec<(u32, u32)> = polys.iter().map(|&p| (p, args.to_slot)).collect();
    // UV footprint mask + overlap report.
    let mut overlap_report: Vec<serde_json::Value> = Vec::new();
    if let Some(mask_path) = &args.uv_mask {
        let uvs = mesh
            .uvs
            .as_ref()
            .context("--uv-mask needs UVs, and this mesh has none")?;
        let (w, h) = (args.uv_mask_size, args.uv_mask_size);
        let px_of = |v: usize| -> (f32, f32) {
            (
                uvs[v][0].rem_euclid(1.0) * w as f32,
                (1.0 - uvs[v][1].rem_euclid(1.0)) * h as f32,
            )
        };
        let tri_px = |t: usize| -> [(f32, f32); 3] {
            [
                px_of(mesh.indices[t * 3] as usize),
                px_of(mesh.indices[t * 3 + 1] as usize),
                px_of(mesh.indices[t * 3 + 2] as usize),
            ]
        };
        let mut mask = vec![0u8; (w * h) as usize];
        for &t in &selected {
            raster_tri(&mut mask, w, h, tri_px(t));
        }
        // Unselected triangles overlapping the mask (their own footprint hits a marked pixel).
        let sel: HashSet<usize> = selected.iter().copied().collect();
        let mut hits = 0usize;
        for t in 0..n_tri {
            if sel.contains(&t) {
                continue;
            }
            // Only triangles that draw with the same material matter (a different slot samples
            // a different texture, so it can't be hit by an edit under this mask).
            if let Some(from) = args.from_slot
                && mesh.triangle_material(t) as u32 != from
            {
                continue;
            }
            let mut shared = 0usize;
            for_each_tri_pixel(w, h, tri_px(t), |x, y| {
                if mask[(y * w + x) as usize] > 0 {
                    shared += 1;
                }
            });
            if shared > 0 {
                hits += 1;
                if overlap_report.len() < 400 {
                    let vi = [
                        mesh.indices[t * 3] as usize,
                        mesh.indices[t * 3 + 1] as usize,
                        mesh.indices[t * 3 + 2] as usize,
                    ];
                    let c = vi
                        .iter()
                        .map(|&v| glam::Vec3::from_array(mesh.positions[v]))
                        .sum::<glam::Vec3>()
                        / 3.0;
                    overlap_report.push(serde_json::json!({
                        "triangle": t,
                        "slot": mesh.triangle_material(t),
                        "shared_px": shared,
                        "centroid": [c.x, c.y, c.z],
                    }));
                }
            }
        }
        image::GrayImage::from_raw(w, h, mask)
            .context("mask buffer")?
            .save(mask_path)
            .with_context(|| format!("writing {}", mask_path.display()))?;
        if !args.json {
            println!(
                "  uv mask -> {} ({hits} unselected triangle(s) overlap the footprint)",
                mask_path.display()
            );
            for o in overlap_report.iter().take(12) {
                println!("    overlap: {o}");
            }
        }
        overlap_report.insert(0, serde_json::json!({ "overlapping_triangles": hits }));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": args.path,
                "mesh": args.mesh,
                "model_id": model_id,
                "to_slot": args.to_slot,
                "to_material": mesh.materials.get(args.to_slot as usize).map(|m| m.name.clone()),
                "selected_triangles": selected_tris,
                "polygons": polys.iter().copied().collect::<Vec<_>>(),
                "output": args.output,
                "uv_mask": args.uv_mask,
                "uv_mask_overlap": overlap_report,
            }))?
        );
    } else {
        println!(
            "mesh '{}' (model {model_id}): {selected_tris} triangle(s) / {} polygon(s) selected -> slot {} ({})",
            args.mesh,
            polys.len(),
            args.to_slot,
            mesh.materials
                .get(args.to_slot as usize)
                .map(|m| m.name.as_str())
                .unwrap_or("?")
        );
    }
    let Some(out) = &args.output else {
        if !args.json {
            println!("  (dry run — pass -o <file> to write the edited FBX)");
        }
        return Ok(());
    };
    if polys.is_empty() {
        bail!("nothing selected; not writing");
    }
    if !args.force {
        if overwrites_input(&args.path, out) {
            bail!(
                "refusing to overwrite the input file {}; choose a different -o path or pass --force",
                args.path.display()
            );
        }
        if out.exists() {
            bail!(
                "refusing to overwrite existing file {} (pass --force to overwrite)",
                out.display()
            );
        }
    }
    let changed = doc.set_polygon_materials(model_id, &changes)?;
    doc.write(out)?;
    if !args.json {
        println!(
            "  {changed} polygon(s) changed slot; wrote {}",
            out.display()
        );
    }
    Ok(())
}

#[derive(Subcommand, Debug)]
pub enum ArmatureCommand {
    /// Validate the skeleton against VRChat humanoid rig requirements.
    Check(FileArgs),
    /// Plan (and optionally write) repairs: canonical humanoid bone names + parent topology.
    Fix(FixArgs),
}

#[derive(Args, Debug)]
pub struct FixArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// Write the repaired FBX here. Without this, runs as a dry run (prints the plan only).
    #[arg(short, long)]
    output: Option<std::path::PathBuf>,
    /// Allow `--output` to overwrite the input file or any existing output file.
    #[arg(long)]
    force: bool,
    /// Emit a machine-readable JSON plan instead of human-readable text.
    #[arg(long)]
    json: bool,
    /// Also write a headless-Blender Python script here that applies the WHOLE plan — including
    /// the flagged geometry repairs (reparents, scale/orientation) this tool won't apply natively.
    /// Run it with `blender --background --python <script>`.
    #[arg(long, value_name = "SCRIPT.py")]
    blender_script: Option<std::path::PathBuf>,
    /// Output FBX path the Blender script exports to (default: `<input>.fixed.fbx`).
    #[arg(long, value_name = "OUT.fbx", requires = "blender_script")]
    blender_output: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct FileArgs {
    /// Path to a binary FBX file.
    path: std::path::PathBuf,
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    json: bool,
}

pub fn inspect(args: &FileArgs) -> Result<()> {
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
pub fn armature_check(args: &FileArgs) -> Result<ExitCode> {
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

pub fn armature_fix(args: &FixArgs) -> Result<()> {
    let mut doc = FbxDocument::load(&args.path)?;
    let plan = plan_repairs(&doc.scene());

    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print_plan(&args.path, &plan);
    }

    // The Blender-script route covers the whole plan (including the flagged geometry repairs),
    // so it is independent of -o: emitting a script is not a write to the FBX.
    if let Some(script_path) = &args.blender_script {
        let default_out = args.path.with_extension("fixed.fbx");
        let blender_out = args.blender_output.as_deref().unwrap_or(&default_out);
        match avatar_armature::blender_script(
            &plan,
            &args.path.display().to_string(),
            &blender_out.display().to_string(),
        ) {
            Some(script) => {
                if script_path.exists() && !args.force {
                    bail!(
                        "refusing to overwrite existing file {} (pass --force to overwrite)",
                        script_path.display()
                    );
                }
                std::fs::write(script_path, script)
                    .with_context(|| format!("writing {}", script_path.display()))?;
                if !args.json {
                    println!(
                        "\n  wrote Blender repair script {} — run: blender --background --python {}",
                        script_path.display(),
                        script_path.display()
                    );
                }
            }
            None => {
                if !args.json {
                    println!("\n  no Blender script written — the plan has no repairs at all");
                }
            }
        }
    }

    match &args.output {
        None => {
            if !args.json && plan.native().count() > 0 {
                println!("\n  (dry run — pass -o <file> to write the repaired FBX)");
            }
        }
        Some(out) => {
            if !args.force {
                if overwrites_input(&args.path, out) {
                    bail!(
                        "refusing to overwrite the input file {}; choose a different -o path or pass --force",
                        args.path.display()
                    );
                }
                if out.exists() {
                    bail!(
                        "refusing to overwrite existing file {} (pass --force to overwrite)",
                        out.display()
                    );
                }
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub(crate) struct InspectSummary {
    pub(crate) version: u32,
    pub(crate) unit_scale_factor: Option<f64>,
    pub(crate) up_axis: Option<i32>,
    pub(crate) total_objects: usize,
    pub(crate) models: usize,
    pub(crate) geometries: usize,
    pub(crate) materials: usize,
    pub(crate) bone_like: usize,
    pub(crate) roots: Vec<String>,
}

pub(crate) fn inspect_summary(scene: &FbxScene) -> InspectSummary {
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
