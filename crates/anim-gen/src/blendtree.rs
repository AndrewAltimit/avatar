//! FX-layer analog-gesture **1D BlendTree** generation — the headline M4 feature (`PLAN.md` §4).
//!
//! VRChat exposes the analog trigger as `GestureLeftWeight` / `GestureRightWeight` (float 0→1). A
//! 1D BlendTree placed in the **Fist** gesture state of the FX layer, blending on that weight
//! parameter across child motions, lets *any* gesture reach *any* fraction — the generator analogue
//! of ComboGestureExpressions.
//!
//! A `BlendTree` is Unity class id **206**. The minimal wiring to make one usable from a controller
//! is three documents:
//!
//! - the `BlendTree` (206) with its `m_Childs` (each `{ m_Motion, m_Threshold, … }`),
//! - the `AnimatorState` (1102) whose `m_Motion` points at the tree, and
//! - the `AnimatorStateMachine` (1107) whose default state is that state.
//!
//! Emitting a *whole* AnimatorController (the layer list, AnimatorControllerLayer
//! `m_StateMachine` refs, the 91 object) is large and brittle across SDK versions, and a user
//! almost always wants to graft the tree into the *existing* FX controller their avatar ships with.
//! So this module emits the **BlendTree document (+ optionally the owning state/state-machine)** as
//! a self-contained fragment, with a documented note ([`BlendTree::wiring_note`]) on how to point an
//! existing state's `m_Motion` at it. The full standalone trio is available via
//! [`BlendTree::to_state_fragment`] for callers who want a drop-in sub-state-machine.

use crate::IdGen;
use crate::gesture::{CONDITION_GREATER, CONDITION_LESS};
use crate::yaml_emit::{Emitter, ObjectRef};

/// `m_BlendType` on a BlendTree: 0 = Simple 1D. (2D and Direct exist; this generator emits 1D.)
pub const BLEND_TYPE_1D: i64 = 0;

/// One child motion of a blend tree.
#[derive(Debug, Clone)]
pub struct ChildMotion {
    /// The motion this child plays: a clip (external `.anim`, by guid) or a nested tree (local
    /// fileID). Use [`ObjectRef::external`] for a clip and [`ObjectRef::local`] for a nested tree.
    pub motion: ObjectRef,
    /// The blend-parameter value at which this child is fully weighted.
    pub threshold: f32,
    /// `m_TimeScale` (playback speed); 1 for normal.
    pub time_scale: f32,
    /// `m_Mirror`.
    pub mirror: bool,
}

impl ChildMotion {
    /// A child that plays an external clip (an AnimationClip in a `.anim`, class-74 fileID
    /// `7400000`, asset type 2) at `threshold`.
    pub fn clip(guid: impl Into<String>, threshold: f32) -> Self {
        ChildMotion {
            motion: ObjectRef::external(7400000, guid, 2),
            threshold,
            time_scale: 1.0,
            mirror: false,
        }
    }

    /// A child whose motion is an arbitrary [`ObjectRef`] (e.g. a nested local blend tree).
    pub fn motion(motion: ObjectRef, threshold: f32) -> Self {
        ChildMotion {
            motion,
            threshold,
            time_scale: 1.0,
            mirror: false,
        }
    }
}

/// A 1D blend tree blending on a single float parameter across [`ChildMotion`]s.
#[derive(Debug, Clone)]
pub struct BlendTree {
    pub name: String,
    /// The blend parameter, e.g. `GestureLeftWeight`.
    pub blend_parameter: String,
    pub children: Vec<ChildMotion>,
    /// `m_UseAutomaticThresholds`: when true Unity spreads thresholds evenly and ignores the
    /// per-child values. We default to *false* (explicit thresholds) so analog mapping is exact.
    pub automatic_thresholds: bool,
}

impl BlendTree {
    /// A new 1D analog-gesture blend tree on `blend_parameter` (e.g. `GestureLeftWeight`).
    pub fn analog_gesture(name: impl Into<String>, blend_parameter: impl Into<String>) -> Self {
        BlendTree {
            name: name.into(),
            blend_parameter: blend_parameter.into(),
            children: Vec::new(),
            automatic_thresholds: false,
        }
    }

    /// Add a child motion (builder-style).
    pub fn child(mut self, child: ChildMotion) -> Self {
        self.children.push(child);
        self
    }

    /// Add a child clip at `threshold` by its `.anim` guid (builder-style convenience).
    pub fn clip(self, guid: impl Into<String>, threshold: f32) -> Self {
        self.child(ChildMotion::clip(guid, threshold))
    }

    /// The thresholds, in child order — handy for tests and for computing min/max.
    pub fn thresholds(&self) -> Vec<f32> {
        self.children.iter().map(|c| c.threshold).collect()
    }

    /// Emit just the `BlendTree` (class 206) document into `e` with the given file id.
    pub fn emit_tree(&self, e: &mut Emitter, file_id: i64) {
        e.doc_header(206, file_id);
        e.line("BlendTree:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "1");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            if self.children.is_empty() {
                e.kv("m_Childs", "[]");
            } else {
                e.key("m_Childs");
                for c in &self.children {
                    emit_child(e, c);
                }
            }
            // `m_BlendParameter` / `m_BlendParameterY` are *string* fields (parameter names).
            e.kv("m_BlendParameter", &self.blend_parameter);
            e.kv("m_BlendParameterY", "Blend");
            e.kv_i64("m_MinThreshold", 0);
            e.kv_f32("m_MaxThreshold", self.max_threshold());
            e.kv_i64("m_UseAutomaticThresholds", self.automatic_thresholds as i64);
            e.kv_i64("m_NormalizedBlendValues", 0);
            e.kv_i64("m_BlendType", BLEND_TYPE_1D);
        });
    }

    fn max_threshold(&self) -> f32 {
        self.children
            .iter()
            .map(|c| c.threshold)
            .fold(1.0_f32, f32::max)
    }

    /// Emit a self-contained fragment: the `AnimatorStateMachine` (1107), `AnimatorState` (1102)
    /// and `BlendTree` (206), wired so the state machine's default state plays the tree. File ids
    /// are allocated from `ids` deterministically. Returns the rendered fragment text **without**
    /// the `%YAML` preamble (it is a fragment to splice into a controller), and the state-machine
    /// file id (the entry point a layer's `m_StateMachine` should reference).
    pub fn to_state_fragment(&self, ids: &mut IdGen) -> (String, i64) {
        let sm_id = ids.alloc();
        let state_id = ids.alloc();
        let tree_id = ids.alloc();

        let mut e = Emitter::new();

        // --- AnimatorStateMachine (1107)
        e.doc_header(1107, sm_id);
        e.line("AnimatorStateMachine:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "1");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            e.key("m_ChildStates");
            e.indented(|e| {
                e.line("- serializedVersion: 1");
                e.indented(|e| {
                    e.kv_ref("m_State", &ObjectRef::local(state_id));
                    e.kv("m_Position", "{x: 200, y: 0, z: 0}");
                });
            });
            e.kv("m_ChildStateMachines", "[]");
            e.kv("m_AnyStateTransitions", "[]");
            e.kv("m_EntryTransitions", "[]");
            e.kv("m_StateMachineTransitions", "{}");
            e.kv("m_StateMachineBehaviours", "[]");
            e.kv("m_AnyStatePosition", "{x: 50, y: 20, z: 0}");
            e.kv("m_EntryPosition", "{x: 50, y: 120, z: 0}");
            e.kv("m_ExitPosition", "{x: 800, y: 120, z: 0}");
            e.kv("m_ParentStateMachinePosition", "{x: 800, y: 20, z: 0}");
            e.kv_ref("m_DefaultState", &ObjectRef::local(state_id));
        });

        // --- AnimatorState (1102)
        e.doc_header(1102, state_id);
        e.line("AnimatorState:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "1");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            e.kv("m_Speed", "1");
            e.kv("m_CycleOffset", "0");
            e.kv("m_Transitions", "[]");
            e.kv("m_StateMachineBehaviours", "[]");
            e.kv("m_Position", "{x: 50, y: 50, z: 0}");
            e.kv("m_IKOnFeet", "0");
            // Write Defaults OFF — VRChat's recommendation for FX clips that don't write every
            // animated property every frame.
            e.kv("m_WriteDefaultValues", "0");
            e.kv("m_Mirror", "0");
            e.kv("m_SpeedParameterActive", "0");
            e.kv("m_MirrorParameterActive", "0");
            e.kv("m_CycleOffsetParameterActive", "0");
            e.kv("m_TimeParameterActive", "0");
            e.kv_ref("m_Motion", &ObjectRef::local(tree_id));
            e.kv("m_Tag", "");
            e.kv("m_SpeedParameter", "");
            e.kv("m_MirrorParameter", "");
            e.kv("m_CycleOffsetParameter", "");
            e.kv("m_TimeParameter", "");
        });

        // --- BlendTree (206)
        self.emit_tree(&mut e, tree_id);

        (e.into_string(), sm_id)
    }

    /// Emit a **gated layer** fragment for this tree: an `Off` default state that plays nothing
    /// (Write Defaults off — the layer writes no properties, so lower layers keep owning them)
    /// and an `On` state playing the tree, with `Off → On` when `gate_parameter` rises above
    /// `on` and `On → Off` when it falls below `off` (the gap is hysteresis, so a value resting
    /// at one threshold can't chatter). The natural shape for a radial-puppet layer: at dial 0
    /// the layer is inert, any dial-up hands the properties to the tree.
    ///
    /// The fragment holds the `AnimatorStateMachine` (1107), both `AnimatorState`s (1102), both
    /// `AnimatorStateTransition`s (1101) and the `BlendTree` (206); returns it with the
    /// state-machine fileID for the layer's `m_StateMachine`.
    pub fn to_gated_layer_fragment(
        &self,
        gate_parameter: &str,
        on: f32,
        off: f32,
        ids: &mut IdGen,
    ) -> (String, i64) {
        let sm_id = ids.alloc();
        let off_state = ids.alloc();
        let on_state = ids.alloc();
        let tree_id = ids.alloc();
        let t_on = ids.alloc();
        let t_off = ids.alloc();

        let mut e = Emitter::new();

        // --- AnimatorStateMachine (1107)
        e.doc_header(1107, sm_id);
        e.line("AnimatorStateMachine:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "1");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            e.key("m_ChildStates");
            e.indented(|e| {
                for (id, row) in [(off_state, 0), (on_state, 1)] {
                    e.line("- serializedVersion: 1");
                    e.indented(|e| {
                        e.kv_ref("m_State", &ObjectRef::local(id));
                        e.kv("m_Position", &format!("{{x: 300, y: {}, z: 0}}", 60 * row));
                    });
                }
            });
            e.kv("m_ChildStateMachines", "[]");
            e.kv("m_AnyStateTransitions", "[]");
            e.kv("m_EntryTransitions", "[]");
            e.kv("m_StateMachineTransitions", "{}");
            e.kv("m_StateMachineBehaviours", "[]");
            e.kv("m_AnyStatePosition", "{x: 50, y: 20, z: 0}");
            e.kv("m_EntryPosition", "{x: 50, y: 120, z: 0}");
            e.kv("m_ExitPosition", "{x: 800, y: 120, z: 0}");
            e.kv("m_ParentStateMachinePosition", "{x: 800, y: 20, z: 0}");
            e.kv_ref("m_DefaultState", &ObjectRef::local(off_state));
        });

        // --- AnimatorStates (1102): Off (no motion) and On (the tree).
        for (id, name, motion, transition) in [
            (off_state, "Off", ObjectRef::null(), t_on),
            (on_state, "On", ObjectRef::local(tree_id), t_off),
        ] {
            e.doc_header(1102, id);
            e.line("AnimatorState:");
            e.indented(|e| {
                e.kv("m_ObjectHideFlags", "1");
                e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
                e.kv("m_PrefabInstance", "{fileID: 0}");
                e.kv("m_PrefabAsset", "{fileID: 0}");
                e.kv("m_Name", name);
                e.kv("m_Speed", "1");
                e.kv("m_CycleOffset", "0");
                e.key("m_Transitions");
                e.indented(|e| {
                    e.line(&format!("- {}", ObjectRef::local(transition).render()));
                });
                e.kv("m_StateMachineBehaviours", "[]");
                e.kv("m_Position", "{x: 50, y: 50, z: 0}");
                e.kv("m_IKOnFeet", "0");
                e.kv("m_WriteDefaultValues", "0");
                e.kv("m_Mirror", "0");
                e.kv("m_SpeedParameterActive", "0");
                e.kv("m_MirrorParameterActive", "0");
                e.kv("m_CycleOffsetParameterActive", "0");
                e.kv("m_TimeParameterActive", "0");
                e.kv_ref("m_Motion", &motion);
                e.kv("m_Tag", "");
                e.kv("m_SpeedParameter", "");
                e.kv("m_MirrorParameter", "");
                e.kv("m_CycleOffsetParameter", "");
                e.kv("m_TimeParameter", "");
            });
        }

        // --- AnimatorStateTransitions (1101): Off→On (Greater `on`), On→Off (Less `off`).
        for (id, dst, mode, threshold) in [
            (t_on, on_state, CONDITION_GREATER, on),
            (t_off, off_state, CONDITION_LESS, off),
        ] {
            e.doc_header(1101, id);
            e.line("AnimatorStateTransition:");
            e.indented(|e| {
                e.kv("m_ObjectHideFlags", "1");
                e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
                e.kv("m_PrefabInstance", "{fileID: 0}");
                e.kv("m_PrefabAsset", "{fileID: 0}");
                e.kv("m_Name", "");
                e.key("m_Conditions");
                e.indented(|e| {
                    e.line(&format!("- m_ConditionMode: {mode}"));
                    e.indented(|e| {
                        e.kv("m_ConditionEvent", gate_parameter);
                        e.kv_f32("m_EventTreshold", threshold);
                    });
                });
                e.kv("m_DstStateMachine", "{fileID: 0}");
                e.kv_ref("m_DstState", &ObjectRef::local(dst));
                e.kv("m_Solo", "0");
                e.kv("m_Mute", "0");
                e.kv("m_IsExit", "0");
                e.kv("serializedVersion", "3");
                e.kv_f32("m_TransitionDuration", 0.1);
                e.kv_i64("m_TransitionOffset", 0);
                e.kv_f32("m_ExitTime", 0.75);
                e.kv("m_HasExitTime", "0");
                e.kv("m_HasFixedDuration", "1");
                e.kv_i64("m_InterruptionSource", 0);
                e.kv("m_OrderedInterruption", "1");
                e.kv("m_CanTransitionToSelf", "0");
            });
        }

        // --- BlendTree (206)
        self.emit_tree(&mut e, tree_id);

        (e.into_string(), sm_id)
    }

    /// A human-readable note on how to graft the emitted BlendTree into an existing FX controller.
    pub fn wiring_note(&self, tree_file_id: i64) -> String {
        format!(
            "To use this blend tree: in your FX `.controller`, set the Fist gesture state's \
`m_Motion` to `{{fileID: {tree_file_id}}}`, paste the `--- !u!206 &{tree_file_id}` document into \
the controller file, and declare the float parameter `{param}` in `m_AnimatorParameters` if it is \
not already present. VRChat populates `{param}` from the analog trigger automatically.",
            param = self.blend_parameter
        )
    }
}

fn emit_child(e: &mut Emitter, c: &ChildMotion) {
    e.line("- serializedVersion: 2");
    e.indented(|e| {
        e.kv_ref("m_Motion", &c.motion);
        e.kv_f32("m_Threshold", c.threshold);
        e.kv("m_Position", "{x: 0, y: 0}");
        e.kv_f32("m_TimeScale", c.time_scale);
        e.kv_i64("m_CycleOffset", 0);
        // `m_DirectBlendParameter` is a *string* (only meaningful for a Direct tree); Unity writes
        // the default `Blend` even on 1D children.
        e.kv("m_DirectBlendParameter", "Blend");
        e.kv_i64("m_Mirror", c.mirror as i64);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml_emit::UNITY_PREAMBLE;
    use avatar_unity_yaml::UnityFile;

    /// The gated layer fragment: default `Off` state with no motion and a `> on` transition into
    /// `On` (the tree), `< off` back out — hysteresis thresholds, deterministic ids.
    #[test]
    fn gated_layer_fragment_gates_the_tree_on_the_parameter() {
        let tree = BlendTree::analog_gesture("Blink", "Blink")
            .clip("a000000000000000000000000000000a", 0.0)
            .clip("a000000000000000000000000000000b", 1.0);
        let mut ids = IdGen::new("Blink puppet");
        let (frag, sm_id) = tree.to_gated_layer_fragment("Blink", 0.01, 0.005, &mut ids);
        let file = UnityFile::parse(&format!("{UNITY_PREAMBLE}{frag}")).unwrap();

        let sm = file.documents.iter().find(|d| d.class_id == 1107).unwrap();
        assert_eq!(sm.file_id, sm_id);
        let states: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1102)
            .collect();
        assert_eq!(
            states.iter().map(|s| s.name().unwrap()).collect::<Vec<_>>(),
            vec!["Off", "On"]
        );
        let off = states[0];
        let on = states[1];
        // Off is the default state and plays nothing; On plays the tree.
        assert_eq!(
            sm.body["m_DefaultState"]["fileID"].as_i64(),
            Some(off.file_id)
        );
        assert_eq!(off.body["m_Motion"]["fileID"].as_i64(), Some(0));
        let tree_doc = file.documents.iter().find(|d| d.class_id == 206).unwrap();
        assert_eq!(
            on.body["m_Motion"]["fileID"].as_i64(),
            Some(tree_doc.file_id)
        );
        // Transitions: Off's goes to On with Greater 0.01; On's back with Less 0.005.
        let transitions: Vec<_> = file
            .documents
            .iter()
            .filter(|d| d.class_id == 1101)
            .collect();
        assert_eq!(transitions.len(), 2);
        let by_id = |id: i64| transitions.iter().find(|t| t.file_id == id).unwrap();
        let t_on = by_id(off.body["m_Transitions"][0]["fileID"].as_i64().unwrap());
        assert_eq!(t_on.body["m_DstState"]["fileID"].as_i64(), Some(on.file_id));
        assert_eq!(
            t_on.body["m_Conditions"][0]["m_ConditionMode"].as_i64(),
            Some(3)
        );
        let t_off = by_id(on.body["m_Transitions"][0]["fileID"].as_i64().unwrap());
        assert_eq!(
            t_off.body["m_DstState"]["fileID"].as_i64(),
            Some(off.file_id)
        );
        assert_eq!(
            t_off.body["m_Conditions"][0]["m_ConditionMode"].as_i64(),
            Some(4)
        );
        // Deterministic.
        let (again, _) = BlendTree::analog_gesture("Blink", "Blink")
            .clip("a000000000000000000000000000000a", 0.0)
            .clip("a000000000000000000000000000000b", 1.0)
            .to_gated_layer_fragment("Blink", 0.01, 0.005, &mut IdGen::new("Blink puppet"));
        assert_eq!(frag, again);
    }
}
