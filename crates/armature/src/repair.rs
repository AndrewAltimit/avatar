//! Armature repair planning and application.
//!
//! [`plan_repairs`] diagnoses a scene against Unity's humanoid expectations and produces a
//! [`RepairPlan`] of discrete [`RepairEdit`]s. One class is applied natively to a writable FBX
//! ([`apply_plan`]):
//!
//! * **Bone renames** — canonicalize each uniquely-mapped bone to its Unity humanoid name so the
//!   importer auto-configures the avatar. Safe because FBX skin clusters and animation curves
//!   reference bones by object *id*, not name — only the human-facing `Model` name changes.
//!
//! The other classes are *detected and reported only* — they would move geometry, not just relabel
//! metadata, and a metadata-only edit would misrepresent the model:
//!
//! * **Reparents** — restore Unity's required humanoid parent topology, *conservatively*: a reparent
//!   is proposed only when a bone's current parent is clearly wrong humanoid wiring (a different
//!   humanoid bone) or missing, never when it hangs off an unmapped intermediate (twist/accessory)
//!   bone we shouldn't cut through. It is **flagged, not applied**: re-pointing the `OO` connection
//!   alone keeps the bone's local transform (authored against the old parent), shifting its world
//!   rest pose and breaking the bind pose Unity reads to build the humanoid. A correct reparent
//!   recomposes the local transform against the new parent — including the PreRotation/pivots
//!   Mixamo/Maya rigs emit — which is geometry work (Blender territory, PLAN §8).
//! * **Scale / orientation normalization** — a non-standard `UnitScaleFactor` or non-Y-up `UpAxis`
//!   cannot be fixed by relabeling the metadata: that would misrepresent un-transformed geometry.
//!   The correct fix re-transforms skinned data, which is Blender territory (PLAN §8).

use std::collections::BTreeMap;

use avatar_fbx::{FbxDocument, FbxScene};
use serde::Serialize;

use crate::{HumanBone, Skeleton, map_humanoid};

/// One proposed change to an armature.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum RepairEdit {
    /// Rename a bone's `Model` to the canonical Unity humanoid name. *(native)*
    RenameBone { id: i64, from: String, to: String },
    /// A humanoid bone hangs off the wrong parent. *(flagged — a bare connection edit would move
    /// the bone's rest/bind pose; a correct reparent recomposes its transform, which is geometry
    /// work)*
    Reparent {
        id: i64,
        bone: String,
        from_parent: String,
        to_parent: String,
        to_parent_id: i64,
    },
    /// `UnitScaleFactor` is non-standard. *(flagged — needs a geometry transform, not a relabel)*
    NormalizeScale { from_unit_scale: f64 },
    /// `UpAxis` is not Y-up. *(flagged — needs a geometry transform, not a relabel)*
    NormalizeOrientation { from_up_axis: i32 },
}

impl RepairEdit {
    /// True if [`apply_plan`] applies this edit. Only bone renames are applied natively; reparents
    /// and the normalization variants are report-only (they would move geometry, not just relabel
    /// it — see the module docs).
    pub fn is_native(&self) -> bool {
        matches!(self, RepairEdit::RenameBone { .. })
    }

    /// A short human-readable description.
    pub fn summary(&self) -> String {
        match self {
            RepairEdit::RenameBone { from, to, .. } => format!("rename '{from}' -> '{to}'"),
            RepairEdit::Reparent {
                bone,
                from_parent,
                to_parent,
                ..
            } => format!(
                "'{bone}' hangs off '{from_parent}' but Unity expects '{to_parent}' — needs a \
                 geometry-aware reparent (a bare connection edit would move its rest/bind pose), \
                 not a metadata relabel"
            ),
            RepairEdit::NormalizeScale { from_unit_scale } => format!(
                "UnitScaleFactor is {from_unit_scale} (Unity expects 100) — needs a scale transform, \
                 not a metadata relabel"
            ),
            RepairEdit::NormalizeOrientation { from_up_axis } => format!(
                "UpAxis is {from_up_axis} (Unity expects Y-up = 1) — needs a rotation transform, \
                 not a metadata relabel"
            ),
        }
    }
}

/// An ordered set of proposed repairs for one FBX armature.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RepairPlan {
    pub edits: Vec<RepairEdit>,
}

impl RepairPlan {
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// Edits applied natively by [`apply_plan`] (renames, reparents).
    pub fn native(&self) -> impl Iterator<Item = &RepairEdit> {
        self.edits.iter().filter(|e| e.is_native())
    }

    /// Report-only edits (the normalization flags).
    pub fn flagged(&self) -> impl Iterator<Item = &RepairEdit> {
        self.edits.iter().filter(|e| !e.is_native())
    }
}

/// Diagnose `scene` and produce a [`RepairPlan`]: canonical renames + conservative topology
/// reparents (native), plus scale/orientation normalization flags (report-only).
pub fn plan_repairs(scene: &FbxScene) -> RepairPlan {
    let skeleton = Skeleton::from_scene(scene);
    let mapping = map_humanoid(&skeleton);
    let mut edits = Vec::new();

    // Slots unambiguously occupied by a single bone, and the reverse id -> slot lookup.
    let present: BTreeMap<HumanBone, i64> = HumanBone::ALL
        .iter()
        .filter_map(|&hb| mapping.unique_id(hb).map(|id| (hb, id)))
        .collect();
    let id_to_slot: BTreeMap<i64, HumanBone> = present.iter().map(|(&hb, &id)| (id, hb)).collect();

    // 1. Renames — canonicalize to Unity humanoid names.
    for (&slot, &id) in &present {
        if let Some(obj) = scene.object(id) {
            let canonical = slot.name();
            if obj.name != canonical {
                edits.push(RepairEdit::RenameBone {
                    id,
                    from: obj.name.clone(),
                    to: canonical.to_string(),
                });
            }
        }
    }

    // 2. Reparents — restore the required humanoid parent, conservatively.
    for (&slot, &id) in &present {
        let Some(parent_slot) = humanoid_parent(slot, &|s| present.contains_key(&s)) else {
            continue;
        };
        let Some(&expected_pid) = present.get(&parent_slot) else {
            continue;
        };
        let cur = scene.parent_of(id);
        if cur == Some(expected_pid) {
            continue; // already correct
        }
        // Act only on clearly-wrong wiring: a missing parent, or a parent that is a *different*
        // mapped humanoid bone. Leave unmapped intermediates (twist/accessory bones) untouched.
        let act = match cur {
            None => true,
            Some(cp) => id_to_slot.get(&cp).is_some_and(|&s| s != parent_slot),
        };
        if act {
            let from_parent = cur
                .and_then(|cp| scene.object(cp))
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "<none>".to_string());
            edits.push(RepairEdit::Reparent {
                id,
                bone: slot.name().to_string(),
                from_parent,
                to_parent: parent_slot.name().to_string(),
                to_parent_id: expected_pid,
            });
        }
    }

    // 3. Normalization — report-only flags (see module docs).
    if let Some(v) = scene.global_settings.unit_scale_factor
        && (v - 100.0).abs() > f64::EPSILON
    {
        edits.push(RepairEdit::NormalizeScale { from_unit_scale: v });
    }
    if let Some(a) = scene.global_settings.up_axis
        && a != 1
    {
        edits.push(RepairEdit::NormalizeOrientation { from_up_axis: a });
    }

    RepairPlan { edits }
}

/// Unity's required humanoid parent for `bone`, given which slots are present (`present(slot)`),
/// with the standard optional-bone fallbacks: no shoulder → the arm hangs off the upper torso; no
/// upper chest → chest/spine; etc. Returns `None` for `Hips` (the humanoid root) and when no
/// candidate parent is present.
fn humanoid_parent(bone: HumanBone, present: &impl Fn(HumanBone) -> bool) -> Option<HumanBone> {
    use HumanBone::*;
    let first = |cands: &[HumanBone]| cands.iter().copied().find(|&c| present(c));
    match bone {
        Hips => None,
        Spine => Some(Hips),
        Chest => first(&[Spine, Hips]),
        UpperChest => first(&[Chest, Spine, Hips]),
        Neck => first(&[UpperChest, Chest, Spine, Hips]),
        Head => first(&[Neck, UpperChest, Chest, Spine, Hips]),
        LeftShoulder | RightShoulder => first(&[UpperChest, Chest, Spine, Hips]),
        LeftUpperArm => first(&[LeftShoulder, UpperChest, Chest, Spine, Hips]),
        RightUpperArm => first(&[RightShoulder, UpperChest, Chest, Spine, Hips]),
        LeftLowerArm => Some(LeftUpperArm),
        RightLowerArm => Some(RightUpperArm),
        LeftHand => Some(LeftLowerArm),
        RightHand => Some(RightLowerArm),
        LeftUpperLeg | RightUpperLeg => Some(Hips),
        LeftLowerLeg => Some(LeftUpperLeg),
        RightLowerLeg => Some(RightUpperLeg),
        LeftFoot => Some(LeftLowerLeg),
        RightFoot => Some(RightLowerLeg),
        LeftToes => Some(LeftFoot),
        RightToes => Some(RightFoot),
        LeftEye | RightEye | Jaw => first(&[Head]),
    }
}

/// Apply the native edits (renames) of `plan` to `doc` in order. Reparents and normalization flags
/// are report-only and skipped (they would move geometry, not just relabel it). Returns the number
/// of edits applied.
pub fn apply_plan(doc: &mut FbxDocument, plan: &RepairPlan) -> anyhow::Result<usize> {
    let mut applied = 0;
    for edit in &plan.edits {
        match edit {
            RepairEdit::RenameBone { id, to, .. } => {
                doc.rename_object(*id, to)?;
                applied += 1;
            }
            RepairEdit::Reparent { .. }
            | RepairEdit::NormalizeScale { .. }
            | RepairEdit::NormalizeOrientation { .. } => {}
        }
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_fbx::{Connection, FbxObject, FbxScene, GlobalSettings, LocalTransform};

    fn bone(id: i64, name: &str) -> FbxObject {
        FbxObject {
            id,
            node_name: "Model".to_string(),
            name: name.to_string(),
            class: "Model".to_string(),
            subclass: "LimbNode".to_string(),
            transform: LocalTransform::default(),
        }
    }

    fn oo(child: i64, parent: i64) -> Connection {
        Connection {
            kind: "OO".to_string(),
            child,
            parent,
            property: None,
        }
    }

    /// A Mixamo-named rig with all required bones, but `LeftHand` (7) mis-parented onto `Hips` (1)
    /// instead of `LeftForeArm` (6), and non-standard units/axis.
    fn broken_scene() -> FbxScene {
        let objects = vec![
            bone(1, "mixamorig:Hips"),
            bone(2, "mixamorig:Spine"),
            bone(3, "mixamorig:Neck"),
            bone(4, "mixamorig:Head"),
            bone(5, "mixamorig:LeftArm"),
            bone(6, "mixamorig:LeftForeArm"),
            bone(7, "mixamorig:LeftHand"),
            bone(8, "mixamorig:RightArm"),
            bone(9, "mixamorig:RightForeArm"),
            bone(10, "mixamorig:RightHand"),
            bone(11, "mixamorig:LeftUpLeg"),
            bone(12, "mixamorig:LeftLeg"),
            bone(13, "mixamorig:LeftFoot"),
            bone(14, "mixamorig:RightUpLeg"),
            bone(15, "mixamorig:RightLeg"),
            bone(16, "mixamorig:RightFoot"),
        ];
        let connections = vec![
            oo(2, 1),
            oo(3, 2),
            oo(4, 3),
            oo(5, 2),
            oo(6, 5),
            oo(7, 1), // <-- mis-parented: LeftHand under Hips, should be under LeftForeArm (6)
            oo(8, 2),
            oo(9, 8),
            oo(10, 9),
            oo(11, 1),
            oo(12, 11),
            oo(13, 12),
            oo(14, 1),
            oo(15, 14),
            oo(16, 15),
        ];
        FbxScene {
            version: 7400,
            global_settings: GlobalSettings {
                unit_scale_factor: Some(1.0), // non-standard
                up_axis: Some(2),             // Z-up, non-standard
                front_axis: None,
            },
            objects,
            connections,
        }
    }

    #[test]
    fn renames_canonicalize_to_humanoid_names() {
        let plan = plan_repairs(&broken_scene());
        let renamed: Vec<(&str, &str)> = plan
            .edits
            .iter()
            .filter_map(|e| match e {
                RepairEdit::RenameBone { from, to, .. } => Some((from.as_str(), to.as_str())),
                _ => None,
            })
            .collect();
        assert!(renamed.contains(&("mixamorig:Hips", "Hips")));
        assert!(renamed.contains(&("mixamorig:LeftArm", "LeftUpperArm")));
        assert!(renamed.contains(&("mixamorig:LeftForeArm", "LeftLowerArm")));
        // All 16 mixamo names differ from canonical, so all are renamed.
        assert_eq!(renamed.len(), 16, "every mixamo bone should be renamed");
    }

    #[test]
    fn detects_only_the_misparented_bone_but_does_not_apply_it() {
        let plan = plan_repairs(&broken_scene());
        let reparents: Vec<_> = plan
            .edits
            .iter()
            .filter(|e| matches!(e, RepairEdit::Reparent { .. }))
            .collect();
        assert_eq!(reparents.len(), 1, "exactly one reparent expected");
        match reparents[0] {
            RepairEdit::Reparent {
                id,
                bone,
                to_parent,
                to_parent_id,
                ..
            } => {
                assert_eq!(*id, 7);
                assert_eq!(bone, "LeftHand");
                assert_eq!(to_parent, "LeftLowerArm");
                assert_eq!(*to_parent_id, 6);
            }
            _ => unreachable!(),
        }
        // A reparent is reported, never auto-applied: a bare connection edit would move the bone's
        // rest/bind pose (see module docs). It must be flagged, not native.
        assert!(!reparents[0].is_native());
    }

    #[test]
    fn flags_reparent_scale_and_orientation_report_only() {
        let plan = plan_repairs(&broken_scene());
        let flagged: Vec<_> = plan.flagged().collect();
        assert_eq!(flagged.len(), 3);
        assert!(
            flagged
                .iter()
                .any(|e| matches!(e, RepairEdit::Reparent { .. }))
        );
        assert!(
            flagged
                .iter()
                .any(|e| matches!(e, RepairEdit::NormalizeScale { .. }))
        );
        assert!(
            flagged
                .iter()
                .any(|e| matches!(e, RepairEdit::NormalizeOrientation { .. }))
        );
        // Only the 16 renames are applied natively.
        assert_eq!(plan.native().count(), 16);
    }

    #[test]
    fn healthy_rig_needs_no_reparent() {
        // Rebuild the scene with LeftHand correctly parented; only renames + flags should remain.
        let mut scene = broken_scene();
        for c in &mut scene.connections {
            if c.child == 7 {
                c.parent = 6;
            }
        }
        let plan = plan_repairs(&scene);
        assert_eq!(
            plan.edits
                .iter()
                .filter(|e| matches!(e, RepairEdit::Reparent { .. }))
                .count(),
            0
        );
    }

    #[test]
    fn upper_arm_falls_back_to_hips_when_torso_absent() {
        use HumanBone::*;
        // A pathological rig with only Hips + the arm chain — no spine, chest, or shoulders. The
        // upper arms must still find a parent (Hips), like every other slot's final fallback.
        let present = |b: HumanBone| matches!(b, Hips | LeftUpperArm | RightUpperArm);
        assert_eq!(humanoid_parent(LeftUpperArm, &present), Some(Hips));
        assert_eq!(humanoid_parent(RightUpperArm, &present), Some(Hips));
        // And the shoulder is still preferred over Hips when present.
        let with_shoulder = |b: HumanBone| matches!(b, Hips | LeftShoulder | LeftUpperArm);
        assert_eq!(
            humanoid_parent(LeftUpperArm, &with_shoulder),
            Some(LeftShoulder)
        );
    }
}
