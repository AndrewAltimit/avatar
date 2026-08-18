//! Gesture-driven FX layers: the SDK3 counterpart of SDK2's per-gesture override slots.
//!
//! VRChat SDK3 exposes the hand gestures as two `Int` animator parameters, `GestureLeft` and
//! `GestureRight` (0 Neutral, 1 Fist, 2 HandOpen, 3 FingerPoint, 4 Victory, 5 RockNRoll,
//! 6 HandGun, 7 ThumbsUp — the same order as SDK2's override slots). The idiomatic FX setup —
//! and what this module emits — is **one layer per hand**: an `AnimatorStateMachine` (1107) with a
//! `Neutral` default state plus one state per gesture that has a clip, and an *Any State*
//! transition (1101) into each state conditioned on `GestureX Equals n`, with "can transition to
//! self" off so a held gesture doesn't retrigger. Gesture values that have no clip fall back to
//! `Neutral` through their own Any-State transition, so the layer is authoritative for all eight.
//!
//! A layer can also read **several** gesture parameters at once ([`GestureLayer::parameters`]):
//! each gesture state then gets one Any-State transition per parameter and `Neutral` requires
//! *all* of them to be 0. That single "either hand" layer is exactly SDK2's semantics (an
//! override slot fired for whichever hand made the gesture) and sidesteps the classic two-layer
//! problem where the upper hand's Neutral clip, resetting shared blendshapes, clobbers the lower
//! hand's expression under Write Defaults off. When both hands hold different gestures, the
//! parameter listed first wins (its transitions come first in `m_AnyStateTransitions`).
//!
//! Clips here are expected to be *face-only* (blendshapes) — SDK3 hand poses come from the
//! Gesture playable layer, so an SDK2 gesture clip that also carried finger muscles must be split
//! (see `avatar-migrate`, which lifts just the blendshape curves). The `Neutral` clip should write
//! every blendshape any gesture in the layer touches back to 0, so the layer works under VRChat's
//! recommended Write Defaults **off** (which is what the emitted states use).

use crate::IdGen;
use crate::controller::{AnimatorController, AnimatorParameter};
use crate::yaml_emit::{Emitter, ObjectRef, UNITY_PREAMBLE};

/// `m_ConditionMode` on an `AnimatorCondition`: `Equals`.
pub const CONDITION_EQUALS: i64 = 6;

/// VRChat's gesture names, indexed by the `GestureLeft`/`GestureRight` value.
pub const GESTURE_NAMES: [&str; 8] = [
    "Neutral",
    "Fist",
    "HandOpen",
    "FingerPoint",
    "Victory",
    "RockNRoll",
    "HandGun",
    "ThumbsUp",
];

/// A gesture layer: the `Int` gesture parameter(s) it reads and the motion for each gesture value.
#[derive(Debug, Clone)]
pub struct GestureLayer {
    /// Layer name (`Left Hand`, `Right Hand`, `Gestures`).
    pub layer_name: String,
    /// The gesture parameter(s) (`GestureLeft` / `GestureRight`); one for a per-hand layer, both
    /// for an either-hand layer (first listed wins a conflict).
    pub parameters: Vec<String>,
    /// The motion played in `Neutral` (also the fallback for gestures with no clip). Should reset
    /// every blendshape the other clips touch.
    pub neutral: ObjectRef,
    /// Motion per gesture value 1..=7 (`motions[0]` = Fist). `None` falls back to `Neutral`.
    pub motions: [Option<ObjectRef>; 7],
    /// Cross-fade duration in seconds for every transition (0.1 is the common choice for faces).
    pub transition_duration: f32,
}

/// Backwards-compatible name for a single-parameter [`GestureLayer`].
pub type GestureHand = GestureLayer;

impl GestureLayer {
    /// A per-hand layer reading one parameter, with no gesture clips yet.
    pub fn new(
        layer_name: impl Into<String>,
        parameter: impl Into<String>,
        neutral: ObjectRef,
    ) -> Self {
        GestureLayer {
            layer_name: layer_name.into(),
            parameters: vec![parameter.into()],
            neutral,
            motions: Default::default(),
            transition_duration: 0.1,
        }
    }

    /// An either-hand layer reading `GestureLeft` *and* `GestureRight` (SDK2 semantics).
    pub fn either_hand(layer_name: impl Into<String>, neutral: ObjectRef) -> Self {
        GestureLayer {
            layer_name: layer_name.into(),
            parameters: vec!["GestureLeft".into(), "GestureRight".into()],
            neutral,
            motions: Default::default(),
            transition_duration: 0.1,
        }
    }

    /// Set the motion for gesture value `gesture` (1..=7). Out-of-range values are ignored.
    pub fn motion(mut self, gesture: usize, motion: ObjectRef) -> Self {
        if (1..=7).contains(&gesture) {
            self.motions[gesture - 1] = Some(motion);
        }
        self
    }

    /// Emit this hand's state-machine fragment (SM + states + Any-State transitions). Returns the
    /// fragment text and the state-machine fileID for the layer's `m_StateMachine`.
    pub fn to_state_fragment(&self, ids: &mut IdGen) -> (String, i64) {
        let sm_id = ids.alloc();
        // States: Neutral (index 0) then one per gesture with a motion.
        let neutral_state = ids.alloc();
        let gesture_states: Vec<(usize, i64, &ObjectRef)> = self
            .motions
            .iter()
            .enumerate()
            .filter_map(|(i, m)| m.as_ref().map(|m| (i + 1, ids.alloc(), m)))
            .collect();
        // Any-State transitions. Neutral: one transition requiring every parameter == 0. Each
        // gesture value 1..=7: one transition per parameter (Equals g), targeting its state or
        // Neutral (unclipped gestures still leave the previous expression). Parameter order in
        // the list is priority order.
        let mut transitions: Vec<Transition> = Vec::new();
        transitions.push(Transition {
            id: ids.alloc(),
            conditions: self.parameters.iter().map(|p| (p.clone(), 0)).collect(),
            dst: neutral_state,
        });
        for g in 1..8usize {
            let dst = gesture_states
                .iter()
                .find(|(gi, _, _)| *gi == g)
                .map(|(_, id, _)| *id)
                .unwrap_or(neutral_state);
            for p in &self.parameters {
                transitions.push(Transition {
                    id: ids.alloc(),
                    conditions: vec![(p.clone(), g as i64)],
                    dst,
                });
            }
        }

        let mut e = Emitter::new();

        // --- AnimatorStateMachine (1107)
        e.doc_header(1107, sm_id);
        e.line("AnimatorStateMachine:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "1");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.layer_name);
            e.key("m_ChildStates");
            e.indented(|e| {
                e.line("- serializedVersion: 1");
                e.indented(|e| {
                    e.kv_ref("m_State", &ObjectRef::local(neutral_state));
                    e.kv("m_Position", "{x: 300, y: 0, z: 0}");
                });
                for (row, (_, id, _)) in gesture_states.iter().enumerate() {
                    e.line("- serializedVersion: 1");
                    e.indented(|e| {
                        e.kv_ref("m_State", &ObjectRef::local(*id));
                        e.kv(
                            "m_Position",
                            &format!("{{x: 300, y: {}, z: 0}}", 60 * (row as i64 + 1)),
                        );
                    });
                }
            });
            e.kv("m_ChildStateMachines", "[]");
            e.key("m_AnyStateTransitions");
            e.indented(|e| {
                for t in &transitions {
                    e.line(&format!("- {}", ObjectRef::local(t.id).render()));
                }
            });
            e.kv("m_EntryTransitions", "[]");
            e.kv("m_StateMachineTransitions", "{}");
            e.kv("m_StateMachineBehaviours", "[]");
            e.kv("m_AnyStatePosition", "{x: 50, y: 20, z: 0}");
            e.kv("m_EntryPosition", "{x: 50, y: 120, z: 0}");
            e.kv("m_ExitPosition", "{x: 800, y: 120, z: 0}");
            e.kv("m_ParentStateMachinePosition", "{x: 800, y: 20, z: 0}");
            e.kv_ref("m_DefaultState", &ObjectRef::local(neutral_state));
        });

        // --- AnimatorStates (1102)
        emit_state(&mut e, neutral_state, GESTURE_NAMES[0], &self.neutral);
        for (g, id, motion) in &gesture_states {
            emit_state(&mut e, *id, GESTURE_NAMES[*g], motion);
        }

        // --- Any-State AnimatorStateTransitions (1101)
        for t in &transitions {
            emit_any_state_transition(&mut e, t, self.transition_duration);
        }

        (e.into_string(), sm_id)
    }
}

fn emit_state(e: &mut Emitter, state_id: i64, name: &str, motion: &ObjectRef) {
    e.doc_header(1102, state_id);
    e.line("AnimatorState:");
    e.indented(|e| {
        e.kv("m_ObjectHideFlags", "1");
        e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
        e.kv("m_PrefabInstance", "{fileID: 0}");
        e.kv("m_PrefabAsset", "{fileID: 0}");
        e.kv("m_Name", name);
        e.kv("m_Speed", "1");
        e.kv("m_CycleOffset", "0");
        e.kv("m_Transitions", "[]");
        e.kv("m_StateMachineBehaviours", "[]");
        e.kv("m_Position", "{x: 50, y: 50, z: 0}");
        e.kv("m_IKOnFeet", "0");
        // Write Defaults OFF; the Neutral clip resets what the gesture clips touch.
        e.kv("m_WriteDefaultValues", "0");
        e.kv("m_Mirror", "0");
        e.kv("m_SpeedParameterActive", "0");
        e.kv("m_MirrorParameterActive", "0");
        e.kv("m_CycleOffsetParameterActive", "0");
        e.kv("m_TimeParameterActive", "0");
        e.kv_ref("m_Motion", motion);
        e.kv("m_Tag", "");
        e.kv("m_SpeedParameter", "");
        e.kv("m_MirrorParameter", "");
        e.kv("m_CycleOffsetParameter", "");
        e.kv("m_TimeParameter", "");
    });
}

/// An Any-State transition: all `conditions` (`parameter Equals value`) must hold.
struct Transition {
    id: i64,
    conditions: Vec<(String, i64)>,
    dst: i64,
}

fn emit_any_state_transition(e: &mut Emitter, t: &Transition, duration: f32) {
    e.doc_header(1101, t.id);
    e.line("AnimatorStateTransition:");
    e.indented(|e| {
        e.kv("m_ObjectHideFlags", "1");
        e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
        e.kv("m_PrefabInstance", "{fileID: 0}");
        e.kv("m_PrefabAsset", "{fileID: 0}");
        e.kv("m_Name", "");
        e.key("m_Conditions");
        e.indented(|e| {
            for (parameter, value) in &t.conditions {
                e.line(&format!("- m_ConditionMode: {CONDITION_EQUALS}"));
                e.indented(|e| {
                    e.kv("m_ConditionEvent", parameter);
                    // Unity's field name really is misspelled ("Treshold").
                    e.kv_i64("m_EventTreshold", *value);
                });
            }
        });
        e.kv("m_DstStateMachine", "{fileID: 0}");
        e.kv_ref("m_DstState", &ObjectRef::local(t.dst));
        e.kv("m_Solo", "0");
        e.kv("m_Mute", "0");
        e.kv("m_IsExit", "0");
        e.kv("serializedVersion", "3");
        e.kv_f32("m_TransitionDuration", duration);
        e.kv_i64("m_TransitionOffset", 0);
        e.kv_f32("m_ExitTime", 0.75);
        e.kv("m_HasExitTime", "0");
        e.kv("m_HasFixedDuration", "1");
        e.kv_i64("m_InterruptionSource", 0);
        e.kv("m_OrderedInterruption", "1");
        // A held gesture must not keep re-entering its own state (which would restart the fade).
        e.kv("m_CanTransitionToSelf", "0");
    });
}

/// Assemble a complete FX `.controller` stream from gesture layers (plus any extra pre-built
/// layers as `(layer_name, fragment_text, state_machine_id)`, appended after them). Declares each
/// gesture `Int` parameter once. Returns the full text.
pub fn fx_gestures(
    name: &str,
    layers: &[GestureLayer],
    extra_layers: &[(String, String, i64)],
    ids: &mut IdGen,
) -> String {
    let controller_id = ids.alloc();
    let mut controller = AnimatorController::new(name);
    let mut fragments = String::new();
    let mut declared: Vec<String> = Vec::new();
    for layer in layers {
        for p in &layer.parameters {
            if !declared.contains(p) {
                controller = controller.parameter(AnimatorParameter::int(p));
                declared.push(p.clone());
            }
        }
        let (fragment, sm_id) = layer.to_state_fragment(ids);
        controller = controller.layer(&layer.layer_name, sm_id);
        fragments.push_str(&fragment);
    }
    for (layer_name, fragment, sm_id) in extra_layers {
        controller = controller.layer(layer_name, *sm_id);
        fragments.push_str(fragment);
    }
    let mut e = Emitter::new();
    controller.emit_controller(&mut e, controller_id);
    format!("{UNITY_PREAMBLE}{}{fragments}", e.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_unity_asset::AnimatorController as ReaderController;
    use avatar_unity_yaml::UnityFile;

    fn hand(param: &str) -> GestureLayer {
        GestureLayer::new(
            format!("{param} Hand"),
            param,
            ObjectRef::external(7400000, "a000000000000000000000000000000a", 2),
        )
        .motion(
            1,
            ObjectRef::external(7400000, "a000000000000000000000000000000b", 2),
        )
        .motion(
            4,
            ObjectRef::external(7400000, "a000000000000000000000000000000c", 2),
        )
    }

    #[test]
    fn fragment_has_neutral_plus_one_state_per_clip_and_eight_any_state_transitions() {
        let mut ids = IdGen::new("gest");
        let (frag, sm_id) = hand("GestureLeft").to_state_fragment(&mut ids);
        let text = format!("{UNITY_PREAMBLE}{frag}");
        let file = UnityFile::parse(&text).unwrap();
        let sm = file.documents.iter().find(|d| d.class_id == 1107).unwrap();
        assert_eq!(sm.file_id, sm_id);
        assert_eq!(sm.body["m_ChildStates"].as_vec().unwrap().len(), 3);
        assert_eq!(sm.body["m_AnyStateTransitions"].as_vec().unwrap().len(), 8);
        let states: Vec<&str> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1102)
            .map(|d| d.name().unwrap())
            .collect();
        assert_eq!(states, vec!["Neutral", "Fist", "Victory"]);
        // Every transition: Equals on GestureLeft, no self-transition, fixed 0.1s.
        let transitions: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1101)
            .collect();
        assert_eq!(transitions.len(), 8);
        let mut thresholds: Vec<i64> = Vec::new();
        for t in &transitions {
            let c = &t.body["m_Conditions"][0];
            assert_eq!(c["m_ConditionMode"].as_i64(), Some(CONDITION_EQUALS));
            assert_eq!(c["m_ConditionEvent"].as_str(), Some("GestureLeft"));
            thresholds.push(c["m_EventTreshold"].as_i64().unwrap());
            assert_eq!(t.body["m_CanTransitionToSelf"].as_i64(), Some(0));
            assert_eq!(t.body["m_HasExitTime"].as_i64(), Some(0));
        }
        thresholds.sort();
        assert_eq!(thresholds, (0..8).collect::<Vec<_>>());
        // Gestures without a clip (e.g. 2 = HandOpen) route to the Neutral state.
        let neutral_id = file
            .documents
            .iter()
            .find(|d| d.class_id == 1102 && d.name() == Some("Neutral"))
            .unwrap()
            .file_id;
        let t2 = transitions
            .iter()
            .find(|t| t.body["m_Conditions"][0]["m_EventTreshold"].as_i64() == Some(2))
            .unwrap();
        assert_eq!(t2.body["m_DstState"]["fileID"].as_i64(), Some(neutral_id));
    }

    #[test]
    fn either_hand_layer_has_a_transition_per_parameter_and_an_all_zero_neutral() {
        let mut ids = IdGen::new("either");
        let layer = GestureLayer::either_hand(
            "Gestures",
            ObjectRef::external(7400000, "a000000000000000000000000000000a", 2),
        )
        .motion(
            1,
            ObjectRef::external(7400000, "a000000000000000000000000000000b", 2),
        );
        let (frag, _) = layer.to_state_fragment(&mut ids);
        let file = UnityFile::parse(&format!("{UNITY_PREAMBLE}{frag}")).unwrap();
        let transitions: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1101)
            .collect();
        // 1 neutral + 7 gestures × 2 parameters.
        assert_eq!(transitions.len(), 15);
        let neutral = transitions
            .iter()
            .find(|t| t.body["m_Conditions"].as_vec().unwrap().len() == 2)
            .expect("neutral transition has both conditions");
        let events: Vec<&str> = neutral.body["m_Conditions"]
            .as_vec()
            .unwrap()
            .iter()
            .map(|c| c["m_ConditionEvent"].as_str().unwrap())
            .collect();
        assert_eq!(events, vec!["GestureLeft", "GestureRight"]);
        assert!(
            neutral.body["m_Conditions"]
                .as_vec()
                .unwrap()
                .iter()
                .all(|c| c["m_EventTreshold"].as_i64() == Some(0))
        );
        // GestureLeft transitions precede GestureRight ones for the same value (priority).
        let sm = file.documents.iter().find(|d| d.class_id == 1107).unwrap();
        let order: Vec<i64> = sm.body["m_AnyStateTransitions"]
            .as_vec()
            .unwrap()
            .iter()
            .map(|r| r["fileID"].as_i64().unwrap())
            .collect();
        let by_id = |id: i64| transitions.iter().find(|t| t.file_id == id).unwrap();
        let fist: Vec<&str> = order
            .iter()
            .map(|id| by_id(*id))
            .filter(|t| t.body["m_Conditions"][0]["m_EventTreshold"].as_i64() == Some(1))
            .map(|t| {
                t.body["m_Conditions"][0]["m_ConditionEvent"]
                    .as_str()
                    .unwrap()
            })
            .collect();
        assert_eq!(fist, vec!["GestureLeft", "GestureRight"]);
    }

    #[test]
    fn fx_gestures_declares_int_params_and_two_layers_and_reads_back() {
        let mut ids = IdGen::new("FX");
        let yaml = fx_gestures(
            "FX",
            &[hand("GestureLeft"), hand("GestureRight")],
            &[],
            &mut ids,
        );
        let file = UnityFile::parse(&yaml).unwrap();
        let ctrl = ReaderController::from_file(&file).unwrap();
        assert_eq!(ctrl.state_machines.len(), 2);
        assert!(ctrl.write_defaults.iter().all(|w| !w));
        let params: Vec<(String, i64)> = ctrl
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.raw_type))
            .collect();
        assert!(params.contains(&("GestureLeft".into(), 3)));
        assert!(params.contains(&("GestureRight".into(), 3)));
        // Deterministic.
        let again = fx_gestures(
            "FX",
            &[hand("GestureLeft"), hand("GestureRight")],
            &[],
            &mut IdGen::new("FX"),
        );
        assert_eq!(yaml, again);
    }
}
