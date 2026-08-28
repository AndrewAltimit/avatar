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
//! each gesture then gets Any-State transitions per parameter and `Neutral` requires *all* of
//! them to be 0. That single "either hand" layer is exactly SDK2's semantics (an override slot
//! fired for whichever hand made the gesture) and sidesteps the classic two-layer problem where
//! the upper hand's Neutral clip, resetting shared blendshapes, clobbers the lower hand's
//! expression under Write Defaults off. Transition conditions are built **mutually exclusive**
//! (at most one target valid at any parameter state) — two simultaneously-valid Any-State
//! transitions to different states ping-pong every crossfade, i.e. a visible oscillation.
//! **Later parameters win** (list `GestureLeft` then `GestureRight`: the right hand takes the
//! face when both act, VRChat's own hands-layer convention); in analog mode "act" is
//! weight-gated ([`WEIGHT_ON`]/[`WEIGHT_OFF`]) so a wand thumb resting on the touchpad (Fist at
//! weight 0) can never mask the other hand's real expression. When **both hands hold the same
//! gesture** an analog either-hand layer routes to a dedicated `<Gesture> LR` state playing a 2D
//! freeform-cartesian tree over both weights whose samples encode the **capped sum**
//! `min(left + right, 1)` — squeezing the second trigger deepens the expression instead of
//! restarting it (per-hand transitions gain `NotEqual` guards so exactly one target stays valid).
//!
//! Clips here are expected to be *face-only* (blendshapes) — SDK3 hand poses come from the
//! Gesture playable layer, so an SDK2 gesture clip that also carried finger muscles must be split
//! (see `avatar-migrate`, which lifts just the blendshape curves). The `Neutral` clip should write
//! every blendshape any gesture in the layer touches back to 0, so the layer works under VRChat's
//! recommended Write Defaults **off** (which is what the emitted states use).

use crate::IdGen;
use crate::blendtree::{BlendTree, ChildMotion};
use crate::controller::{AnimatorController, AnimatorParameter};
use crate::yaml_emit::{Emitter, ObjectRef, UNITY_PREAMBLE};

/// `m_ConditionMode` on an `AnimatorCondition`: `Equals`.
pub const CONDITION_EQUALS: i64 = 6;
/// `m_ConditionMode`: `Greater` (float parameters).
pub const CONDITION_GREATER: i64 = 3;
/// `m_ConditionMode`: `Less` (float parameters).
pub const CONDITION_LESS: i64 = 4;
/// `m_ConditionMode`: `NotEqual` (int parameters).
pub const CONDITION_NOT_EQUAL: i64 = 7;

/// Analog mode: a higher-priority hand must be squeezing at least this hard to claim the face…
pub const WEIGHT_ON: f32 = 0.05;
/// …and below this it releases its claim (the gap is hysteresis, so a trigger hovering at one
/// threshold can't chatter between hands).
pub const WEIGHT_OFF: f32 = 0.02;

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
    /// Optional **half-strength** motion per gesture (every curve at 50 %), analog mode only:
    /// midpoint samples for the both-hands capped-sum tree, so `0.5 + 0.1` lands near `0.6`
    /// instead of snapping between corners.
    pub motions_half: [Option<ObjectRef>; 7],
    /// Cross-fade duration in seconds for every transition (0.1 is the common choice for faces).
    pub transition_duration: f32,
    /// **Analog gestures** (SDK2 Vive-wand semantics): each gesture state's motion becomes a 1D
    /// [`BlendTree`](crate::BlendTree) on the gesture parameter's weight float (`GestureLeft` →
    /// `GestureLeftWeight`), blending `Neutral` (threshold 0) → the gesture clip (threshold 1),
    /// so trigger depth *is* expression depth. With several parameters each gesture gets one
    /// state **per parameter** (`Fist L` / `Fist R`) so each hand blends on its own weight —
    /// still one layer, so the either-hand conflict rule (first parameter wins) is unchanged.
    /// On controllers whose weight only tracks an analog axis for some gestures (Index: Fist),
    /// other gestures need the trigger held to show — the same trade SDK2 made on wands.
    pub analog: bool,
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
            motions_half: Default::default(),
            transition_duration: 0.1,
            analog: false,
        }
    }

    /// An either-hand layer reading `GestureLeft` *and* `GestureRight` (SDK2 semantics).
    pub fn either_hand(layer_name: impl Into<String>, neutral: ObjectRef) -> Self {
        GestureLayer {
            layer_name: layer_name.into(),
            parameters: vec!["GestureLeft".into(), "GestureRight".into()],
            neutral,
            motions: Default::default(),
            motions_half: Default::default(),
            transition_duration: 0.1,
            analog: false,
        }
    }

    /// Analog mode (builder-style): blend each gesture state on its parameter's weight float.
    pub fn analog(mut self) -> Self {
        self.analog = true;
        self
    }

    /// Set the motion for gesture value `gesture` (1..=7). Out-of-range values are ignored.
    pub fn motion(mut self, gesture: usize, motion: ObjectRef) -> Self {
        if (1..=7).contains(&gesture) {
            self.motions[gesture - 1] = Some(motion);
        }
        self
    }

    /// Set the half-strength motion for gesture value `gesture` (1..=7) — the both-hands
    /// capped-sum tree's midpoint sample.
    pub fn motion_half(mut self, gesture: usize, motion: ObjectRef) -> Self {
        if (1..=7).contains(&gesture) {
            self.motions_half[gesture - 1] = Some(motion);
        }
        self
    }

    /// The hand suffix a parameter contributes to an analog state name (`GestureLeft` → `L`).
    fn hand_suffix(parameter: &str) -> &str {
        if parameter.contains("Left") {
            "L"
        } else if parameter.contains("Right") {
            "R"
        } else {
            parameter
        }
    }

    /// Emit this hand's state-machine fragment (SM + states + Any-State transitions; in analog
    /// mode also the per-state `BlendTree`s). Returns the fragment text and the state-machine
    /// fileID for the layer's `m_StateMachine`.
    pub fn to_state_fragment(&self, ids: &mut IdGen) -> (String, i64) {
        let sm_id = ids.alloc();
        // States: Neutral (index 0) then one per gesture with a motion — in analog mode one per
        // (gesture, parameter) pair, each with its own BlendTree id, so each hand's state blends
        // on that hand's weight.
        let neutral_state = ids.alloc();
        // The both-hands capped-sum state exists in analog two-parameter (either-hand) layers.
        let both_hands = self.analog && self.parameters.len() == 2;
        let mut gesture_states: Vec<GState> = Vec::new();
        for (i, m) in self.motions.iter().enumerate() {
            let Some(m) = m else { continue };
            if self.analog {
                for (param_idx, p) in self.parameters.iter().enumerate() {
                    let name = if self.parameters.len() > 1 {
                        format!("{} {}", GESTURE_NAMES[i + 1], Self::hand_suffix(p))
                    } else {
                        GESTURE_NAMES[i + 1].to_string()
                    };
                    gesture_states.push(GState {
                        gesture: i + 1,
                        kind: StateKind::Hand(param_idx),
                        state_id: ids.alloc(),
                        tree_id: Some(ids.alloc()),
                        name,
                        motion: m.clone(),
                    });
                }
                if both_hands {
                    gesture_states.push(GState {
                        gesture: i + 1,
                        kind: StateKind::Both,
                        state_id: ids.alloc(),
                        tree_id: Some(ids.alloc()),
                        name: format!("{} LR", GESTURE_NAMES[i + 1]),
                        motion: m.clone(),
                    });
                }
            } else {
                gesture_states.push(GState {
                    gesture: i + 1,
                    kind: StateKind::Hand(0),
                    state_id: ids.alloc(),
                    tree_id: None,
                    name: GESTURE_NAMES[i + 1].to_string(),
                    motion: m.clone(),
                });
            }
        }
        // Any-State transitions, built so that at most one target is ever valid at a time —
        // two simultaneously-valid transitions to different states would ping-pong every
        // `transition_duration` (the crossfade re-arms `canTransitionToSelf`-exempt targets),
        // which is exactly the wild oscillation two active hands used to cause. Rules:
        //
        // - Neutral: every parameter == 0.
        // - **Later parameters win** (list `GestureLeft` then `GestureRight` and the right hand
        //   takes the face when both act — VRChat's own hands-layer convention): a parameter's
        //   transitions require every *later* parameter to be inactive.
        // - Analog mode adds weight gating: "active" means *squeezing* ([`WEIGHT_ON`]), not just
        //   gesturing — on Vive wands a thumb resting on the touchpad centre reports Fist at
        //   weight 0, and without the gate that phantom would mask (and oscillate against) the
        //   other hand's real expression. Inactive = `param == 0` OR `weight <` [`WEIGHT_OFF`]
        //   (one transition per alternative), so a higher-priority hand that stops squeezing
        //   hands the face back.
        let mut transitions: Vec<Transition> = Vec::new();
        transitions.push(Transition {
            id: ids.alloc(),
            conditions: self
                .parameters
                .iter()
                .map(|p| Cond::Eq(p.clone(), 0))
                .collect(),
            dst: neutral_state,
        });
        for g in 1..8usize {
            let find = |kind: StateKind| {
                gesture_states
                    .iter()
                    .find(|st| st.gesture == g && (!self.analog || st.kind == kind))
                    .map(|st| st.state_id)
            };
            // Both hands on the same gesture: the capped-sum state, checked before the per-hand
            // ones (its condition set is exclusive with all of theirs — see below).
            if both_hands && let Some(dst) = find(StateKind::Both) {
                transitions.push(Transition {
                    id: ids.alloc(),
                    conditions: self
                        .parameters
                        .iter()
                        .map(|p| Cond::Eq(p.clone(), g as i64))
                        .collect(),
                    dst,
                });
            }
            for (param_idx, p) in self.parameters.iter().enumerate() {
                let dst = find(StateKind::Hand(param_idx)).unwrap_or(neutral_state);
                // The transition's own trigger, plus (analog, non-lowest-priority) the gate that
                // this hand is actually squeezing.
                let mut base = vec![Cond::Eq(p.clone(), g as i64)];
                if self.analog && param_idx > 0 {
                    base.push(Cond::Greater(format!("{p}Weight"), WEIGHT_ON));
                    // Exclusive with the Both state: this hand wins only while the other hand is
                    // NOT holding the same gesture.
                    if both_hands {
                        base.push(Cond::Ne(self.parameters[0].clone(), g as i64));
                    }
                }
                // One transition per way every higher-priority parameter can be inactive.
                let higher: Vec<&String> = self.parameters[param_idx + 1..].iter().collect();
                let mut alt_sets: Vec<Vec<Cond>> = vec![base];
                for h in higher {
                    let mut next: Vec<Vec<Cond>> = Vec::new();
                    for set in &alt_sets {
                        let mut a = set.clone();
                        a.push(Cond::Eq((*h).clone(), 0));
                        next.push(a);
                        if self.analog {
                            let mut b = set.clone();
                            b.push(Cond::Less(format!("{h}Weight"), WEIGHT_OFF));
                            // Exclusive with the Both state (the `== 0` alternative already is).
                            if both_hands {
                                b.push(Cond::Ne((*h).clone(), g as i64));
                            }
                            next.push(b);
                        }
                    }
                    alt_sets = next;
                }
                for conditions in alt_sets {
                    transitions.push(Transition {
                        id: ids.alloc(),
                        conditions,
                        dst,
                    });
                }
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
                for (row, st) in gesture_states.iter().enumerate() {
                    e.line("- serializedVersion: 1");
                    e.indented(|e| {
                        e.kv_ref("m_State", &ObjectRef::local(st.state_id));
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
        for st in &gesture_states {
            let motion = match st.tree_id {
                Some(tree_id) => ObjectRef::local(tree_id),
                None => st.motion.clone(),
            };
            emit_state(&mut e, st.state_id, &st.name, &motion);
        }

        // --- Any-State AnimatorStateTransitions (1101)
        for t in &transitions {
            emit_any_state_transition(&mut e, t, self.transition_duration);
        }

        // --- BlendTrees (206), analog mode. Per-hand states: 1D, Neutral at weight 0 → the
        // gesture clip at 1. Both-hands states: 2D freeform cartesian over the two weights,
        // sampled to approximate the **capped sum** `min(left + right, 1)` — Neutral at the
        // origin, the full clip on and past the `x + y = 1` diagonal, and (when a half-strength
        // clip is provided) 50 % midpoints so e.g. `0.5 + 0.1` lands near `0.6`.
        for st in &gesture_states {
            let Some(tree_id) = st.tree_id else { continue };
            match st.kind {
                StateKind::Hand(param_idx) => {
                    BlendTree::analog_gesture(
                        &st.name,
                        format!("{}Weight", self.parameters[param_idx]),
                    )
                    .child(ChildMotion::motion(self.neutral.clone(), 0.0))
                    .child(ChildMotion::motion(st.motion.clone(), 1.0))
                    .emit_tree(&mut e, tree_id);
                }
                StateKind::Both => {
                    let mut tree = BlendTree::freeform_2d(
                        &st.name,
                        format!("{}Weight", self.parameters[0]),
                        format!("{}Weight", self.parameters[1]),
                    )
                    .child(ChildMotion::at(self.neutral.clone(), 0.0, 0.0));
                    if let Some(half) = &self.motions_half[st.gesture - 1] {
                        tree = tree
                            .child(ChildMotion::at(half.clone(), 0.5, 0.0))
                            .child(ChildMotion::at(half.clone(), 0.0, 0.5));
                    }
                    tree.child(ChildMotion::at(st.motion.clone(), 1.0, 0.0))
                        .child(ChildMotion::at(st.motion.clone(), 0.0, 1.0))
                        .child(ChildMotion::at(st.motion.clone(), 0.5, 0.5))
                        .child(ChildMotion::at(st.motion.clone(), 1.0, 1.0))
                        .emit_tree(&mut e, tree_id);
                }
            }
        }

        (e.into_string(), sm_id)
    }
}

/// One gesture state of a layer: which gesture value and (in analog mode) which parameter it
/// serves, its ids, display name, and the clip it plays (directly, or via its blend tree).
/// Which hand(s) a gesture state serves.
#[derive(Clone, Copy, PartialEq)]
enum StateKind {
    /// The state for one parameter (hand), by index into `parameters`.
    Hand(usize),
    /// The both-hands capped-sum state (analog, two-parameter layers only).
    Both,
}

struct GState {
    gesture: usize,
    kind: StateKind,
    state_id: i64,
    /// `Some` in analog mode: the state's BlendTree fileID (1D per-hand, 2D for `Both`).
    tree_id: Option<i64>,
    name: String,
    motion: ObjectRef,
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

/// One `AnimatorCondition`: `parameter <mode> threshold`.
#[derive(Clone)]
enum Cond {
    /// `Equals` on an `Int` parameter.
    Eq(String, i64),
    /// `NotEqual` on an `Int` parameter.
    Ne(String, i64),
    /// `Greater` on a `Float` parameter.
    Greater(String, f32),
    /// `Less` on a `Float` parameter.
    Less(String, f32),
}

/// An Any-State transition: all `conditions` must hold (AND).
struct Transition {
    id: i64,
    conditions: Vec<Cond>,
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
            for cond in &t.conditions {
                let (mode, parameter) = match cond {
                    Cond::Eq(p, _) => (CONDITION_EQUALS, p),
                    Cond::Ne(p, _) => (CONDITION_NOT_EQUAL, p),
                    Cond::Greater(p, _) => (CONDITION_GREATER, p),
                    Cond::Less(p, _) => (CONDITION_LESS, p),
                };
                e.line(&format!("- m_ConditionMode: {mode}"));
                e.indented(|e| {
                    e.kv("m_ConditionEvent", parameter);
                    // Unity's field name really is misspelled ("Treshold").
                    match cond {
                        Cond::Eq(_, v) | Cond::Ne(_, v) => e.kv_i64("m_EventTreshold", *v),
                        Cond::Greater(_, w) | Cond::Less(_, w) => e.kv_f32("m_EventTreshold", *w),
                    }
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
            if layer.analog {
                let w = format!("{p}Weight");
                if !declared.contains(&w) {
                    controller = controller.parameter(AnimatorParameter::float(&w));
                    declared.push(w);
                }
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
        // Mutual exclusivity, discrete mode: the earlier parameter's (left-hand) transitions
        // additionally require the later (winning) parameter to be 0; the later parameter's are
        // unconditional. So no two transitions to different states can be valid at once.
        for t in &transitions {
            let conds = t.body["m_Conditions"].as_vec().unwrap();
            let first_event = conds[0]["m_ConditionEvent"].as_str().unwrap();
            if conds[0]["m_EventTreshold"].as_i64() == Some(0) {
                continue; // the all-zero Neutral transition
            }
            if first_event == "GestureLeft" {
                assert_eq!(conds.len(), 2);
                assert_eq!(conds[1]["m_ConditionEvent"].as_str(), Some("GestureRight"));
                assert_eq!(conds[1]["m_EventTreshold"].as_i64(), Some(0));
            } else {
                assert_eq!(conds.len(), 1, "right-hand transitions are unconditional");
            }
        }
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

    /// Analog either-hand layer: one state *per (gesture, hand)* named `Fist L`/`Fist R`, each
    /// playing a local 1D BlendTree on that hand's weight float (`Neutral` clip at 0 → the
    /// gesture clip at 1); transitions route each hand's gesture value to that hand's state; the
    /// controller declares the weight floats.
    #[test]
    fn analog_layer_blends_each_hand_on_its_weight() {
        let neutral = ObjectRef::external(7400000, "a000000000000000000000000000000a", 2);
        let fist = ObjectRef::external(7400000, "a000000000000000000000000000000b", 2);
        let half = ObjectRef::external(7400000, "a000000000000000000000000000000c", 2);
        let layer = GestureLayer::either_hand("Gestures", neutral)
            .motion(1, fist)
            .motion_half(1, half)
            .analog();
        let yaml = fx_gestures("FX", &[layer], &[], &mut IdGen::new("FX"));
        let file = UnityFile::parse(&yaml).unwrap();

        // Params: both gesture ints and both weight floats (m_Type 1).
        let ctrl = ReaderController::from_file(&file).unwrap();
        let params: Vec<(String, i64)> = ctrl
            .parameters
            .iter()
            .map(|p| (p.name.clone(), p.raw_type))
            .collect();
        assert!(params.contains(&("GestureLeftWeight".into(), 1)));
        assert!(params.contains(&("GestureRightWeight".into(), 1)));

        // States: Neutral + Fist L + Fist R + Fist LR, the gesture states playing local trees.
        let states: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1102)
            .collect();
        let names: Vec<&str> = states.iter().map(|d| d.name().unwrap()).collect();
        assert_eq!(names, vec!["Neutral", "Fist L", "Fist R", "Fist LR"]);
        let trees: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 206)
            .collect();
        assert_eq!(trees.len(), 3);
        // The LR state's tree: 2D freeform cartesian over both weights, sampling the capped sum
        // (neutral at the origin, half clips at (0.5, 0)/(0, 0.5), full on the diagonal).
        let pos = |c: &avatar_unity_yaml::Yaml, k: &str| {
            c["m_Position"][k]
                .as_f64()
                .or(c["m_Position"][k].as_i64().map(|v| v as f64))
                .unwrap()
        };
        let lr = states.iter().find(|d| d.name() == Some("Fist LR")).unwrap();
        let lr_tree_id = lr.body["m_Motion"]["fileID"].as_i64().unwrap();
        let lr_tree = trees.iter().find(|t| t.file_id == lr_tree_id).unwrap();
        assert_eq!(lr_tree.body["m_BlendType"].as_i64(), Some(3));
        assert_eq!(
            lr_tree.body["m_BlendParameter"].as_str(),
            Some("GestureLeftWeight")
        );
        assert_eq!(
            lr_tree.body["m_BlendParameterY"].as_str(),
            Some("GestureRightWeight")
        );
        let childs = lr_tree.body["m_Childs"].as_vec().unwrap();
        assert_eq!(childs.len(), 7, "neutral + 2 half + 4 full samples");
        let half_positions: Vec<(f64, f64)> = childs
            .iter()
            .filter(|c| c["m_Motion"]["guid"].as_str() == Some("a000000000000000000000000000000c"))
            .map(|c| (pos(c, "x"), pos(c, "y")))
            .collect();
        assert_eq!(half_positions, vec![(0.5, 0.0), (0.0, 0.5)]);
        for (state, param) in states[1..]
            .iter()
            .zip(["GestureLeftWeight", "GestureRightWeight"])
        {
            let tree_id = state.body["m_Motion"]["fileID"].as_i64().unwrap();
            let tree = trees.iter().find(|t| t.file_id == tree_id).unwrap();
            assert_eq!(tree.body["m_BlendParameter"].as_str(), Some(param));
            let childs = tree.body["m_Childs"].as_vec().unwrap();
            assert_eq!(childs.len(), 2);
            assert_eq!(childs[0]["m_Threshold"].as_i64(), Some(0));
            assert_eq!(
                childs[0]["m_Motion"]["guid"].as_str(),
                Some("a000000000000000000000000000000a"),
                "threshold 0 plays the Neutral clip"
            );
            assert_eq!(childs[1]["m_Threshold"].as_i64(), Some(1));
            assert_eq!(
                childs[1]["m_Motion"]["guid"].as_str(),
                Some("a000000000000000000000000000000b"),
                "threshold 1 plays the gesture clip"
            );
        }

        // Transitions are mutually exclusive and weight-gated. Right (the winning hand):
        // one transition, `GestureRight == 1 AND GestureRightWeight > WEIGHT_ON`. Left: two
        // alternatives, `GestureLeft == 1 AND (GestureRight == 0 | GestureRightWeight <
        // WEIGHT_OFF)` — so a right thumb resting on the pad (Fist at weight 0) cannot mask or
        // oscillate against a left-hand expression.
        let transitions: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1101)
            .collect();
        // 1 Neutral + Fist (1 LR + 1 right + 2 left) + 6 unclipped gestures x (1 right + 2
        // left) = 23.
        assert_eq!(transitions.len(), 23);
        let dst_name = |t: &&avatar_unity_yaml::UnityDocument| {
            let dst = t.body["m_DstState"]["fileID"].as_i64().unwrap();
            states
                .iter()
                .find(|s| s.file_id == dst)
                .and_then(|s| s.name())
                .unwrap()
                .to_string()
        };
        let fist: Vec<_> = transitions
            .iter()
            .filter(|t| {
                let c = &t.body["m_Conditions"][0];
                c["m_EventTreshold"].as_i64() == Some(1)
            })
            .collect();
        assert_eq!(fist.len(), 4);
        for t in &fist {
            let conds = t.body["m_Conditions"].as_vec().unwrap();
            match (
                conds[0]["m_ConditionEvent"].as_str().unwrap(),
                dst_name(t).as_str(),
            ) {
                ("GestureLeft", "Fist LR") => {
                    // Both hands on the same gesture: L == 1 AND R == 1.
                    assert_eq!(conds.len(), 2);
                    assert_eq!(conds[1]["m_ConditionEvent"].as_str(), Some("GestureRight"));
                    assert_eq!(conds[1]["m_ConditionMode"].as_i64(), Some(CONDITION_EQUALS));
                    assert_eq!(conds[1]["m_EventTreshold"].as_i64(), Some(1));
                }
                ("GestureRight", "Fist R") => {
                    // R == 1 AND the weight gate AND the left hand NOT also on Fist.
                    assert_eq!(conds.len(), 3);
                    assert_eq!(
                        conds[1]["m_ConditionEvent"].as_str(),
                        Some("GestureRightWeight")
                    );
                    assert_eq!(
                        conds[1]["m_ConditionMode"].as_i64(),
                        Some(CONDITION_GREATER)
                    );
                    assert_eq!(conds[2]["m_ConditionEvent"].as_str(), Some("GestureLeft"));
                    assert_eq!(
                        conds[2]["m_ConditionMode"].as_i64(),
                        Some(CONDITION_NOT_EQUAL)
                    );
                }
                ("GestureLeft", "Fist L") => {
                    // Alternative A: R == 0 (2 conds). Alternative B: RW < off AND R != 1 (3).
                    let ev = conds[1]["m_ConditionEvent"].as_str().unwrap();
                    let mode = conds[1]["m_ConditionMode"].as_i64().unwrap();
                    let is_eq0 = conds.len() == 2
                        && ev == "GestureRight"
                        && mode == CONDITION_EQUALS
                        && conds[1]["m_EventTreshold"].as_i64() == Some(0);
                    let is_weight_off = conds.len() == 3
                        && ev == "GestureRightWeight"
                        && mode == CONDITION_LESS
                        && conds[2]["m_ConditionMode"].as_i64() == Some(CONDITION_NOT_EQUAL);
                    assert!(
                        is_eq0 || is_weight_off,
                        "{ev} mode {mode} len {}",
                        conds.len()
                    );
                }
                other => panic!("unexpected fist transition {other:?}"),
            }
        }

        // Deterministic.
        let layer2 = GestureLayer::either_hand(
            "Gestures",
            ObjectRef::external(7400000, "a000000000000000000000000000000a", 2),
        )
        .motion(
            1,
            ObjectRef::external(7400000, "a000000000000000000000000000000b", 2),
        )
        .motion_half(
            1,
            ObjectRef::external(7400000, "a000000000000000000000000000000c", 2),
        )
        .analog();
        assert_eq!(
            yaml,
            fx_gestures("FX", &[layer2], &[], &mut IdGen::new("FX"))
        );
    }
}
