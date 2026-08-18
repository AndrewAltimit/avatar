//! `avatar-anim-gen` — generate Unity `.anim` clips and FX-layer analog-gesture blend trees as
//! text (Unity YAML), for the M4 "asset generation" milestone (`PLAN.md` §4, §9).
//!
//! This crate is a **generator**: it emits Unity-YAML documents that Unity will import. The hard
//! part is correct serialization — the exact field names, the 2-space block indentation, the inline
//! `{x: 0, y: 0}` / `{fileID: N}` flow maps, and stable `fileID`s — so the bulk of the code is a
//! small typed model plus a faithful emitter ([`yaml_emit`]). Two layers are covered:
//!
//! - [`clip`] — [`AnimationClip`] → a `--- !u!74` `.anim` document. Two worked cases: a blendshape
//!   weight curve ([`FloatCurve::blendshape`]) and a GameObject active toggle
//!   ([`FloatCurve::game_object_active`]); both are `m_FloatCurves` entries differing only in
//!   `(path, attribute, classID)`.
//! - [`blendtree`] — [`BlendTree`] → a `--- !u!206` 1D blend tree (plus, optionally, the owning
//!   `AnimatorState`/`AnimatorStateMachine`) that blends `GestureLeftWeight`/`GestureRightWeight`
//!   across child clips so any gesture reaches any fraction — the analog-gesture headline feature.
//! - [`controller`] — [`AnimatorController`] → a `--- !u!91` FX `AnimatorController` wrapping a
//!   blend-tree fragment in one layer ([`fx_blend_tree`]), for callers who want a complete
//!   standalone `.controller` rather than a fragment to paste into an existing one.
//!
//! # fileID strategy
//!
//! Unity identifies every object in a file by a 64-bit local `fileID`. Generated ids must be (a)
//! **deterministic** — no randomness; the same input yields byte-identical output, which keeps
//! generated assets diffable and CI reproducible — and (b) **collision-free within a file**. We use
//! [`IdGen`], a small counter seeded by a FNV-1a hash of a caller-supplied name. The hash gives a
//! stable, name-derived base so two independently-generated assets don't accidentally share ids;
//! the counter then hands out sequential ids within the file. Ids are masked into the positive
//! `i64` range Unity uses for authored (non-instance) objects. This mirrors the decision in
//! `PLAN.md` risk 2: *prefer generating fresh assets over surgical edits*.

pub mod blendtree;
pub mod clip;
pub mod controller;
pub mod expressions;
pub mod gesture;
pub mod toggle;
pub mod yaml_emit;

pub use blendtree::{BlendTree, ChildMotion};
pub use clip::{AnimationClip, ClipSettings, FloatCurve, Keyframe};
pub use controller::{
    AnimatorController, AnimatorLayer, AnimatorParameter, ParamType, fx_blend_tree,
};
pub use expressions::{
    ExpressionParamSpec, ExpressionParams, ExpressionValueType, ExpressionsMenu, MenuControlSpec,
    ScriptRef, VRC_EXPRESSION_PARAMETERS_SCRIPT, VRC_EXPRESSIONS_MENU_SCRIPT, VRCSDK3A_DLL_GUID,
};
pub use gesture::{GESTURE_NAMES, GestureHand, GestureLayer, fx_gestures};
pub use toggle::{GeneratedFile, ToggleBundle, ToggleSpec, ToggleTarget, generate_toggle};
pub use yaml_emit::{Emitter, ObjectRef};

/// A deterministic allocator of Unity local `fileID`s for one generated file.
///
/// Seed it from a stable name (the asset/object name) so independently generated files land in
/// different id ranges, then call [`IdGen::alloc`] for each object. No randomness is involved — the
/// same seed always yields the same id sequence.
#[derive(Debug, Clone)]
pub struct IdGen {
    next: i64,
}

impl IdGen {
    /// Create an allocator whose first id is derived from `seed` (typically the asset name).
    pub fn new(seed: &str) -> Self {
        // FNV-1a over the seed, masked into a comfortable positive range and rounded so the first
        // ids look like Unity's (which tend to be large round-ish numbers). The exact value is
        // unimportant; determinism and intra-file uniqueness are.
        let h = avatar_unity_yaml::fnv1a(seed.as_bytes());
        // Keep within ~10^15 and away from 0; step by 1 thereafter.
        let base = (h % 900_000_000_000_000) + 100_000_000_000_000;
        IdGen { next: base as i64 }
    }

    /// Allocate the next fileID.
    pub fn alloc(&mut self) -> i64 {
        let id = self.next;
        self.next += 1;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_unity_yaml::UnityFile;

    #[test]
    fn idgen_is_deterministic() {
        let mut a = IdGen::new("Smile");
        let mut b = IdGen::new("Smile");
        assert_eq!(a.alloc(), b.alloc());
        assert_eq!(a.alloc(), b.alloc());
        // Different seeds give different bases.
        let mut c = IdGen::new("Frown");
        assert_ne!(IdGen::new("Smile").alloc(), c.alloc());
    }

    #[test]
    fn idgen_increments_and_stays_positive() {
        let mut g = IdGen::new("anything");
        let a = g.alloc();
        let b = g.alloc();
        assert_eq!(b, a + 1);
        assert!(a > 0);
    }

    // --- AnimationClip generation -------------------------------------------------------------

    fn smile_clip() -> AnimationClip {
        AnimationClip::new("Smile").float_curve(FloatCurve::blendshape(
            "Body",
            "Smile",
            vec![Keyframe::flat(0.0, 0.0), Keyframe::flat(1.0 / 60.0, 100.0)],
        ))
    }

    #[test]
    fn clip_emits_expected_headers_and_fields() {
        let mut ids = IdGen::new("Smile");
        let yaml = smile_clip().to_unity_yaml(ids.alloc());

        // Document header + type.
        assert!(yaml.contains("%YAML 1.1"));
        assert!(yaml.contains("--- !u!74 &"));
        assert!(yaml.contains("AnimationClip:"));
        // Name + curve collection.
        assert!(yaml.contains("m_Name: Smile"));
        assert!(yaml.contains("m_FloatCurves:"));
        // The blendshape binding fields.
        assert!(yaml.contains("attribute: blendShape.Smile"));
        assert!(yaml.contains("path: Body"));
        assert!(yaml.contains("classID: 137"));
        // A keyframe with the 100 weight, and Unity float formatting (no trailing .0).
        assert!(yaml.contains("value: 100"));
        assert!(yaml.contains("m_AnimationClipSettings:"));
        // Empty transform-curve collections are present (Unity expects them).
        assert!(yaml.contains("m_PositionCurves: []"));
    }

    #[test]
    fn toggle_clip_uses_gameobject_isactive() {
        let mut ids = IdGen::new("Toggle");
        let clip = AnimationClip::new("HatOn").float_curve(FloatCurve::game_object_active(
            "Armature/Head/Hat",
            vec![Keyframe::flat(0.0, 1.0)],
        ));
        let yaml = clip.to_unity_yaml(ids.alloc());
        assert!(yaml.contains("attribute: m_IsActive"));
        assert!(yaml.contains("path: Armature/Head/Hat"));
        assert!(yaml.contains("classID: 1"));
    }

    #[test]
    fn clip_roundtrips_through_unity_yaml_reader() {
        let mut ids = IdGen::new("Smile");
        let file_id = ids.alloc();
        let yaml = smile_clip().to_unity_yaml(file_id);

        let parsed = UnityFile::parse(&yaml).expect("our generated .anim must parse");
        assert_eq!(parsed.documents.len(), 1);
        let d = &parsed.documents[0];
        assert_eq!(d.class_id, 74);
        assert_eq!(d.file_id, file_id);
        assert_eq!(d.type_name, "AnimationClip");
        assert_eq!(d.name(), Some("Smile"));
        // The float curve survived the reader: confirm the nested binding fields.
        let curves = d.body["m_FloatCurves"].as_vec().expect("m_FloatCurves seq");
        assert_eq!(curves.len(), 1);
        assert_eq!(curves[0]["attribute"].as_str(), Some("blendShape.Smile"));
        assert_eq!(curves[0]["classID"].as_i64(), Some(137));
    }

    // --- BlendTree generation -----------------------------------------------------------------

    fn analog_tree() -> BlendTree {
        BlendTree::analog_gesture("Fist", "GestureLeftWeight")
            .clip("1234567890abcdef1234567890abcdef", 0.0)
            .clip("abcdef1234567890abcdef1234567890", 1.0)
    }

    #[test]
    fn blendtree_emits_expected_206_document() {
        let tree = analog_tree();
        let mut e = Emitter::new();
        tree.emit_tree(&mut e, 110600000);
        let yaml = format!("{}{}", yaml_emit::UNITY_PREAMBLE, e.into_string());

        assert!(yaml.contains("--- !u!206 &110600000"));
        assert!(yaml.contains("BlendTree:"));
        assert!(yaml.contains("m_Name: Fist"));
        assert!(yaml.contains("m_BlendParameter: GestureLeftWeight"));
        assert!(yaml.contains("m_BlendType: 0"));
        assert!(yaml.contains("m_Childs:"));
        // Both child clips, by guid + threshold.
        assert!(yaml.contains("guid: 1234567890abcdef1234567890abcdef"));
        assert!(yaml.contains("m_Threshold: 0"));
        assert!(yaml.contains("m_Threshold: 1"));
        assert_eq!(tree.thresholds(), vec![0.0, 1.0]);
    }

    #[test]
    fn blendtree_state_fragment_roundtrips() {
        let tree = analog_tree();
        let mut ids = IdGen::new("Fist");
        let (fragment, sm_id) = tree.to_state_fragment(&mut ids);
        let yaml = format!("{}{}", yaml_emit::UNITY_PREAMBLE, fragment);

        let parsed = UnityFile::parse(&yaml).expect("fragment must parse as Unity YAML");
        // State machine (1107), state (1102), blend tree (206).
        let classes: Vec<u32> = parsed.documents.iter().map(|d| d.class_id).collect();
        assert!(classes.contains(&1107));
        assert!(classes.contains(&1102));
        assert!(classes.contains(&206));

        // The state machine's default state points at the AnimatorState, which points at the tree.
        let sm = parsed
            .documents
            .iter()
            .find(|d| d.class_id == 1107)
            .unwrap();
        assert_eq!(sm.file_id, sm_id);
        let default_state = sm.body["m_DefaultState"]["fileID"].as_i64().unwrap();
        let state = parsed
            .documents
            .iter()
            .find(|d| d.class_id == 1102 && d.file_id == default_state)
            .expect("default state resolves");
        let motion = state.body["m_Motion"]["fileID"].as_i64().unwrap();
        let tree_doc = parsed
            .documents
            .iter()
            .find(|d| d.class_id == 206 && d.file_id == motion)
            .expect("state motion resolves to the blend tree");
        assert_eq!(tree_doc.name(), Some("Fist"));
        assert_eq!(
            tree_doc.body["m_BlendParameter"].as_str(),
            Some("GestureLeftWeight")
        );
    }

    #[test]
    fn wiring_note_mentions_motion_and_parameter() {
        let tree = analog_tree();
        let note = tree.wiring_note(110600000);
        assert!(note.contains("fileID: 110600000"));
        assert!(note.contains("GestureLeftWeight"));
    }
}
