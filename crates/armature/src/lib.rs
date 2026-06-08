//! Skeleton extraction and VRChat humanoid rig validation.
//!
//! Given a parsed [`avatar_fbx::FbxScene`], build a bone hierarchy from its `Model` objects,
//! classify bone names ([`humanoid`]), then resolve a Unity humanoid mapping using the skeleton
//! hierarchy. Depth ordering disambiguates the spine, arm, and leg chains, which names alone
//! cannot do reliably. Finally, report what's missing or mis-mapped against VRChat's rig
//! requirements.
//!
//! References: <https://creators.vrchat.com/avatars/rig-requirements/>.

pub mod humanoid;
pub mod repair;

use std::collections::{BTreeMap, HashSet};

use avatar_fbx::FbxScene;
use serde::Serialize;

pub use humanoid::{BoneCategory, HumanBone, NameInfo, Requirement, Side};
pub use repair::{RepairEdit, RepairPlan, apply_plan, plan_repairs};

/// A bone in the extracted skeleton (one `Model` object).
#[derive(Debug, Clone)]
pub struct Bone {
    pub id: i64,
    pub name: String,
    /// FBX model sub-class, e.g. `"LimbNode"`, `"Null"`, `"Mesh"`.
    pub subclass: String,
    /// Parent bone id, if the parent is also a `Model`. `None` means this is a skeleton root.
    pub parent: Option<i64>,
    /// Name classification (category, side, leaf flag).
    pub info: NameInfo,
}

impl Bone {
    fn is_bone_like(&self) -> bool {
        matches!(self.subclass.as_str(), "LimbNode" | "Limb" | "Root")
    }
}

/// A skeleton built from the `Model` objects of an FBX scene.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// Build a skeleton from every `Model` object in the scene.
    pub fn from_scene(scene: &FbxScene) -> Self {
        let model_ids: HashSet<i64> = scene.models().map(|m| m.id).collect();

        let bones = scene
            .models()
            .map(|m| {
                let parent = scene.parent_of(m.id).filter(|pid| model_ids.contains(pid));
                Bone {
                    id: m.id,
                    name: m.name.clone(),
                    subclass: m.subclass.clone(),
                    parent,
                    info: humanoid::classify(&m.name),
                }
            })
            .collect();

        Skeleton { bones }
    }

    /// Skeleton root bones (models with no model parent).
    pub fn roots(&self) -> impl Iterator<Item = &Bone> {
        self.bones.iter().filter(|b| b.parent.is_none())
    }

    pub fn bone(&self, id: i64) -> Option<&Bone> {
        self.bones.iter().find(|b| b.id == id)
    }

    /// Number of parent hops from this bone up to a root (roots are depth 0). Capped to guard
    /// against malformed cyclic hierarchies.
    pub fn depth(&self, id: i64) -> usize {
        let mut depth = 0;
        let mut cur = self.bone(id).and_then(|b| b.parent);
        while let Some(p) = cur {
            depth += 1;
            if depth > 4096 {
                break;
            }
            cur = self.bone(p).and_then(|b| b.parent);
        }
        depth
    }

    /// True if `id` or any of its descendants is a bone-like node (used to tell an armature
    /// root apart from a plain mesh root).
    fn subtree_has_bone(&self, id: i64) -> bool {
        let mut visited = HashSet::new();
        self.subtree_has_bone_inner(id, &mut visited)
    }

    fn subtree_has_bone_inner(&self, id: i64, visited: &mut HashSet<i64>) -> bool {
        // Guard against malformed cyclic parent links: a bone already on the path can't add
        // anything new, and revisiting it would recurse forever.
        if !visited.insert(id) {
            return false;
        }
        if self.bone(id).is_some_and(Bone::is_bone_like) {
            return true;
        }
        self.bones
            .iter()
            .filter(|b| b.parent == Some(id))
            .any(|child| self.subtree_has_bone_inner(child.id, visited))
    }
}

/// The resolved humanoid mapping plus the bones that were deliberately ignored.
#[derive(Debug, Clone, Default)]
pub struct HumanoidMapping {
    /// Humanoid slot -> source bone name(s) assigned to it.
    pub slots: BTreeMap<HumanBone, Vec<String>>,
    /// Humanoid slot -> source bone id(s), parallel to [`slots`](Self::slots). Lets repair planning
    /// address the exact FBX object behind each slot.
    pub slot_ids: BTreeMap<HumanBone, Vec<i64>>,
    /// Ids of bones assigned to some slot.
    pub assigned_ids: HashSet<i64>,
    pub finger_count: usize,
    pub leaf_end_count: usize,
}

impl HumanoidMapping {
    /// The single bone id mapped to `slot`, if exactly one bone occupies it (the unambiguous case).
    pub fn unique_id(&self, slot: HumanBone) -> Option<i64> {
        match self.slot_ids.get(&slot).map(Vec::as_slice) {
            Some([id]) => Some(*id),
            _ => None,
        }
    }
}

/// Resolve a Unity humanoid mapping from a skeleton, using names for unambiguous bones and the
/// hierarchy (depth ordering) for the spine / arm / leg chains.
pub fn map_humanoid(skeleton: &Skeleton) -> HumanoidMapping {
    let mut mapping = HumanoidMapping::default();

    // Pass 1: direct, unambiguous categories.
    for bone in &skeleton.bones {
        if bone.info.is_leaf_end {
            mapping.leaf_end_count += 1;
            continue;
        }
        let Some(cat) = bone.info.category else {
            continue;
        };
        let slot = match cat {
            BoneCategory::Finger => {
                mapping.finger_count += 1;
                None
            }
            BoneCategory::Hips => Some(HumanBone::Hips),
            BoneCategory::Neck => Some(HumanBone::Neck),
            BoneCategory::Head => Some(HumanBone::Head),
            BoneCategory::Jaw => Some(HumanBone::Jaw),
            BoneCategory::Shoulder => sided(
                bone.info.side,
                HumanBone::LeftShoulder,
                HumanBone::RightShoulder,
            ),
            BoneCategory::Hand => sided(bone.info.side, HumanBone::LeftHand, HumanBone::RightHand),
            BoneCategory::Foot => sided(bone.info.side, HumanBone::LeftFoot, HumanBone::RightFoot),
            BoneCategory::Toes => sided(bone.info.side, HumanBone::LeftToes, HumanBone::RightToes),
            BoneCategory::Eye => sided(bone.info.side, HumanBone::LeftEye, HumanBone::RightEye),
            // Chain categories are resolved by hierarchy in pass 2.
            BoneCategory::Spine
            | BoneCategory::Chest
            | BoneCategory::UpperChest
            | BoneCategory::UpperArm
            | BoneCategory::LowerArm
            | BoneCategory::UpperLeg
            | BoneCategory::LowerLeg => None,
        };
        if let Some(slot) = slot {
            assign(&mut mapping, slot, bone);
        }
    }

    // Pass 2: chains, ordered proximal -> distal by hierarchy depth.
    let spine_group = &[
        BoneCategory::Spine,
        BoneCategory::Chest,
        BoneCategory::UpperChest,
    ];
    assign_chain(
        skeleton,
        &mut mapping,
        Side::None,
        spine_group,
        &[HumanBone::Spine, HumanBone::Chest, HumanBone::UpperChest],
    );

    let arm_group = &[BoneCategory::UpperArm, BoneCategory::LowerArm];
    assign_chain(
        skeleton,
        &mut mapping,
        Side::Left,
        arm_group,
        &[HumanBone::LeftUpperArm, HumanBone::LeftLowerArm],
    );
    assign_chain(
        skeleton,
        &mut mapping,
        Side::Right,
        arm_group,
        &[HumanBone::RightUpperArm, HumanBone::RightLowerArm],
    );

    let leg_group = &[BoneCategory::UpperLeg, BoneCategory::LowerLeg];
    assign_chain(
        skeleton,
        &mut mapping,
        Side::Left,
        leg_group,
        &[HumanBone::LeftUpperLeg, HumanBone::LeftLowerLeg],
    );
    assign_chain(
        skeleton,
        &mut mapping,
        Side::Right,
        leg_group,
        &[HumanBone::RightUpperLeg, HumanBone::RightLowerLeg],
    );

    mapping
}

fn sided(side: Side, left: HumanBone, right: HumanBone) -> Option<HumanBone> {
    match side {
        Side::Left => Some(left),
        Side::Right => Some(right),
        Side::None => None,
    }
}

fn assign(mapping: &mut HumanoidMapping, slot: HumanBone, bone: &Bone) {
    mapping
        .slots
        .entry(slot)
        .or_default()
        .push(bone.name.clone());
    mapping.slot_ids.entry(slot).or_default().push(bone.id);
    mapping.assigned_ids.insert(bone.id);
}

/// Gather the (non-leaf) bones of a chain group on a given side, order them proximal -> distal
/// by depth, and assign them to `slots` in order. Extra bones beyond the slot list stay unmapped.
fn assign_chain(
    skeleton: &Skeleton,
    mapping: &mut HumanoidMapping,
    side: Side,
    group: &[BoneCategory],
    slots: &[HumanBone],
) {
    let mut members: Vec<&Bone> = skeleton
        .bones
        .iter()
        .filter(|b| !b.info.is_leaf_end)
        .filter(|b| b.info.category.is_some_and(|c| group.contains(&c)))
        .filter(|b| side == Side::None || b.info.side == side)
        .collect();

    // Proximal (shallow) first; tie-break by name for determinism.
    members.sort_by(|a, b| {
        skeleton
            .depth(a.id)
            .cmp(&skeleton.depth(b.id))
            .then_with(|| a.name.cmp(&b.name))
    });

    if members.len() <= slots.len() {
        // Enough slots for everyone: fill proximal -> distal. group[i] <-> slots[i] is a 1:1
        // ordered correspondence, so depth order lands each bone in its slot.
        for (bone, &slot) in members.iter().zip(slots.iter()) {
            assign(mapping, slot, bone);
        }
        return;
    }

    // More members than slots — e.g. an extra spine segment between Spine and Chest. Pure depth
    // order would shift an explicitly-named Chest/UpperChest out of its slot (and drop the top
    // one), so anchor each upper slot to the deepest bone whose category matches it, fill the base
    // slot with the proximal-most leftover, and leave any surplus segments unmapped.
    let mut used = vec![false; members.len()];
    let mut chosen: Vec<Option<usize>> = vec![None; slots.len()];

    // Upper slots (distal-first), each matched to the deepest unused bone of its own category.
    for si in (1..slots.len()).rev() {
        let want = group[si];
        let pick = members
            .iter()
            .enumerate()
            .filter(|&(i, b)| !used[i] && b.info.category == Some(want))
            .max_by_key(|&(i, _)| skeleton.depth(members[i].id))
            .map(|(i, _)| i);
        if let Some(i) = pick {
            used[i] = true;
            chosen[si] = Some(i);
        }
    }
    // Base slot: the proximal-most remaining member.
    if let Some(i) = (0..members.len()).find(|&i| !used[i]) {
        used[i] = true;
        chosen[0] = Some(i);
    }
    // Backfill any upper slot no category matched, proximal -> distal, so a slot isn't dropped
    // just because no name lined up with it.
    for entry in chosen.iter_mut().skip(1) {
        if entry.is_none()
            && let Some(i) = (0..members.len()).find(|&i| !used[i])
        {
            used[i] = true;
            *entry = Some(i);
        }
    }

    for (si, &maybe) in chosen.iter().enumerate() {
        if let Some(i) = maybe {
            assign(mapping, slots[si], members[i]);
        }
    }
}

/// Result of validating a skeleton against VRChat humanoid rig requirements.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ArmatureReport {
    pub total_models: usize,
    pub bone_like_count: usize,
    /// Roots that are (or contain) a skeleton — VRChat expects exactly one.
    pub armature_roots: Vec<String>,
    /// Roots with no bones (meshes, props) — informational.
    pub mesh_roots: Vec<String>,
    /// Inferred humanoid bone -> the source bone name(s) that mapped to it.
    pub mapped: BTreeMap<String, Vec<String>>,
    /// Humanoid slots mapped by more than one source bone (likely a genuine duplicate bone).
    pub duplicate_mappings: BTreeMap<String, Vec<String>>,
    pub missing_required: Vec<String>,
    pub missing_recommended: Vec<String>,
    /// Bone-like nodes that matched no humanoid slot (accessory/twist/dynamic bones, etc.).
    pub unmapped_bones: Vec<String>,
    /// Count of finger bones recognized and excluded from body mapping.
    pub ignored_finger_bones: usize,
    /// Count of leaf `*_End` bones recognized and excluded.
    pub ignored_leaf_bones: usize,
}

impl ArmatureReport {
    /// True if every Unity-required humanoid bone is present.
    pub fn is_humanoid_ready(&self) -> bool {
        self.missing_required.is_empty()
    }
}

/// Analyze a scene's skeleton against VRChat humanoid rig requirements.
pub fn analyze(scene: &FbxScene) -> ArmatureReport {
    let skeleton = Skeleton::from_scene(scene);
    let mapping = map_humanoid(&skeleton);

    let mut duplicate_mappings = BTreeMap::new();
    let mut mapped = BTreeMap::new();
    for (hb, names) in &mapping.slots {
        if names.len() > 1 {
            duplicate_mappings.insert(hb.name().to_string(), names.clone());
        }
        mapped.insert(hb.name().to_string(), names.clone());
    }

    let mut missing_required = Vec::new();
    let mut missing_recommended = Vec::new();
    for hb in HumanBone::ALL {
        if mapping.slots.contains_key(&hb) {
            continue;
        }
        match hb.requirement() {
            Requirement::Required => missing_required.push(hb.name().to_string()),
            Requirement::Recommended => missing_recommended.push(hb.name().to_string()),
            Requirement::Optional => {}
        }
    }

    // Bone-like nodes that weren't mapped, aren't fingers, and aren't leaf ends.
    let unmapped_bones = skeleton
        .bones
        .iter()
        .filter(|b| b.is_bone_like())
        .filter(|b| !mapping.assigned_ids.contains(&b.id))
        .filter(|b| !b.info.is_leaf_end)
        .filter(|b| b.info.category != Some(BoneCategory::Finger))
        .map(|b| b.name.clone())
        .collect();

    let mut armature_roots = Vec::new();
    let mut mesh_roots = Vec::new();
    for root in skeleton.roots() {
        if skeleton.subtree_has_bone(root.id) {
            armature_roots.push(root.name.clone());
        } else {
            mesh_roots.push(root.name.clone());
        }
    }

    let bone_like_count = skeleton.bones.iter().filter(|b| b.is_bone_like()).count();

    ArmatureReport {
        total_models: scene.models().count(),
        bone_like_count,
        armature_roots,
        mesh_roots,
        mapped,
        duplicate_mappings,
        missing_required,
        missing_recommended,
        unmapped_bones,
        ignored_finger_bones: mapping.finger_count,
        ignored_leaf_bones: mapping.leaf_end_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(id: i64, name: &str, parent: Option<i64>) -> Bone {
        Bone {
            id,
            name: name.to_string(),
            subclass: "LimbNode".to_string(),
            parent,
            info: humanoid::classify(name),
        }
    }

    /// A trimmed Mixamo-style skeleton exercising the chains the hierarchy pass must resolve:
    /// numbered spine, UpLeg/Leg legs, a leaf `_End`, and a finger bone.
    fn mixamo_like() -> Skeleton {
        Skeleton {
            bones: vec![
                bone(1, "mixamorig:Hips", None),
                bone(2, "mixamorig:Spine", Some(1)),
                bone(3, "mixamorig:Spine1", Some(2)),
                bone(4, "mixamorig:Spine2", Some(3)),
                bone(5, "mixamorig:Neck", Some(4)),
                bone(6, "mixamorig:Head", Some(5)),
                bone(7, "mixamorig:HeadTop_End", Some(6)),
                bone(8, "mixamorig:LeftShoulder", Some(4)),
                bone(9, "mixamorig:LeftArm", Some(8)),
                bone(10, "mixamorig:LeftForeArm", Some(9)),
                bone(11, "mixamorig:LeftHand", Some(10)),
                bone(12, "mixamorig:LeftHandMiddle1", Some(11)),
                bone(13, "mixamorig:LeftUpLeg", Some(1)),
                bone(14, "mixamorig:LeftLeg", Some(13)),
                bone(15, "mixamorig:LeftFoot", Some(14)),
                bone(16, "mixamorig:LeftToeBase", Some(15)),
            ],
        }
    }

    #[test]
    fn spine_chain_resolved_by_depth() {
        let m = map_humanoid(&mixamo_like());
        assert_eq!(m.slots[&HumanBone::Spine], vec!["mixamorig:Spine"]);
        assert_eq!(m.slots[&HumanBone::Chest], vec!["mixamorig:Spine1"]);
        assert_eq!(m.slots[&HumanBone::UpperChest], vec!["mixamorig:Spine2"]);
    }

    #[test]
    fn leg_chain_resolved_by_depth() {
        let m = map_humanoid(&mixamo_like());
        assert_eq!(
            m.slots[&HumanBone::LeftUpperLeg],
            vec!["mixamorig:LeftUpLeg"]
        );
        // The regression: bare "Leg" is the lower leg, not a duplicate upper leg.
        assert_eq!(m.slots[&HumanBone::LeftLowerLeg], vec!["mixamorig:LeftLeg"]);
    }

    #[test]
    fn fingers_and_leaves_excluded_hand_kept() {
        let m = map_humanoid(&mixamo_like());
        assert_eq!(m.slots[&HumanBone::LeftHand], vec!["mixamorig:LeftHand"]);
        assert_eq!(m.finger_count, 1);
        assert_eq!(m.leaf_end_count, 1);
        // The finger and leaf bones must not appear in any slot.
        let all_mapped: Vec<&String> = m.slots.values().flatten().collect();
        assert!(!all_mapped.iter().any(|n| n.contains("Middle")));
        assert!(!all_mapped.iter().any(|n| n.contains("_End")));
    }

    #[test]
    fn no_duplicate_mappings() {
        let m = map_humanoid(&mixamo_like());
        for (slot, names) in &m.slots {
            assert_eq!(names.len(), 1, "slot {slot:?} mapped {names:?}");
        }
    }

    #[test]
    fn extra_spine_segment_keeps_explicit_chest_and_upperchest() {
        // Spine, a surplus Spine1, then explicitly named Chest and UpperChest. Pure depth-order
        // filling would push "Chest" into the UpperChest slot and drop "UpperChest"; the anchored
        // pass keeps each explicit name in its own slot and leaves the surplus segment unmapped.
        let s = Skeleton {
            bones: vec![
                bone(1, "Hips", None),
                bone(2, "Spine", Some(1)),
                bone(3, "Spine1", Some(2)),
                bone(4, "Chest", Some(3)),
                bone(5, "UpperChest", Some(4)),
                bone(6, "Neck", Some(5)),
                bone(7, "Head", Some(6)),
            ],
        };
        let m = map_humanoid(&s);
        assert_eq!(m.slots[&HumanBone::Spine], vec!["Spine"]);
        assert_eq!(m.slots[&HumanBone::Chest], vec!["Chest"]);
        assert_eq!(m.slots[&HumanBone::UpperChest], vec!["UpperChest"]);
        let mapped: Vec<&String> = m.slots.values().flatten().collect();
        assert!(
            !mapped.iter().any(|n| n.as_str() == "Spine1"),
            "the surplus spine segment must stay unmapped, not displace a named slot"
        );
    }

    #[test]
    fn subtree_has_bone_terminates_on_parent_cycle() {
        // Two non-bone-like nodes that point at each other — a malformed hierarchy. Without a
        // cycle guard this recurses until the stack overflows; it must terminate and report no
        // bone in the subtree.
        let null = |id: i64, parent: i64| Bone {
            id,
            name: format!("Null{id}"),
            subclass: "Null".to_string(),
            parent: Some(parent),
            info: humanoid::classify(""),
        };
        let s = Skeleton {
            bones: vec![null(1, 2), null(2, 1)],
        };
        assert!(!s.subtree_has_bone(1));
    }
}
