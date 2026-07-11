//! Robustness of the typed readers against malformed / wrong-shaped controller and clip
//! documents, in the same spirit as the `avatar-fbx` / `avatar-unity-yaml` malformed suites: a
//! hostile or hand-mangled asset must degrade to defaults or `None`, never panic. The readers sit
//! directly on lint's ingest path, so every parse here is a file `avatar lint` might be handed.

use avatar_unity_asset::{AnimationClip, AnimatorController};
use avatar_unity_yaml::UnityFile;

fn parse(text: &str) -> UnityFile {
    UnityFile::parse(text).expect("test inputs are structurally valid Unity YAML")
}

#[test]
fn controller_absent_yields_none() {
    let file = parse("--- !u!74 &1\nAnimationClip:\n  m_Name: NotAController\n");
    assert!(AnimatorController::from_file(&file).is_none());
    let empty = parse("--- !u!1 &1\nGameObject:\n  m_Name: X\n");
    assert!(AnimatorController::from_file(&empty).is_none());
}

#[test]
fn controller_with_wrong_typed_fields_degrades_to_defaults() {
    // Parameters as a scalar, layers missing, a state whose fields are all wrong types, a
    // transition whose conditions are a scalar, and a blend tree with scalar children.
    let text = "\
--- !u!91 &9100000
AnimatorController:
  m_Name: 7
  m_AnimatorParameters: not-a-list
--- !u!1102 &1
AnimatorState:
  m_WriteDefaultValues: banana
  m_Motion: also-not-a-ref
--- !u!1101 &2
AnimatorStateTransition:
  m_Conditions: nope
--- !u!206 &3
BlendTree:
  m_Childs: 12
  m_BlendType: many
--- !u!1107 &4
AnimatorStateMachine:
  m_ChildStates: {}
  m_DefaultState: []
";
    let c = AnimatorController::from_file(&parse(text)).expect("class-91 doc present");
    assert!(c.parameters.is_empty(), "scalar parameter list -> empty");
    assert_eq!(c.state_count, 1);
    // Unparseable write-defaults falls back to Unity's default (true).
    assert_eq!(c.write_defaults, vec![true]);
    assert_eq!(c.states.len(), 1);
    assert!(!c.states[0].motion.is_set(), "non-ref motion reads as null");
    assert!(c.conditions.is_empty());
    assert_eq!(c.blend_trees.len(), 1);
    assert!(c.blend_trees[0].direct_parameters.is_empty());
    assert!(c.blend_tree_motion_guids.is_empty());
    assert_eq!(c.state_machines.len(), 1);
    assert_eq!(c.state_machines[0].child_state_count, 0);
    assert!(!c.state_machines[0].has_default_state);
}

#[test]
fn clip_absent_or_mangled_never_panics() {
    let not_a_clip = parse("--- !u!91 &1\nAnimatorController:\n  m_Name: FX\n");
    assert!(AnimationClip::from_file(&not_a_clip).is_none());

    // Curve collections with wrong types; entries missing every expected field.
    let mangled = "\
--- !u!74 &7400000
AnimationClip:
  m_Name: 3.5
  m_FloatCurves:
  - 42
  - curve: x
  m_PositionCurves: scalar
  m_RotationCurves: {}
  m_PPtrCurves:
  - {}
";
    let clip = AnimationClip::from_file(&parse(mangled)).expect("class-74 doc present");
    // Entries survive with default (empty/zero) bindings rather than panicking.
    assert_eq!(clip.float_curves.len(), 2);
    assert_eq!(clip.float_curves[0].attribute, "");
    assert_eq!(clip.float_curves[0].class_id, 0);
    assert!(!clip.float_curves[0].is_muscle());
    // Scalar/mapping collections count as zero entries.
    assert_eq!(clip.transform_curves, 0);
    assert_eq!(clip.pptr_curves, 1);
    assert!(!clip.is_empty());
    assert!(!clip.animates_transforms());
}

#[test]
fn duplicate_controller_documents_attribute_to_the_first() {
    // A file that (illegally) holds two class-91 docs: the reader documents that owned objects
    // are attributed to the file as a whole, keyed off the first controller.
    let text = "\
--- !u!91 &1
AnimatorController:
  m_Name: First
--- !u!91 &2
AnimatorController:
  m_Name: Second
--- !u!1102 &3
AnimatorState:
  m_Name: S
  m_WriteDefaultValues: 0
  m_Motion: {fileID: 0}
";
    let c = AnimatorController::from_file(&parse(text)).expect("parses");
    assert_eq!(c.name.as_deref(), Some("First"));
    assert_eq!(c.state_count, 1);
}
