//! `avatar-migrate` — SDK2 → SDK3 (Avatars 3.0) migration of a VRChat avatar project.
//!
//! Given an extracted SDK2 avatar project (as `avatar unitypackage extract` produces), this
//! rewrites the avatar prefab into an SDK3 one and assembles a fresh Unity project around it:
//!
//! - **Descriptor**: the SDK2 `VRC_AvatarDescriptor` becomes a `VRCAvatarDescriptor` at the same
//!   fileID (view position, IPD scale, visemes, portrait offsets carried over; playable layers
//!   set to SDK defaults except an FX layer generated from the SDK2 gesture overrides; eye look
//!   derived from the rig geometry; blink blendshape wired). A `PipelineManager` is retyped or
//!   added; the SDK2 blueprint id is dropped (an SDK3 upload is a new avatar).
//! - **Animator**: `applyRootMotion` off, no controller (SDK3 drives its own).
//! - **Dynamics**: DynamicBone → `VRCPhysBone` and DynamicBoneCollider → `VRCPhysBoneCollider`,
//!   in place (same fileIDs, so collider lists keep resolving), using the SDK's own conversion
//!   rules ([`sdk3::PhysBoneSpec::from_dynamic_bone`]). Optionally: Unity `Cloth` removed and its
//!   `CapsuleCollider`s retyped as PhysBone colliders, and new PhysBone chains added on named
//!   roots (a Cloth skirt becomes a PhysBone skirt colliding with the former cloth capsules).
//! - **Clutter**: named subtrees stripped (a haptics vest, stray cameras…).
//! - **FX**: SDK2 gesture overrides → clean blendshape clips + an either-hand gesture layer
//!   ([`fx`]); expression menu/parameters assets generated (empty, ready for toggles).
//! - **Project**: `Assets/` copied minus exclusions (SDK2's `VRCSDK`, examples, DynamicBone
//!   scripts), `vpm-manifest.json` for `com.vrchat.avatars`, `ProjectVersion.txt`.
//!
//! Everything the prefab rewrite doesn't touch is preserved byte-for-byte
//! ([`avatar_unity_yaml::EditableUnityFile`]). The FBX and its `.meta` (humanoid map, T-pose)
//! are copied unchanged. The one step this tool does not own is Unity/VCC + the SDK upload.

pub mod eyelook;
pub mod fx;
pub mod math;
pub mod packages;
pub mod project;
pub mod rewrite;
pub mod scene;
pub mod sdk2;
pub mod sdk3;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_anim_gen::toggle::{deterministic_guid, native_asset_meta};
use avatar_anim_gen::{ExpressionParams, ExpressionsMenu};
use avatar_unity_yaml::{Scalar, UnityFile, build_guid_index, field_i64, field_str, walk_assets};
use serde::Serialize;

use crate::eyelook::{EyeLookAngles, derive_eye_look};
use crate::fx::{FxBundle, FxLayout, build_fx_from_overrides};
use crate::math::{Trs, Vec3};
use crate::packages::{RelinkedMaterial, ShaderIndex, VpmPackage, relink_locked_materials};
use crate::rewrite::PrefabRewriter;
use crate::scene::{CAPSULE_COLLIDER, MONO_BEHAVIOUR, SKINNED_MESH_RENDERER, Scene, TRANSFORM};
use crate::sdk2::Sdk2Avatar;
use crate::sdk3::{
    DescriptorSpec, LimitType, LipSyncStyle, PhysBoneColliderSpec, PhysBoneSpec, PlayableLayer,
    pipeline_manager_body,
};

/// A PhysBone chain to add on a named root (e.g. a skirt hanging off `Hips`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PhysBoneRootSpec {
    /// Root Transform, by unique name or `A/B/C` path from the avatar root.
    pub root: String,
    /// Children of the root to leave out (names/paths) — the humanoid chains under `Hips`.
    pub ignore: Vec<String>,
    /// GameObjects whose (converted) colliders the chain collides with; empty = every collider
    /// converted from a Unity CapsuleCollider.
    pub colliders: Vec<String>,
    /// Gather the chain's children (the root's children minus `ignore`) under a new empty child
    /// of the root with this name and root the PhysBone *there*. This is what you want when the
    /// root is a humanoid bone (`Hips`): the PhysBone then owns only the chain, and colliders that
    /// live under the humanoid bone (leg capsules) are no longer inside its own hierarchy — which
    /// VRChat's PhysBone scheduler flags as a cyclic dependency.
    pub group: Option<String>,
    /// Tuning overrides; `None` = the skirt-ish defaults in [`MigrateOptions::default_skirt`].
    pub pull: Option<f64>,
    pub spring: Option<f64>,
    pub stiffness: Option<f64>,
    pub gravity: Option<f64>,
    pub immobile: Option<f64>,
    pub radius: Option<f64>,
    pub max_angle: Option<f64>,
}

impl PhysBoneRootSpec {
    /// Parse the CLI form `root|ignore1,ignore2|collider1,collider2|group` (later parts optional).
    pub fn parse(s: &str) -> Result<Self> {
        let mut parts = s.split('|');
        let root = parts.next().unwrap_or("").trim().to_string();
        if root.is_empty() {
            bail!("PhysBone root spec is empty");
        }
        let list = |p: Option<&str>| -> Vec<String> {
            p.map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default()
        };
        Ok(PhysBoneRootSpec {
            root,
            ignore: list(parts.next()),
            colliders: list(parts.next()),
            group: parts
                .next()
                .map(|g| g.trim().to_string())
                .filter(|g| !g.is_empty()),
            pull: None,
            spring: None,
            stiffness: None,
            gravity: None,
            immobile: None,
            radius: None,
            max_angle: None,
        })
    }
}

/// What to migrate and how.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MigrateOptions {
    /// Source project root (contains `Assets/`).
    pub source_project: PathBuf,
    /// The SDK2 avatar prefab (Assets-relative or absolute). `None` = the only prefab in the
    /// project carrying an SDK2 descriptor.
    pub prefab: Option<PathBuf>,
    /// Output project directory (created; must not already contain `Assets/`).
    pub output: PathBuf,
    /// Name for the migrated prefab and the folder holding generated assets.
    pub avatar_name: String,
    /// GameObjects (names/paths) to remove with their subtrees.
    pub strip: Vec<String>,
    /// Remove Unity `Cloth` components.
    pub drop_cloth: bool,
    /// Retype Unity `CapsuleCollider`s (Cloth support) as `VRCPhysBoneCollider`s.
    pub capsules_to_physbone_colliders: bool,
    /// New PhysBone chains to add.
    pub physbone_roots: Vec<PhysBoneRootSpec>,
    /// Eye bones (left, right) by name/path; `None` = eye look disabled.
    pub eye_bones: Option<(String, String)>,
    pub eye_look_angles: EyeLookAngles,
    /// Blink blendshape name on the viseme mesh (`None` = auto: first of `Blink`, `blink`,
    /// `Blink_Both`, `vrc.blink`).
    pub blink_shape: Option<String>,
    /// Build the FX layer from the SDK2 gesture overrides.
    pub fx_from_overrides: bool,
    /// Assets-relative directories not to copy (SDK2's `VRCSDK`, examples, DynamicBone…).
    pub exclude: Vec<String>,
    /// `com.vrchat.avatars` version to pin in `vpm-manifest.json`.
    pub sdk_version: String,
    /// Unity editor version for `ProjectVersion.txt`.
    pub unity_version: String,
    /// VPM packages (directories with `package.json`, or `.zip`s of one) to bundle into the output
    /// project's `Packages/` — e.g. the shader package the materials need.
    pub vpm_packages: Vec<PathBuf>,
    /// Re-point materials whose shader was replaced by a locker's generated `Hidden/…` copy back
    /// to their `OriginalShader` (found among source assets + bundled packages), and drop the
    /// generated copies.
    pub relink_locked_shaders: bool,
    /// Don't write anything; just plan and report.
    pub dry_run: bool,
}

impl MigrateOptions {
    /// Sensible defaults for `source_project` → `output`.
    pub fn new(
        source_project: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
        avatar_name: impl Into<String>,
    ) -> Self {
        MigrateOptions {
            source_project: source_project.into(),
            prefab: None,
            output: output.into(),
            avatar_name: avatar_name.into(),
            strip: Vec::new(),
            drop_cloth: false,
            capsules_to_physbone_colliders: false,
            physbone_roots: Vec::new(),
            eye_bones: None,
            eye_look_angles: EyeLookAngles::default(),
            blink_shape: None,
            fx_from_overrides: true,
            exclude: vec!["VRCSDK".into(), "VRChat Examples".into()],
            sdk_version: "3.10.4".into(),
            unity_version: "2022.3.22f1".into(),
            vpm_packages: Vec::new(),
            relink_locked_shaders: false,
            dry_run: false,
        }
    }

    /// Skirt-ish PhysBone defaults for an added chain: soft pull, some bounce, world-motion
    /// immobile so walking doesn't fling it, angle-limited so it can't invert through the body.
    pub fn default_skirt(game_object: i64, root: i64) -> PhysBoneSpec {
        let mut pb = PhysBoneSpec::new(game_object);
        pb.root_transform = root;
        pb.multi_child_type = 0;
        pb.pull = 0.25;
        pb.spring = 0.5;
        pb.stiffness = 0.15;
        pb.gravity = 0.03;
        pb.gravity_falloff = 0.5;
        pb.immobile_type = 1;
        pb.immobile = 0.3;
        pb.radius = 0.03;
        pb.limit_type = LimitType::Angle;
        pb.max_angle_x = 55.0;
        pb.allow_grabbing = false;
        pb.allow_posing = false;
        pb
    }
}

/// One migrated / converted component, for the report.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ComponentChange {
    pub what: String,
    pub object_path: String,
    pub file_id: i64,
}

/// A generated or copied file.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OutputFile {
    /// Output-project-relative path.
    pub path: String,
    pub kind: String,
}

/// The migration report (`--json`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MigrationReport {
    pub source_prefab: String,
    pub output_project: String,
    pub output_prefab: String,
    pub dry_run: bool,
    pub avatar_root: String,
    pub descriptor: Option<String>,
    pub view_position: Option<[f64; 3]>,
    pub root_scale: [f64; 3],
    pub stripped: Vec<String>,
    pub converted: Vec<ComponentChange>,
    pub added: Vec<ComponentChange>,
    pub removed: Vec<ComponentChange>,
    pub eye_look: Option<String>,
    pub blink_blendshape: Option<(String, i64)>,
    pub fx: Option<FxBundle>,
    pub generated: Vec<OutputFile>,
    pub assets_copied: usize,
    pub assets_skipped: usize,
    /// Source assets not copied because a bundled package already provides their GUID.
    pub assets_deduped: usize,
    /// `(name, version)` of VPM packages bundled into `Packages/`.
    pub bundled_packages: Vec<(String, String)>,
    /// Materials re-pointed from a locked shader copy to their original shader.
    pub relinked_materials: Vec<RelinkedMaterial>,
    pub warnings: Vec<String>,
    /// What still needs a human in Unity.
    pub next_steps: Vec<String>,
    pub prefab_log: Vec<String>,
}

/// Run the migration.
pub fn migrate(opts: &MigrateOptions) -> Result<MigrationReport> {
    let assets_root = opts.source_project.join("Assets");
    if !assets_root.is_dir() {
        bail!("{} has no Assets/ directory", opts.source_project.display());
    }
    let files = walk_assets(&assets_root);
    let guid_index = build_guid_index(&files);

    // ---- 0. Bundled VPM packages (loaded first: they feed the shader index and the GUID dedupe)
    let scratch = std::env::temp_dir().join(format!("avatar-migrate-{}", std::process::id()));
    let mut packages: Vec<VpmPackage> = Vec::new();
    for p in &opts.vpm_packages {
        packages.push(
            VpmPackage::load(p, &scratch)
                .with_context(|| format!("--vpm-package {}", p.display()))?,
        );
    }
    let mut exclude: Vec<String> = opts.exclude.clone();
    let mut package_guids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for pkg in &packages {
        for lf in &pkg.legacy_folders {
            if !exclude.contains(lf) {
                exclude.push(lf.clone());
            }
        }
        package_guids.extend(pkg.guids.keys().cloned());
    }
    // Locked-shader relink: source assets + packages form the shader name index.
    let mut overrides: HashMap<String, String> = HashMap::new();
    let mut relinked_materials: Vec<RelinkedMaterial> = Vec::new();
    let mut early_warnings: Vec<String> = Vec::new();
    if opts.relink_locked_shaders {
        let mut shaders = ShaderIndex::default();
        shaders.add(&guid_index);
        for pkg in &packages {
            shaders.add(&pkg.guids);
        }
        let r = relink_locked_materials(&assets_root, &files, &shaders);
        for d in r.exclude_dirs {
            if !exclude.contains(&d) {
                exclude.push(d);
            }
        }
        for (mat, orig) in &r.unresolved {
            early_warnings.push(format!(
                "material {mat}: original shader '{orig}' not found in the project or bundled packages; left on its locked copy"
            ));
        }
        overrides = r.overrides;
        relinked_materials = r.relinked;
    }

    // ---- locate + parse the SDK2 prefab
    let prefab_path = match &opts.prefab {
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => {
            let a = assets_root.join(p);
            if a.exists() {
                a
            } else {
                opts.source_project.join(p)
            }
        }
        None => find_sdk2_prefab(&files)?,
    };
    let prefab_text = std::fs::read_to_string(&prefab_path)
        .with_context(|| format!("reading {}", prefab_path.display()))?;
    let mut rw = PrefabRewriter::new(&prefab_text)?;
    let scene = rw.scene().clone();
    let sdk2 = Sdk2Avatar::read(&scene)?;
    let mut warnings = early_warnings;
    let mut next_steps = Vec::new();
    let mut converted = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut stripped = Vec::new();

    let root_name = scene.name_of_transform(sdk2.root_transform).to_string();
    let root_scale = scene.transforms[&sdk2.root_transform].local.scale;
    let path_of_go = |go: i64| -> String {
        scene
            .game_objects
            .get(&go)
            .map(|g| scene.path_of(g.transform))
            .unwrap_or_default()
    };

    // ---- 1. strip subtrees (first, so later passes don't see their components)
    let mut stripped_transforms: Vec<i64> = Vec::new();
    for name in &opts.strip {
        let t = scene
            .transform_by_path(name)
            .with_context(|| format!("--strip {name}"))?;
        stripped_transforms.extend(scene.descendants(t));
        rw.strip_subtree(t)?;
        stripped.push(scene.path_of(t));
    }
    let is_stripped = |go: i64| -> bool {
        scene
            .game_objects
            .get(&go)
            .is_some_and(|g| stripped_transforms.contains(&g.transform))
    };

    // ---- 1b. Root GameObject name: SDK2's upload pipeline leaves `prefab-id-v1_avtr_…`; the
    // migrated avatar is named after itself.
    if root_name != opts.avatar_name {
        rw.set_scalar(
            sdk2.root_game_object,
            "m_Name",
            Scalar::Str(&opts.avatar_name),
        )?;
        rw.log.push(format!(
            "renamed root '{}' -> '{}'",
            root_name, opts.avatar_name
        ));
    }

    // ---- 2. Animator: root motion off, controller cleared
    match &sdk2.animator {
        Some(a) => {
            rw.set_scalar(a.file_id, "m_ApplyRootMotion", Scalar::Int(0))?;
            rw.set_reference(a.file_id, "m_Controller", 0, None, 0)?;
            if a.apply_root_motion {
                converted.push(ComponentChange {
                    what: "Animator: applyRootMotion 1 -> 0 (SDK3 locomotion must not apply root motion)".into(),
                    object_path: String::new(),
                    file_id: a.file_id,
                });
            }
            if a.avatar.is_none() {
                warnings.push("root Animator has no humanoid Avatar assigned".into());
            }
        }
        None => warnings.push("no Animator on the avatar root".into()),
    }

    // ---- 3. Cloth / capsules
    if opts.drop_cloth {
        for c in &sdk2.cloths {
            let go = scene.owner_of(*c).unwrap_or(0);
            if is_stripped(go) {
                continue;
            }
            rw.remove_component(*c)?;
            removed.push(ComponentChange {
                what: "Cloth".into(),
                object_path: path_of_go(go),
                file_id: *c,
            });
        }
    }
    // Objects that carry a DynamicBoneCollider (their capsule, if any, is redundant).
    let db_collider_gos: Vec<i64> = sdk2
        .dynamic_bone_colliders
        .iter()
        .map(|c| c.game_object)
        .collect();
    let mut capsule_colliders_by_go: HashMap<i64, i64> = HashMap::new(); // go -> collider comp
    for cap in &sdk2.capsule_colliders {
        if is_stripped(cap.game_object) {
            continue;
        }
        if opts.capsules_to_physbone_colliders {
            if db_collider_gos.contains(&cap.game_object) {
                rw.remove_component(cap.file_id)?;
                removed.push(ComponentChange {
                    what: "CapsuleCollider (object also has a DynamicBoneCollider)".into(),
                    object_path: path_of_go(cap.game_object),
                    file_id: cap.file_id,
                });
                continue;
            }
            let spec = PhysBoneColliderSpec::from_capsule_collider(
                cap.game_object,
                cap.direction,
                cap.radius,
                cap.height,
                cap.center,
            );
            rw.retype_component(
                cap.file_id,
                MONO_BEHAVIOUR,
                &spec.to_body(),
                &format!(
                    "CapsuleCollider -> VRCPhysBoneCollider on '{}'",
                    path_of_go(cap.game_object)
                ),
            )?;
            capsule_colliders_by_go.insert(cap.game_object, cap.file_id);
            converted.push(ComponentChange {
                what: "CapsuleCollider -> VRCPhysBoneCollider (capsule)".into(),
                object_path: path_of_go(cap.game_object),
                file_id: cap.file_id,
            });
        } else if opts.drop_cloth {
            warnings.push(format!(
                "CapsuleCollider left on '{}' (was Cloth support; pass --capsules-to-physbone-colliders to reuse it)",
                path_of_go(cap.game_object)
            ));
        }
    }

    // ---- 4. DynamicBoneCollider -> VRCPhysBoneCollider (in place)
    let mut db_collider_ids: Vec<i64> = Vec::new();
    for c in &sdk2.dynamic_bone_colliders {
        if is_stripped(c.game_object) {
            continue;
        }
        let spec = PhysBoneColliderSpec::from_dynamic_bone_collider(
            c.game_object,
            c.direction,
            c.bound,
            c.radius,
            c.height,
            c.center,
        );
        rw.retype_component(
            c.file_id,
            MONO_BEHAVIOUR,
            &spec.to_body(),
            &format!(
                "DynamicBoneCollider -> VRCPhysBoneCollider on '{}'",
                path_of_go(c.game_object)
            ),
        )?;
        db_collider_ids.push(c.file_id);
        converted.push(ComponentChange {
            what: format!(
                "DynamicBoneCollider -> VRCPhysBoneCollider ({})",
                if spec.shape == sdk3::ColliderShape::Capsule {
                    "capsule"
                } else {
                    "sphere"
                }
            ),
            object_path: path_of_go(c.game_object),
            file_id: c.file_id,
        });
    }

    // ---- 5. DynamicBone -> VRCPhysBone (in place, SDK conversion rules)
    for db in &sdk2.dynamic_bones {
        if is_stripped(db.game_object) {
            continue;
        }
        let own_transform = scene.game_objects[&db.game_object].transform;
        let root_t = if db.root != 0 { db.root } else { own_transform };
        let obj_scale = scene.world(own_transform).scale.x.abs();
        let root_scale_x = scene.world(root_t).scale.x.abs().max(1e-9);
        let scale_ratio = obj_scale / root_scale_x;
        let avg_len = average_bone_length(&scene, root_t, &db.exclusions);
        let colliders: Vec<i64> = db
            .colliders
            .iter()
            .copied()
            .filter(|c| db_collider_ids.contains(c))
            .collect();
        if colliders.len() != db.colliders.len() {
            warnings.push(format!(
                "DynamicBone on '{}': {} of {} colliders could not be mapped (stripped or missing)",
                path_of_go(db.game_object),
                db.colliders.len() - colliders.len(),
                db.colliders.len()
            ));
        }
        let mut spec = PhysBoneSpec::from_dynamic_bone(
            db.game_object,
            if db.root != 0 { db.root } else { 0 },
            db.exclusions.clone(),
            db.elasticity,
            db.damping,
            db.stiffness,
            db.inert,
            db.radius,
            scale_ratio,
            db.gravity.y,
            db.force.y,
            db.freeze_axis,
            avg_len,
            obj_scale,
            colliders,
        );
        if db.end_length > 0.0 || db.end_offset != Vec3::ZERO {
            // The SDK derives an endpoint from endLength/endOffset; approximate with the offset
            // (endLength scales the last bone's direction, which needs the last bone — noted).
            spec.endpoint_position = db.end_offset;
            if db.end_length > 0.0 {
                warnings.push(format!(
                    "DynamicBone on '{}': m_EndLength {} not carried (set Endpoint Position in Unity if the tip looks short)",
                    path_of_go(db.game_object),
                    db.end_length
                ));
            }
        }
        rw.retype_component(
            db.file_id,
            MONO_BEHAVIOUR,
            &spec.to_body(),
            &format!(
                "DynamicBone -> VRCPhysBone on '{}'",
                path_of_go(db.game_object)
            ),
        )?;
        converted.push(ComponentChange {
            what: format!(
                "DynamicBone -> VRCPhysBone (pull {:.2}, spring {:.2}, immobile {:.2}, maxAngle {:.0}, radius {:.3}, gravity {:.3})",
                spec.pull, spec.spring, spec.immobile, spec.max_angle_x, spec.radius, spec.gravity
            ),
            object_path: path_of_go(db.game_object),
            file_id: db.file_id,
        });
    }

    // ---- 6. New PhysBone chains
    for pr in &opts.physbone_roots {
        let root_t = scene
            .transform_by_path(&pr.root)
            .with_context(|| format!("PhysBone root '{}'", pr.root))?;
        let go = scene.transforms[&root_t].game_object;
        let mut ignore: Vec<i64> = Vec::new();
        for i in &pr.ignore {
            let t = scene
                .transform_by_path(i)
                .with_context(|| format!("PhysBone ignore '{i}' for root '{}'", pr.root))?;
            ignore.push(t);
        }
        let colliders: Vec<i64> = if pr.colliders.is_empty() {
            let mut v: Vec<i64> = capsule_colliders_by_go.values().copied().collect();
            v.sort();
            v
        } else {
            let mut v = Vec::new();
            for name in &pr.colliders {
                let t = scene
                    .transform_by_path(name)
                    .with_context(|| format!("PhysBone collider object '{name}'"))?;
                let g = scene.transforms[&t].game_object;
                let id = capsule_colliders_by_go
                    .get(&g)
                    .copied()
                    .or_else(|| {
                        sdk2.dynamic_bone_colliders
                            .iter()
                            .find(|c| c.game_object == g)
                            .map(|c| c.file_id)
                    })
                    .with_context(|| format!("'{name}' has no converted collider"))?;
                v.push(id);
            }
            v
        };
        // The chain roots are the root's remaining *bone-only* children (Transform and nothing
        // else). Children carrying components — collider holders, renderers, props — are not part
        // of a chain: they are left in place when grouping and auto-ignored otherwise, so the
        // simulation never moves a collider or a prop hanging off a humanoid bone.
        let (chain, non_bone): (Vec<i64>, Vec<i64>) = scene.transforms[&root_t]
            .children
            .iter()
            .copied()
            .filter(|c| !ignore.contains(c) && !stripped_transforms.contains(c))
            .partition(|c| {
                scene
                    .transforms
                    .get(c)
                    .and_then(|t| scene.game_objects.get(&t.game_object))
                    .is_some_and(|g| g.components.iter().all(|comp| *comp == *c))
            });
        if pr.group.is_none() {
            for nb in &non_bone {
                if !ignore.contains(nb) {
                    ignore.push(*nb);
                }
            }
        }
        if !non_bone.is_empty() {
            rw.log.push(format!(
                "PhysBone '{}': left {} component-bearing child(ren) out of the chain ({})",
                pr.root,
                non_bone.len(),
                non_bone
                    .iter()
                    .map(|t| scene.name_of_transform(*t).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let (pb_go, pb_root, ignore) = match &pr.group {
            Some(name) => {
                if chain.is_empty() {
                    bail!(
                        "PhysBone root '{}': nothing left to group after ignores",
                        pr.root
                    );
                }
                let (ggo, gtr) = rw.add_child_game_object(root_t, name)?;
                for c in &chain {
                    rw.reparent(*c, gtr)?;
                }
                rw.log.push(format!(
                    "grouped {} chain root(s) under '{}/{name}'",
                    chain.len(),
                    scene.path_of(root_t)
                ));
                (ggo, 0, Vec::new())
            }
            None => (go, root_t, ignore),
        };
        // Cyclic-dependency check: a collider inside the PhysBone's own hierarchy. Grouped, the
        // hierarchy is just the chain; ungrouped it is the whole root subtree minus the ignores.
        let owned: Vec<i64> = if pr.group.is_some() {
            chain.iter().flat_map(|c| scene.descendants(*c)).collect()
        } else {
            let ignored: Vec<i64> = ignore.iter().flat_map(|i| scene.descendants(*i)).collect();
            scene
                .descendants(root_t)
                .into_iter()
                .filter(|t| *t != root_t && !ignored.contains(t))
                .collect()
        };
        for c in &colliders {
            if let Some(ct) = scene.transform_of_component(*c)
                && owned.contains(&ct)
            {
                warnings.push(format!(
                    "PhysBone on '{}': collider on '{}' is inside the chain's own hierarchy — VRChat reports a cyclic dependency; regroup the chain (`|GROUP` on --physbone) or drop that collider",
                    pr.root,
                    scene.path_of(ct)
                ));
            }
        }
        // Ungrouped on a humanoid root: colliders under the root's ignored children still trip
        // VRChat's scheduler (it walks the root's full hierarchy), so say so.
        if pr.group.is_none() {
            let under_root = scene.descendants(root_t);
            for c in &colliders {
                if let Some(ct) = scene.transform_of_component(*c)
                    && under_root.contains(&ct)
                    && !owned.contains(&ct)
                {
                    warnings.push(format!(
                        "PhysBone on '{}': collider on '{}' is under the root's hierarchy (an ignored branch) — VRChat's scheduler still reports a cyclic dependency; regroup the chain with `|GROUP` on --physbone",
                        pr.root,
                        scene.path_of(ct)
                    ));
                }
            }
        }
        let mut spec = MigrateOptions::default_skirt(pb_go, pb_root);
        spec.ignore_transforms = ignore;
        spec.colliders = colliders;
        if let Some(v) = pr.pull {
            spec.pull = v;
        }
        if let Some(v) = pr.spring {
            spec.spring = v;
        }
        if let Some(v) = pr.stiffness {
            spec.stiffness = v;
        }
        if let Some(v) = pr.gravity {
            spec.gravity = v;
        }
        if let Some(v) = pr.immobile {
            spec.immobile = v;
        }
        if let Some(v) = pr.radius {
            spec.radius = v;
        }
        if let Some(v) = pr.max_angle {
            spec.max_angle_x = v;
        }
        let id = rw.add_component(
            pb_go,
            MONO_BEHAVIOUR,
            &spec.to_body(),
            &format!("physbone/{}", pr.root),
        )?;
        rw.log
            .push(format!("added VRCPhysBone chain rooted at '{}'", pr.root));
        added.push(ComponentChange {
            what: format!(
                "VRCPhysBone chain ({}, {} colliders)",
                match &pr.group {
                    Some(g) => format!("chain regrouped under '{g}'"),
                    None => format!("ignoring {} children", spec.ignore_transforms.len()),
                },
                spec.colliders.len()
            ),
            object_path: match &pr.group {
                Some(g) => format!("{}/{g}", scene.path_of(root_t)),
                None => scene.path_of(root_t),
            },
            file_id: id,
        });
    }

    // ---- 7. FX from gesture overrides + expression assets
    let gen_dir = format!("{}_SDK3", opts.avatar_name);
    let mut generated: Vec<OutputFile> = Vec::new();
    let mut generated_files: Vec<(String, String)> = Vec::new(); // (Assets-relative, content)
    let mut fx_bundle: Option<FxBundle> = None;
    let mut fx_ref: Option<(i64, String)> = None;
    if opts.fx_from_overrides {
        match sdk2
            .descriptor
            .as_ref()
            .and_then(|d| d.custom_standing_anims.as_ref())
        {
            Some((_, guid)) => {
                let layout = FxLayout {
                    dir: &format!("{gen_dir}/FX"),
                    controller_name: "FX",
                };
                match build_fx_from_overrides(&assets_root, &guid_index, guid, &layout) {
                    Ok(bundle) => {
                        for f in &bundle.files {
                            generated_files.push((f.rel_path.clone(), f.content.clone()));
                        }
                        fx_ref = Some(bundle.controller_ref.clone());
                        for (slot, why) in &bundle.skipped {
                            warnings.push(format!("override slot {slot}: {why}"));
                        }
                        fx_bundle = Some(bundle);
                    }
                    Err(e) => warnings.push(format!("FX from overrides skipped: {e:#}")),
                }
            }
            None => warnings
                .push("no CustomStandingAnims override controller; no FX layer generated".into()),
        }
    }
    // Expression parameters + menu (empty, ready for toggles).
    let params = ExpressionParams::new("Parameters");
    let params_guid = deterministic_guid(&format!("{gen_dir}/Parameters.asset"));
    generated_files.push((
        format!("{gen_dir}/Parameters.asset"),
        params.to_unity_yaml(11400000),
    ));
    generated_files.push((
        format!("{gen_dir}/Parameters.asset.meta"),
        native_asset_meta(&params_guid, 11400000),
    ));
    let menu = ExpressionsMenu::new("Menu");
    let menu_guid = deterministic_guid(&format!("{gen_dir}/Menu.asset"));
    generated_files.push((
        format!("{gen_dir}/Menu.asset"),
        menu.to_unity_yaml(11400000),
    ));
    generated_files.push((
        format!("{gen_dir}/Menu.asset.meta"),
        native_asset_meta(&menu_guid, 11400000),
    ));

    // ---- 8. Descriptor
    let mut view_position = None;
    let mut eye_look_note = None;
    let mut blink = None;
    let descriptor_note = match &sdk2.descriptor {
        Some(d) => {
            view_position = Some([d.view_position.x, d.view_position.y, d.view_position.z]);
            // Eye look.
            let eye_look = match &opts.eye_bones {
                Some((l, r)) => {
                    let lt = scene
                        .transform_by_path(l)
                        .with_context(|| format!("left eye '{l}'"))?;
                    let rt = scene
                        .transform_by_path(r)
                        .with_context(|| format!("right eye '{r}'"))?;
                    let eyelids = if d.viseme_mesh != 0 {
                        match find_blink_index(
                            &scene,
                            &guid_index,
                            d.viseme_mesh,
                            opts.blink_shape.as_deref(),
                        ) {
                            Ok(Some((name, idx))) => {
                                blink = Some((name, idx));
                                Some((d.viseme_mesh, [idx as i32, -1, -1]))
                            }
                            Ok(None) => {
                                warnings.push("no blink blendshape found on the viseme mesh; eyelids left unset".into());
                                None
                            }
                            Err(e) => {
                                warnings.push(format!("blink blendshape lookup failed: {e:#}"));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    eye_look_note = Some(format!(
                        "derived from rig: up {}°, down {}°, left {}°, right {}°",
                        opts.eye_look_angles.up,
                        opts.eye_look_angles.down,
                        opts.eye_look_angles.left,
                        opts.eye_look_angles.right
                    ));
                    Some(derive_eye_look(
                        &scene,
                        lt,
                        rt,
                        opts.eye_look_angles,
                        eyelids,
                    ))
                }
                None => None,
            };
            let mut base_layers: [PlayableLayer; 5] = Default::default();
            base_layers[4] = PlayableLayer {
                controller: fx_ref.clone(),
            };
            let spec = DescriptorSpec {
                game_object: d.game_object,
                view_position: d.view_position,
                scale_ipd: d.scale_ipd,
                lip_sync: match d.lip_sync {
                    1 => LipSyncStyle::JawFlapBone,
                    2 => LipSyncStyle::JawFlapBlendShape,
                    3 => LipSyncStyle::VisemeBlendShape,
                    4 => LipSyncStyle::VisemeParameterOnly,
                    _ => LipSyncStyle::Default,
                },
                viseme_mesh: d.viseme_mesh,
                viseme_blendshapes: d.viseme_blendshapes.clone(),
                mouth_open_blendshape: d.mouth_open_blendshape.clone(),
                portrait_camera_position_offset: d.portrait_camera_position_offset,
                portrait_camera_rotation_offset: d.portrait_camera_rotation_offset,
                base_layers,
                special_layers: Default::default(),
                expressions_menu: Some((11400000, menu_guid.clone())),
                expression_parameters: Some((11400000, params_guid.clone())),
                eye_look,
            };
            rw.retype_component(
                d.file_id,
                MONO_BEHAVIOUR,
                &spec.to_body(),
                "VRC_AvatarDescriptor (SDK2) -> VRCAvatarDescriptor (SDK3)",
            )?;
            converted.push(ComponentChange {
                what: "VRC_AvatarDescriptor (SDK2) -> VRCAvatarDescriptor (SDK3)".into(),
                object_path: path_of_go(d.game_object),
                file_id: d.file_id,
            });
            if d.custom_sitting_anims.is_some() {
                warnings.push("CustomSittingAnims override not migrated (SDK3 sitting is the Sitting special layer)".into());
            }
            Some(format!("SDK2 descriptor at fileID {}", d.file_id))
        }
        None => {
            warnings.push(
                "no SDK2 VRC_AvatarDescriptor found; prefab left without a descriptor".into(),
            );
            None
        }
    };

    // PipelineManager: retype (new DLL) or add.
    match sdk2.pipeline_manager {
        Some(pm) => {
            let go = scene.owner_of(pm).unwrap_or(sdk2.root_game_object);
            rw.retype_component(
                pm,
                MONO_BEHAVIOUR,
                &pipeline_manager_body(go, ""),
                "PipelineManager -> SDK3 PipelineManager (blueprint cleared)",
            )?;
            converted.push(ComponentChange {
                what:
                    "PipelineManager re-pointed at the SDK3 DLL; blueprint id cleared (new upload)"
                        .into(),
                object_path: String::new(),
                file_id: pm,
            });
        }
        None => {
            let id = rw.add_component(
                sdk2.root_game_object,
                MONO_BEHAVIOUR,
                &pipeline_manager_body(sdk2.root_game_object, ""),
                "pipeline-manager",
            )?;
            added.push(ComponentChange {
                what: "PipelineManager".into(),
                object_path: String::new(),
                file_id: id,
            });
        }
    }

    // ---- 9. Leftover checks
    let out_text = rw.text().to_string();
    let out_parsed = UnityFile::parse(&out_text)?;
    let out_scene = Scene::from_file(&out_parsed)?;
    let mut cams = 0;
    let mut extra_animators = 0;
    for d in out_scene.docs.values() {
        match d.class_id {
            scene::CAMERA => cams += 1,
            scene::ANIMATOR
                if field_i64(&d.body["m_GameObject"], "fileID") != Some(sdk2.root_game_object) =>
            {
                extra_animators += 1;
            }
            _ => {}
        }
    }
    if cams > 0 {
        warnings.push(format!("{cams} Camera component(s) remain on the avatar"));
    }
    if extra_animators > 0 {
        warnings.push(format!(
            "{extra_animators} extra Animator(s) remain below the root"
        ));
    }
    for d in out_scene.docs.values() {
        if d.class_id == MONO_BEHAVIOUR
            && (sdk2::is_dynamic_bone(&d.body) || sdk2::is_dynamic_bone_collider(&d.body))
        {
            warnings.push(format!(
                "DynamicBone component {} was not converted",
                d.file_id
            ));
        }
    }
    let _ = TRANSFORM;
    let _ = CAPSULE_COLLIDER;

    // ---- 9b. Material shaders: a locked/optimized shader exported without its #include files
    // compiles to pink in a fresh project. Detect it here so the user hears it from the report.
    let broken: Vec<(String, String)> =
        shader_include_check(&assets_root, &files, &guid_index, &exclude)
            .into_iter()
            .filter(|(m, _)| !overrides.contains_key(m))
            .collect();
    if !broken.is_empty() {
        let names: Vec<String> = broken.iter().take(3).map(|(m, _)| m.clone()).collect();
        warnings.push(format!(
            "{} material(s) use shaders whose #include files are missing from the export (e.g. {}); they will render pink until the shader package (Poiyomi Toon for '.poiyomi' shaders) is installed and the materials are re-pointed at it",
            broken.len(),
            names.join(", ")
        ));
        next_steps.push("Install the shader package the materials expect (Poiyomi Toon for '.poiyomi/…' shaders), then select the pink materials and switch them to the installed shader — textures/colours are kept by property name.".into());
    }

    // ---- 10. Assemble the output project
    let out_prefab_rel = format!("{gen_dir}/{}.prefab", opts.avatar_name);
    let prefab_guid = deterministic_guid(&format!("{gen_dir}/{}.prefab", opts.avatar_name));
    generated_files.push((out_prefab_rel.clone(), out_text.clone()));
    generated_files.push((
        format!("{out_prefab_rel}.meta"),
        project::prefab_meta(&prefab_guid),
    ));
    generated_files.push((
        format!("{gen_dir}.meta"),
        project::folder_meta(&deterministic_guid(&format!("{gen_dir}/"))),
    ));
    if fx_bundle.is_some() {
        generated_files.push((
            format!("{gen_dir}/FX.meta"),
            project::folder_meta(&deterministic_guid(&format!("{gen_dir}/FX/"))),
        ));
    }
    for (rel, _) in &generated_files {
        generated.push(OutputFile {
            path: format!("Assets/{rel}"),
            kind: kind_of(rel),
        });
    }

    let source_prefab_rel = prefab_path
        .strip_prefix(&assets_root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| prefab_path.to_string_lossy().to_string());
    let mut assets_copied = 0;
    let mut assets_skipped = 0;
    let mut assets_deduped = 0;
    if !opts.dry_run {
        let out_assets = project::assets_dir(&opts.output);
        if out_assets.exists() {
            bail!(
                "{} already exists; refusing to overwrite an existing project",
                out_assets.display()
            );
        }
        // The SDK2 prefab itself (and its .meta) is replaced by the migrated one.
        // Source files whose GUID a bundled package already provides are not copied (Unity would
        // otherwise see duplicate GUIDs and reassign one at random).
        let mut skip_paths = vec![
            source_prefab_rel.clone(),
            format!("{source_prefab_rel}.meta"),
        ];
        for (guid, path) in &guid_index {
            if package_guids.contains(guid)
                && let Ok(rel) = path.strip_prefix(&assets_root)
            {
                let rel = rel.to_string_lossy().replace('\\', "/");
                assets_deduped += 1;
                skip_paths.push(format!("{rel}.meta"));
                skip_paths.push(rel);
            }
        }
        let (c, s) =
            project::copy_assets(&assets_root, &out_assets, &exclude, &skip_paths, &overrides)?;
        assets_copied = c;
        assets_skipped = s;
        for (rel, content) in &generated_files {
            project::write_text(&out_assets.join(rel), content)?;
        }
        for pkg in &packages {
            pkg.install(&opts.output)?;
        }
        let bundled: Vec<(String, String)> = packages
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect();
        project::write_text(
            &opts.output.join("Packages/vpm-manifest.json"),
            &project::vpm_manifest(&opts.sdk_version, &bundled),
        )?;
        project::write_text(
            &opts.output.join("Packages/manifest.json"),
            &project::unity_manifest(),
        )?;
        project::write_text(
            &opts.output.join("ProjectSettings/ProjectVersion.txt"),
            &project::project_version(&opts.unity_version),
        )?;
    }

    let _ = std::fs::remove_dir_all(&scratch);
    if !relinked_materials.is_empty() {
        next_steps.push(format!(
            "{} material(s) were re-pointed from locked shader copies to their original shader; open each once in the inspector so the shader's upgrade pass runs, and eyeball them.",
            relinked_materials.len()
        ));
    }
    next_steps.push("Open the output folder with the VRChat Creator Companion (Projects → Add Existing Project) so it resolves com.vrchat.avatars, then open it in Unity.".into());
    next_steps.push(format!("Drag Assets/{out_prefab_rel} into a scene; check the Avatar Descriptor's View Position and the FX layer, then Build & Publish from the VRChat SDK panel."));
    if fx_bundle.is_some() {
        next_steps.push("Test each hand gesture: expressions come from the generated 'Gestures' FX layer (either hand triggers).".into());
    }
    if !opts.physbone_roots.is_empty() || !sdk2.dynamic_bones.is_empty() {
        next_steps.push("Play-test the PhysBones (hair/skirt) and tune Pull/Spring/Gravity in the inspector if needed.".into());
    }
    if eye_look_note.is_some() {
        next_steps.push("Eye look was derived from the rig; use the descriptor's eye-look preview to confirm the eyes glance the right way.".into());
    }

    Ok(MigrationReport {
        source_prefab: source_prefab_rel,
        output_project: opts.output.to_string_lossy().to_string(),
        output_prefab: format!("Assets/{out_prefab_rel}"),
        dry_run: opts.dry_run,
        avatar_root: root_name,
        descriptor: descriptor_note,
        view_position,
        root_scale: [root_scale.x, root_scale.y, root_scale.z],
        stripped,
        converted,
        added,
        removed,
        eye_look: eye_look_note,
        blink_blendshape: blink,
        fx: fx_bundle,
        generated,
        assets_copied,
        assets_skipped,
        assets_deduped,
        bundled_packages: packages
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect(),
        relinked_materials,
        warnings,
        next_steps,
        prefab_log: rw.log.clone(),
    })
}

fn kind_of(rel: &str) -> String {
    if rel.ends_with(".meta") {
        "meta".into()
    } else if rel.ends_with(".prefab") {
        "prefab".into()
    } else if rel.ends_with(".anim") {
        "clip".into()
    } else if rel.ends_with(".controller") {
        "controller".into()
    } else if rel.ends_with(".asset") {
        "asset".into()
    } else {
        "file".into()
    }
}

/// The one `.prefab` in the project whose objects include an SDK2 descriptor.
fn find_sdk2_prefab(files: &[PathBuf]) -> Result<PathBuf> {
    let mut hits = Vec::new();
    for f in files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "prefab"))
    {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let parsed = UnityFile::parse_lossy(&text);
        if parsed
            .documents
            .iter()
            .any(|d| d.class_id == MONO_BEHAVIOUR && sdk2::is_sdk2_descriptor(&d.body))
        {
            hits.push(f.clone());
        }
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        0 => bail!("no prefab with an SDK2 VRC_AvatarDescriptor found; pass --prefab"),
        n => bail!("{n} prefabs carry an SDK2 descriptor; pass --prefab to choose"),
    }
}

/// The SDK's `AverageWorldBoneLength` for a chain: mean parent→child distance over the chain's
/// bones (excluding `exclusions` subtrees), in avatar space.
fn average_bone_length(scene: &Scene, root: i64, exclusions: &[i64]) -> f64 {
    let mut total = 0.0;
    let mut count = 0usize;
    let mut stack = vec![root];
    while let Some(t) = stack.pop() {
        let Some(tr) = scene.transforms.get(&t) else {
            continue;
        };
        let p: Trs = scene.world(t);
        for c in &tr.children {
            if exclusions.contains(c) {
                continue;
            }
            let cw = scene.world(*c);
            total += p.position.distance(cw.position);
            count += 1;
            stack.push(*c);
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

/// Find the blink blendshape's index on the viseme SkinnedMeshRenderer's mesh: resolve the
/// renderer's `m_Mesh` to its FBX, list that mesh's blendshape channels in import order, and pick
/// `preferred` (or the first conventional blink name). Returns `(name, index)`.
fn find_blink_index(
    scene: &Scene,
    guid_index: &HashMap<String, PathBuf>,
    smr: i64,
    preferred: Option<&str>,
) -> Result<Option<(String, i64)>> {
    let doc = scene
        .doc(smr)
        .context("viseme SkinnedMeshRenderer not in prefab")?;
    if doc.class_id != SKINNED_MESH_RENDERER {
        bail!("VisemeSkinnedMesh {smr} is not a SkinnedMeshRenderer");
    }
    let mesh_guid = field_str(&doc.body["m_Mesh"], "guid").context("viseme mesh has no guid")?;
    let fbx = guid_index
        .get(mesh_guid)
        .with_context(|| format!("mesh guid {mesh_guid} not in project"))?;
    if !fbx
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
    {
        bail!("viseme mesh source {} is not an FBX", fbx.display());
    }
    let go = scene.owner_of(smr).unwrap_or(0);
    let mesh_name = scene
        .game_objects
        .get(&go)
        .map(|g| g.name.clone())
        .unwrap_or_default();
    let fbx_scene =
        avatar_fbx::FbxScene::load(fbx).with_context(|| format!("loading {}", fbx.display()))?;
    let channels: Vec<String> = fbx_scene
        .blendshape_channels()
        .into_iter()
        .filter(|c| c.mesh_model_name.as_deref() == Some(mesh_name.as_str()))
        .map(|c| c.name)
        .collect();
    if channels.is_empty() {
        return Ok(None);
    }
    let candidates: Vec<String> = match preferred {
        Some(p) => vec![p.to_string()],
        None => vec![
            "Blink".into(),
            "blink".into(),
            "Blink_Both".into(),
            "vrc.blink".into(),
            "まばたき".into(),
        ],
    };
    for cand in candidates {
        if let Some(idx) = channels.iter().position(|c| c == &cand) {
            return Ok(Some((cand, idx as i64)));
        }
    }
    Ok(None)
}

/// Materials whose shader source has an unresolvable relative `#include` (returns
/// `(material Assets-relative path, missing include)`), skipping excluded directories.
fn shader_include_check(
    assets_root: &Path,
    files: &[PathBuf],
    guid_index: &HashMap<String, PathBuf>,
    exclude: &[String],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut shader_cache: HashMap<String, Option<String>> = HashMap::new();
    for mat in files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "mat"))
    {
        let rel = mat
            .strip_prefix(assets_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if exclude
            .iter()
            .any(|e| rel.starts_with(&format!("{}/", e.trim_matches('/'))))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(mat) else {
            continue;
        };
        let Some(guid) = text
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("m_Shader:"))
            .and_then(|v| v.split("guid: ").nth(1))
            .map(|g| {
                g.trim_end_matches('}')
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string()
            })
        else {
            continue;
        };
        let missing = shader_cache.entry(guid.clone()).or_insert_with(|| {
            let shader = guid_index.get(&guid)?;
            if shader.extension().is_none_or(|e| e != "shader") {
                return None;
            }
            let src = std::fs::read_to_string(shader).ok()?;
            let dir = shader.parent()?;
            for line in src.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("#include")
                    && let Some(start) = rest.find('"')
                    && let Some(end) = rest[start + 1..].find('"')
                {
                    let inc = &rest[start + 1..start + 1 + end];
                    if inc.starts_with("Unity")
                        || inc.starts_with("HLSL")
                        || inc.starts_with("Auto")
                        || inc.starts_with("Lighting")
                        || inc.starts_with("Packages/")
                    {
                        continue;
                    }
                    let candidate = dir.join(inc);
                    if !candidate.exists()
                        && !assets_root.join(inc).exists()
                        && !assets_root
                            .parent()
                            .map(|r| r.join(inc).exists())
                            .unwrap_or(false)
                    {
                        return Some(inc.to_string());
                    }
                }
            }
            None
        });
        if let Some(inc) = missing {
            out.push((rel, inc.clone()));
        }
    }
    out.sort();
    out
}

/// Where the source project's assets live.
pub fn source_assets(opts: &MigrateOptions) -> PathBuf {
    opts.source_project.join("Assets")
}

/// A one-line human summary of a report.
pub fn summarize(report: &MigrationReport) -> String {
    format!(
        "{}: {} converted, {} added, {} removed, {} stripped, {} generated file(s), {} warning(s)",
        report.output_prefab,
        report.converted.len(),
        report.added.len(),
        report.removed.len(),
        report.stripped.len(),
        report.generated.len(),
        report.warnings.len()
    )
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
