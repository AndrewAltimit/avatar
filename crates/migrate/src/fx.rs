//! SDK2 gesture overrides → an SDK3 FX controller.
//!
//! An SDK2 avatar's expressions live in an `AnimatorOverrideController` (`CustomStandingAnims`)
//! that swaps clips of the SDK's `Male_Standing_Pose.fbx` template — one slot per gesture
//! (`FIST`, `HANDOPEN`, `FINGERPOINT`, `VICTORY`, `ROCKNROLL`, `HANDGUN`, `THUMBSUP`), plus
//! locomotion/emote slots. In SDK3 the gesture is an `Int` parameter and the expression a state in
//! the FX playable layer. This module:
//!
//! 1. resolves the override controller and maps each overridden template clip to its slot name
//!    (via the template FBX's `.meta` `fileIDToRecycleName` when the SDK examples were exported
//!    with the avatar; otherwise the SDK2 template's fixed fileIDs, [`SDK2_TEMPLATE_SLOTS`]);
//! 2. for each gesture slot, reads the override `.anim` and **lifts only its blendshape curves**
//!    (SDK2 gesture clips also carried finger-muscle curves; hand poses are the Gesture layer's
//!    job in SDK3, and muscle curves in FX would fight it) into a fresh, clean clip holding each
//!    shape at the clip's final value;
//! 3. builds a `Neutral` clip writing every touched shape back to 0 (Write Defaults off);
//! 4. assembles the FX controller with one either-hand gesture layer (SDK2 fired an override for
//!    whichever hand made the gesture — see `avatar_anim_gen::gesture`).
//!
//! Locomotion/emote overrides (`IDLE`, `WALKFWD`, `PRONE*`, `EMOTE*`, …) are **reported, not
//! migrated**: SDK3's Base/Action layers are a different design (blend trees, root-motion-free
//! locomotion), and an SDK2 idle/walk clip dropped into them is the classic source of the drift
//! bugs this migration exists to remove.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_anim_gen::toggle::{deterministic_guid, native_asset_meta};
use avatar_anim_gen::{
    AnimationClip as GenClip, FloatCurve, GestureLayer, IdGen, Keyframe, ObjectRef, fx_gestures,
};
use avatar_unity_asset::AnimationClip;
use avatar_unity_yaml::{UnityFile, field_i64, field_str, parse_meta};
use serde::Serialize;

use avatar_anim_gen::GESTURE_NAMES;

/// The SDK2 `Male_Standing_Pose.fbx` template's clip fileIDs → slot names (stable across every
/// SDK2 release; the fallback when the template's `.meta` was not exported with the avatar).
pub const SDK2_TEMPLATE_SLOTS: &[(i64, &str)] = &[
    (7400002, "IDLE"),
    (7400004, "PRONEIDLE"),
    (7400006, "EMOTE1"),
    (7400008, "EMOTE2"),
    (7400010, "EMOTE3"),
    (7400012, "EMOTE4"),
    (7400014, "EMOTE5"),
    (7400016, "EMOTE6"),
    (7400018, "EMOTE7"),
    (7400020, "EMOTE8"),
    (7400022, "FALL"),
    (7400024, "CROUCHWALKFWD"),
    (7400026, "CROUCHIDLE"),
    (7400028, "CROUCHWALKRT"),
    (7400030, "SPRINTFWD"),
    (7400032, "RUNFWD"),
    (7400034, "WALKFWD"),
    (7400036, "RUNBACK"),
    (7400038, "STRAFERT"),
    (7400040, "STRAFELT135"),
    (7400042, "STRAFERT135"),
    (7400044, "STRAFELT45"),
    (7400046, "STRAFERT45"),
    (7400048, "RUNSTRAFELT45"),
    (7400050, "RUNSTRAFERT45"),
    (7400052, "FIST"),
    (7400054, "FINGERPOINT"),
    (7400056, "ROCKNROLL"),
    (7400058, "HANDOPEN"),
    (7400060, "THUMBSUP"),
    (7400062, "VICTORY"),
    (7400064, "HANDGUN"),
    (7400066, "PRONEFWD"),
    (7400068, "WALKBACK"),
    (7400070, "RUNSTRAFELT135"),
    (7400072, "RUNSTRAFERT135"),
];

/// SDK2 gesture slot name → SDK3 `GestureLeft`/`GestureRight` value.
pub fn gesture_index(slot: &str) -> Option<usize> {
    match slot {
        "FIST" => Some(1),
        "HANDOPEN" => Some(2),
        "FINGERPOINT" => Some(3),
        "VICTORY" => Some(4),
        "ROCKNROLL" => Some(5),
        "HANDGUN" => Some(6),
        "THUMBSUP" => Some(7),
        _ => None,
    }
}

/// One override slot as found in the SDK2 controller.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OverrideSlot {
    /// Template slot name (`FIST`, `IDLE`, …), or the raw fileID if unresolvable.
    pub slot: String,
    /// Project-relative path of the override clip, if its GUID resolved.
    pub clip_path: Option<String>,
    pub clip_guid: String,
    /// `Some(n)` if this is a gesture slot migrated into the FX layer.
    pub gesture: Option<usize>,
}

/// One migrated gesture clip.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MigratedGesture {
    pub gesture: usize,
    pub gesture_name: String,
    pub source_clip: String,
    /// `(path, shape, value)` blendshape curves lifted into the new clip.
    pub blendshapes: Vec<(String, String, f32)>,
    /// Muscle / transform / other curves dropped from the source clip.
    pub dropped_curves: usize,
    pub generated_file: String,
}

/// A generated file (project-relative path under the output `Assets/`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FxFile {
    pub rel_path: String,
    #[serde(skip)]
    pub content: String,
}

/// The FX generation result.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FxBundle {
    pub files: Vec<FxFile>,
    /// `{fileID, guid}` of the controller (for the descriptor's FX layer).
    pub controller_ref: (i64, String),
    pub slots: Vec<OverrideSlot>,
    pub gestures: Vec<MigratedGesture>,
    /// Slots that were not migrated (locomotion/emotes) with the reason.
    pub skipped: Vec<(String, String)>,
    /// Blendshapes the Neutral clip resets.
    pub neutral_shapes: Vec<(String, String)>,
}

/// Where the FX assets go, relative to the output `Assets/`.
pub struct FxLayout<'a> {
    /// Directory under `Assets/` (e.g. `Avatar/SDK3/FX`).
    pub dir: &'a str,
    /// Controller name (also the file stem).
    pub controller_name: &'a str,
}

/// Build the FX bundle from an SDK2 override controller.
///
/// `assets_root` is the source project's `Assets/` directory; `guid_index` maps GUID → asset path
/// (from `.meta` files); `override_guid` is the `CustomStandingAnims` reference.
pub fn build_fx_from_overrides(
    assets_root: &Path,
    guid_index: &HashMap<String, PathBuf>,
    override_guid: &str,
    layout: &FxLayout,
    analog: bool,
) -> Result<FxBundle> {
    let override_path = guid_index.get(override_guid).with_context(|| {
        format!("override controller guid {override_guid} not found in the project")
    })?;
    let text = std::fs::read_to_string(override_path)
        .with_context(|| format!("reading {}", override_path.display()))?;
    let file = UnityFile::parse(&text)?;
    let doc = file
        .documents
        .iter()
        .find(|d| d.class_id == 221)
        .context("no AnimatorOverrideController (class 221) document")?;
    let clips = doc.body["m_Clips"].as_vec().cloned().unwrap_or_default();

    // Slot names: from the template FBX's .meta if present, else the fixed table.
    let mut slot_names: HashMap<(String, i64), String> = HashMap::new();
    let mut template_metas: HashMap<String, Option<HashMap<i64, String>>> = HashMap::new();
    for c in &clips {
        let orig = &c["m_OriginalClip"];
        let Some(guid) = field_str(orig, "guid") else {
            continue;
        };
        let file_id = field_i64(orig, "fileID").unwrap_or(0);
        let names = template_metas
            .entry(guid.to_string())
            .or_insert_with(|| read_recycle_names(guid_index, guid));
        let name = names
            .as_ref()
            .and_then(|m| m.get(&file_id).cloned())
            .or_else(|| {
                SDK2_TEMPLATE_SLOTS
                    .iter()
                    .find(|(id, _)| *id == file_id)
                    .map(|(_, n)| n.to_string())
            })
            .unwrap_or_else(|| file_id.to_string());
        slot_names.insert((guid.to_string(), file_id), name);
    }

    let mut slots = Vec::new();
    let mut gestures = Vec::new();
    let mut skipped = Vec::new();
    let mut files = Vec::new();
    let mut neutral_shapes: BTreeSet<(String, String)> = BTreeSet::new();
    let mut motions: BTreeMap<usize, (i64, String)> = BTreeMap::new();

    for c in &clips {
        let orig = &c["m_OriginalClip"];
        let over = &c["m_OverrideClip"];
        let (Some(og), Some(over_guid)) = (field_str(orig, "guid"), field_str(over, "guid")) else {
            continue;
        };
        let file_id = field_i64(orig, "fileID").unwrap_or(0);
        let slot = slot_names
            .get(&(og.to_string(), file_id))
            .cloned()
            .unwrap_or_else(|| file_id.to_string());
        let clip_path = guid_index.get(over_guid).map(|p| rel(assets_root, p));
        let gesture = gesture_index(&slot);
        slots.push(OverrideSlot {
            slot: slot.clone(),
            clip_path: clip_path.clone(),
            clip_guid: over_guid.to_string(),
            gesture,
        });
        let Some(g) = gesture else {
            skipped.push((
                slot.clone(),
                "locomotion/emote slot: SDK3 Base/Action layers are a different design; not migrated"
                    .to_string(),
            ));
            continue;
        };
        if motions.contains_key(&g) {
            skipped.push((
                slot.clone(),
                "duplicate gesture slot; first wins".to_string(),
            ));
            continue;
        }
        let Some(src) = guid_index.get(over_guid) else {
            skipped.push((
                slot.clone(),
                format!("override clip guid {over_guid} not in project"),
            ));
            continue;
        };
        let src_text =
            std::fs::read_to_string(src).with_context(|| format!("reading {}", src.display()))?;
        let src_file = UnityFile::parse(&src_text)?;
        let Some(clip) = AnimationClip::from_file(&src_file) else {
            skipped.push((slot.clone(), "override is not an AnimationClip".to_string()));
            continue;
        };
        let mut shapes: Vec<(String, String, f32)> = Vec::new();
        let mut dropped = 0usize;
        for fc in &clip.float_curves {
            if let Some(shape) = fc.attribute.strip_prefix("blendShape.")
                && let Some(v) = fc.final_value()
            {
                shapes.push((fc.path.clone(), shape.to_string(), v));
                continue;
            }
            dropped += 1;
        }
        dropped += clip.transform_curves + clip.pptr_curves;
        shapes.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let gesture_name = GESTURE_NAMES[g].to_string();
        let file_stem = format!("Gesture_{gesture_name}");
        let mut clip_out = GenClip::new(&file_stem);
        for (path, shape, v) in &shapes {
            clip_out.add_float_curve(FloatCurve::blendshape(
                path.clone(),
                shape,
                vec![Keyframe::flat(0.0, *v)],
            ));
            neutral_shapes.insert((path.clone(), shape.clone()));
        }
        let guid = deterministic_guid(&format!(
            "{}/{}/{file_stem}",
            layout.dir, layout.controller_name
        ));
        let rel_path = format!("{}/{file_stem}.anim", layout.dir);
        files.push(FxFile {
            rel_path: rel_path.clone(),
            content: clip_out.to_unity_yaml(7400000),
        });
        files.push(FxFile {
            rel_path: format!("{rel_path}.meta"),
            content: native_asset_meta(&guid, 7400000),
        });
        motions.insert(g, (7400000, guid));
        gestures.push(MigratedGesture {
            gesture: g,
            gesture_name,
            source_clip: clip_path.unwrap_or_else(|| over_guid.to_string()),
            blendshapes: shapes,
            dropped_curves: dropped,
            generated_file: rel_path,
        });
    }

    // Neutral clip: every touched shape back to 0.
    let mut neutral = GenClip::new("Gesture_Neutral");
    for (path, shape) in &neutral_shapes {
        neutral.add_float_curve(FloatCurve::blendshape(
            path.clone(),
            shape,
            vec![Keyframe::flat(0.0, 0.0)],
        ));
    }
    let neutral_guid = deterministic_guid(&format!(
        "{}/{}/Gesture_Neutral",
        layout.dir, layout.controller_name
    ));
    files.push(FxFile {
        rel_path: format!("{}/Gesture_Neutral.anim", layout.dir),
        content: neutral.to_unity_yaml(7400000),
    });
    files.push(FxFile {
        rel_path: format!("{}/Gesture_Neutral.anim.meta", layout.dir),
        content: native_asset_meta(&neutral_guid, 7400000),
    });

    // The controller: one either-hand layer; in analog mode each gesture state is a per-hand
    // BlendTree on GestureLeftWeight/GestureRightWeight (SDK2 Vive trigger-depth semantics).
    let mut layer =
        GestureLayer::either_hand("Gestures", ObjectRef::external(7400000, neutral_guid, 2));
    if analog {
        layer = layer.analog();
    }
    for (g, (fid, guid)) in &motions {
        layer = layer.motion(*g, ObjectRef::external(*fid, guid.clone(), 2));
    }
    let mut ids = IdGen::new(layout.controller_name);
    let controller_id = {
        // fx_gestures allocates the controller id first; mirror that so we can reference it.
        let mut probe = ids.clone();
        probe.alloc()
    };
    let controller_yaml = fx_gestures(layout.controller_name, &[layer], &[], &mut ids);
    let controller_guid = deterministic_guid(&format!(
        "{}/{}.controller",
        layout.dir, layout.controller_name
    ));
    files.push(FxFile {
        rel_path: format!("{}/{}.controller", layout.dir, layout.controller_name),
        content: controller_yaml,
    });
    files.push(FxFile {
        rel_path: format!("{}/{}.controller.meta", layout.dir, layout.controller_name),
        content: native_asset_meta(&controller_guid, controller_id),
    });

    if gestures.is_empty() {
        bail!("the override controller has no gesture slots to migrate");
    }

    Ok(FxBundle {
        files,
        controller_ref: (controller_id, controller_guid),
        slots,
        gestures,
        skipped,
        neutral_shapes: neutral_shapes.into_iter().collect(),
    })
}

/// `fileIDToRecycleName` (or the newer `internalIDToNameTable`) of a model importer `.meta`, if
/// the asset with `guid` is present with its meta.
fn read_recycle_names(
    guid_index: &HashMap<String, PathBuf>,
    guid: &str,
) -> Option<HashMap<i64, String>> {
    let path = guid_index.get(guid)?;
    let meta = avatar_unity_yaml::meta_path(path);
    let text = std::fs::read_to_string(&meta).ok()?;
    let yaml = parse_meta(&text)?;
    let importer = &yaml["ModelImporter"];
    let mut out = HashMap::new();
    if let Some(map) = importer["fileIDToRecycleName"].as_hash() {
        for (k, v) in map {
            let id = k
                .as_i64()
                .or_else(|| k.as_str().and_then(|s| s.parse().ok()));
            if let (Some(id), Some(name)) = (id, v.as_str()) {
                out.insert(id, name.to_string());
            }
        }
    }
    if let Some(list) = importer["internalIDToNameTable"].as_vec() {
        for e in list {
            let first = e["first"]
                .as_hash()
                .and_then(|h| h.values().next().cloned());
            let id = first.and_then(|f| f.as_i64());
            if let (Some(id), Some(name)) = (id, e["second"].as_str()) {
                out.insert(id, name.to_string());
            }
        }
    }
    Some(out)
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}
