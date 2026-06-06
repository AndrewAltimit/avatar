//! Unity humanoid bones, plus name classification used by the (hierarchy-aware) mapper.
//!
//! Name matching alone cannot reliably resolve a humanoid rig: conventions disagree on whether
//! "Leg" means the upper or lower leg, spine bones are often just numbered (`Spine`, `Spine1`,
//! `Spine2`), and hands carry finger and leaf "_End" bones that must not be mistaken for the
//! body bone. So this module only *classifies* a bone name into a coarse [`BoneCategory`] + side
//! plus flags. The actual slot assignment for ambiguous chains (spine / arms / legs) is done by
//! the mapper in `lib.rs` using the skeleton hierarchy (depth ordering).

use serde::Serialize;

/// A Unity humanoid bone slot (a pragmatic subset of `HumanBodyBones`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum HumanBone {
    Hips,
    Spine,
    Chest,
    UpperChest,
    Neck,
    Head,
    LeftShoulder,
    RightShoulder,
    LeftUpperArm,
    RightUpperArm,
    LeftLowerArm,
    RightLowerArm,
    LeftHand,
    RightHand,
    LeftUpperLeg,
    RightUpperLeg,
    LeftLowerLeg,
    RightLowerLeg,
    LeftFoot,
    RightFoot,
    LeftToes,
    RightToes,
    LeftEye,
    RightEye,
    Jaw,
}

/// How important a bone is for a VRChat humanoid avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Requirement {
    /// Required by Unity's humanoid rig (the avatar will not import as humanoid without it).
    Required,
    /// Not strictly required by Unity, but VRChat expects it for a complete rig
    /// (full spine chain + shoulders).
    Recommended,
    /// Optional; enables extra features (eye look, jaw/visemes, toe articulation).
    Optional,
}

impl HumanBone {
    pub const ALL: [HumanBone; 25] = [
        HumanBone::Hips,
        HumanBone::Spine,
        HumanBone::Chest,
        HumanBone::UpperChest,
        HumanBone::Neck,
        HumanBone::Head,
        HumanBone::LeftShoulder,
        HumanBone::RightShoulder,
        HumanBone::LeftUpperArm,
        HumanBone::RightUpperArm,
        HumanBone::LeftLowerArm,
        HumanBone::RightLowerArm,
        HumanBone::LeftHand,
        HumanBone::RightHand,
        HumanBone::LeftUpperLeg,
        HumanBone::RightUpperLeg,
        HumanBone::LeftLowerLeg,
        HumanBone::RightLowerLeg,
        HumanBone::LeftFoot,
        HumanBone::RightFoot,
        HumanBone::LeftToes,
        HumanBone::RightToes,
        HumanBone::LeftEye,
        HumanBone::RightEye,
        HumanBone::Jaw,
    ];

    pub fn requirement(self) -> Requirement {
        use HumanBone::*;
        use Requirement::*;
        match self {
            Hips | Spine | Head | LeftUpperArm | RightUpperArm | LeftLowerArm | RightLowerArm
            | LeftHand | RightHand | LeftUpperLeg | RightUpperLeg | LeftLowerLeg
            | RightLowerLeg | LeftFoot | RightFoot => Required,
            Chest | Neck | LeftShoulder | RightShoulder => Recommended,
            UpperChest | LeftToes | RightToes | LeftEye | RightEye | Jaw => Optional,
        }
    }

    pub fn name(self) -> &'static str {
        use HumanBone::*;
        match self {
            Hips => "Hips",
            Spine => "Spine",
            Chest => "Chest",
            UpperChest => "UpperChest",
            Neck => "Neck",
            Head => "Head",
            LeftShoulder => "LeftShoulder",
            RightShoulder => "RightShoulder",
            LeftUpperArm => "LeftUpperArm",
            RightUpperArm => "RightUpperArm",
            LeftLowerArm => "LeftLowerArm",
            RightLowerArm => "RightLowerArm",
            LeftHand => "LeftHand",
            RightHand => "RightHand",
            LeftUpperLeg => "LeftUpperLeg",
            RightUpperLeg => "RightUpperLeg",
            LeftLowerLeg => "LeftLowerLeg",
            RightLowerLeg => "RightLowerLeg",
            LeftFoot => "LeftFoot",
            RightFoot => "RightFoot",
            LeftToes => "LeftToes",
            RightToes => "RightToes",
            LeftEye => "LeftEye",
            RightEye => "RightEye",
            Jaw => "Jaw",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    Left,
    Right,
    None,
}

/// A coarse bone category inferred from a name. Chains (`Spine`/`Chest`/`UpperChest`,
/// `UpperArm`/`LowerArm`, `UpperLeg`/`LowerLeg`) are refined by hierarchy in the mapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoneCategory {
    Hips,
    Spine,
    Chest,
    UpperChest,
    Neck,
    Head,
    Shoulder,
    UpperArm,
    LowerArm,
    Hand,
    UpperLeg,
    LowerLeg,
    Foot,
    Toes,
    Eye,
    Jaw,
    /// A finger bone — ignored for body mapping (we don't expose finger slots yet).
    Finger,
}

/// The result of classifying a single bone name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameInfo {
    pub category: Option<BoneCategory>,
    pub side: Side,
    /// A Mixamo-style leaf such as `*_End` / `HeadTop_End` — not a real humanoid bone.
    pub is_leaf_end: bool,
}

/// Synonym substrings checked in priority order (most specific first). The first whose
/// substring is contained in the normalized (alnum-only) name wins. Spine/leg numbering
/// ambiguity is left for the hierarchy pass; here we only need a category in the right group.
const CORE_SYNONYMS: &[(&str, BoneCategory)] = &[
    ("upperchest", BoneCategory::UpperChest),
    ("spine3", BoneCategory::UpperChest),
    ("spine03", BoneCategory::UpperChest),
    ("upperarm", BoneCategory::UpperArm),
    ("uparm", BoneCategory::UpperArm),
    ("lowerarm", BoneCategory::LowerArm),
    ("forearm", BoneCategory::LowerArm),
    ("elbow", BoneCategory::LowerArm),
    ("upperleg", BoneCategory::UpperLeg),
    ("upleg", BoneCategory::UpperLeg),
    ("thigh", BoneCategory::UpperLeg),
    ("lowerleg", BoneCategory::LowerLeg),
    ("loleg", BoneCategory::LowerLeg),
    ("calf", BoneCategory::LowerLeg),
    ("shin", BoneCategory::LowerLeg),
    ("knee", BoneCategory::LowerLeg),
    ("shoulder", BoneCategory::Shoulder),
    ("clavicle", BoneCategory::Shoulder),
    ("collar", BoneCategory::Shoulder),
    ("chest", BoneCategory::Chest),
    ("spine2", BoneCategory::Chest),
    ("spine02", BoneCategory::Chest),
    ("spine", BoneCategory::Spine),
    ("neck", BoneCategory::Neck),
    ("head", BoneCategory::Head),
    ("toebase", BoneCategory::Toes),
    ("toes", BoneCategory::Toes),
    ("toe", BoneCategory::Toes),
    ("ball", BoneCategory::Toes),
    ("foot", BoneCategory::Foot),
    ("ankle", BoneCategory::Foot),
    ("hand", BoneCategory::Hand),
    ("wrist", BoneCategory::Hand),
    ("hips", BoneCategory::Hips),
    ("hip", BoneCategory::Hips),
    ("pelvis", BoneCategory::Hips),
    ("eye", BoneCategory::Eye),
    ("jaw", BoneCategory::Jaw),
    // Generic fallbacks last, so specific matches above take precedence. A bare "arm"/"leg"
    // with no upper/lower qualifier defaults to the proximal/distal convention most rigs use
    // (arm -> upper, leg -> lower); the hierarchy pass corrects either way.
    ("arm", BoneCategory::UpperArm),
    ("leg", BoneCategory::LowerLeg),
];

const FINGER_TOKENS: &[&str] = &[
    "thumb", "index", "middle", "ring", "pinky", "little", "finger",
];
const LEFT_TOKENS: &[&str] = &["left", "l", "lt", "lf", "lhs"];
const RIGHT_TOKENS: &[&str] = &["right", "r", "rt", "rgt", "rhs"];

/// Classify a rig bone name into a coarse category, side, and leaf flag.
pub fn classify(name: &str) -> NameInfo {
    let tokens = tokenize(name);
    let joined: String = tokens.concat();
    let is_leaf_end = tokens.iter().any(|t| t == "end");
    let side = detect_side(&tokens);

    // Finger tokens often carry a joint number (`Middle1`, `Thumb3`), so match against the
    // joined name rather than whole tokens.
    let category = if FINGER_TOKENS.iter().any(|f| joined.contains(f)) {
        Some(BoneCategory::Finger)
    } else {
        CORE_SYNONYMS
            .iter()
            .find(|(syn, _)| joined.contains(syn))
            .map(|(_, c)| *c)
    };

    NameInfo {
        category,
        side,
        is_leaf_end,
    }
}

fn detect_side(tokens: &[String]) -> Side {
    for t in tokens {
        if LEFT_TOKENS.contains(&t.as_str()) {
            return Side::Left;
        }
        if RIGHT_TOKENS.contains(&t.as_str()) {
            return Side::Right;
        }
    }
    let joined = tokens.join("");
    if joined.contains("left") {
        Side::Left
    } else if joined.contains("right") {
        Side::Right
    } else {
        Side::None
    }
}

/// Lowercase, split camelCase boundaries and separators, drop empty tokens.
fn tokenize(name: &str) -> Vec<String> {
    let mut spaced = String::with_capacity(name.len() * 2);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if c.is_ascii_uppercase()
            && matches!(prev, Some(p) if p.is_ascii_lowercase() || p.is_ascii_digit())
        {
            spaced.push(' ');
        }
        spaced.push(c);
        prev = Some(c);
    }
    spaced
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(name: &str) -> Option<BoneCategory> {
        classify(name).category
    }

    #[test]
    fn unambiguous_names() {
        assert_eq!(cat("Hips"), Some(BoneCategory::Hips));
        assert_eq!(cat("pelvis"), Some(BoneCategory::Hips));
        assert_eq!(classify("mixamorig:LeftHand").side, Side::Left);
        assert_eq!(cat("mixamorig:LeftHand"), Some(BoneCategory::Hand));
        assert_eq!(cat("mixamorig:RightForeArm"), Some(BoneCategory::LowerArm));
        assert_eq!(cat("J_Bip_L_UpperArm"), Some(BoneCategory::UpperArm));
        assert_eq!(cat("Bip01_L_Calf"), Some(BoneCategory::LowerLeg));
        assert_eq!(cat("upper_chest"), Some(BoneCategory::UpperChest));
    }

    #[test]
    fn mixamo_leg_naming() {
        // The bug the prop test could not catch: UpLeg is upper, bare Leg is lower.
        assert_eq!(cat("mixamorig:LeftUpLeg"), Some(BoneCategory::UpperLeg));
        assert_eq!(cat("mixamorig:LeftLeg"), Some(BoneCategory::LowerLeg));
    }

    #[test]
    fn fingers_are_not_hands() {
        assert_eq!(cat("mixamorig:LeftHandMiddle1"), Some(BoneCategory::Finger));
        assert_eq!(cat("mixamorig:RightHandThumb3"), Some(BoneCategory::Finger));
        // The hand itself is still a hand.
        assert_eq!(cat("mixamorig:LeftHand"), Some(BoneCategory::Hand));
    }

    #[test]
    fn leaf_end_bones_flagged() {
        assert!(classify("mixamorig:HeadTop_End").is_leaf_end);
        assert!(classify("mixamorig:LeftToe_End").is_leaf_end);
        assert!(!classify("mixamorig:Head").is_leaf_end);
    }

    #[test]
    fn unrelated_names_are_unmapped() {
        assert_eq!(cat("WeaponMount"), None);
        assert_eq!(cat("HairBone_01"), None);
    }
}
