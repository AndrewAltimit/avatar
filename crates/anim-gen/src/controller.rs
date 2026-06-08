//! Full FX **AnimatorController** (`.controller`, class id **91**) assembly.
//!
//! [`blendtree`](crate::blendtree) emits the inner trio — `AnimatorStateMachine` (1107),
//! `AnimatorState` (1102), `BlendTree` (206) — as a self-contained fragment a user pastes into an
//! *existing* FX controller. This module reverses that scope decision for the case where the user
//! wants a brand-new FX controller built around an analog-gesture blend tree: it emits the
//! enclosing class-91 `AnimatorController` object (its `m_AnimatorParameters` and
//! `m_AnimatorLayers`, each layer's `m_StateMachine` referencing the fragment's state machine) and
//! prepends it to the fragment, yielding a complete multi-document `.controller` stream.
//!
//! The headline entry point is [`fx_blend_tree`]: given a blend tree, it allocates the controller
//! id first (so it is the lowest, stablest id in the file), assembles the state/state-machine/tree
//! fragment via [`BlendTree::to_state_fragment`], auto-declares the tree's blend parameter as a
//! `Float`, wires a single layer to the fragment's state machine, and returns the whole file.
//!
//! # The Unity-import caveat (the "last mile")
//!
//! The field set and ordering here are matched against a real Unity-authored FX controller —
//! including the `serializedVersion` markers Unity's importer checks (91 → 5, the layer sub-struct
//! → 5; parameter entries carry no top-level `serializedVersion`). The in-repo round-trip test only
//! proves the output parses through *our* reader ([`avatar_unity_asset::AnimatorController`]); it
//! cannot prove a *specific* Unity editor accepts it on import — that interactive Unity step is the
//! one this toolchain deliberately does not own. See `docs/reference/anim-gen.md`.

use crate::IdGen;
use crate::blendtree::BlendTree;
use crate::yaml_emit::{Emitter, ObjectRef, UNITY_PREAMBLE};

/// An animator parameter's type, carrying Unity's raw `m_Type` int.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    /// `m_Type: 1`.
    Float,
    /// `m_Type: 3`.
    Int,
    /// `m_Type: 4`.
    Bool,
    /// `m_Type: 9`.
    Trigger,
}

impl ParamType {
    /// The raw Unity `m_Type` value.
    pub fn raw(self) -> i64 {
        match self {
            ParamType::Float => 1,
            ParamType::Int => 3,
            ParamType::Bool => 4,
            ParamType::Trigger => 9,
        }
    }
}

/// A declared animator parameter (`m_AnimatorParameters` entry).
#[derive(Debug, Clone)]
pub struct AnimatorParameter {
    pub name: String,
    pub param_type: ParamType,
}

impl AnimatorParameter {
    /// A `Float` parameter (e.g. `GestureLeftWeight`).
    pub fn float(name: impl Into<String>) -> Self {
        AnimatorParameter {
            name: name.into(),
            param_type: ParamType::Float,
        }
    }

    /// An `Int` parameter (e.g. `GestureLeft`).
    pub fn int(name: impl Into<String>) -> Self {
        AnimatorParameter {
            name: name.into(),
            param_type: ParamType::Int,
        }
    }

    /// A `Bool` parameter.
    pub fn bool(name: impl Into<String>) -> Self {
        AnimatorParameter {
            name: name.into(),
            param_type: ParamType::Bool,
        }
    }

    /// A `Trigger` parameter.
    pub fn trigger(name: impl Into<String>) -> Self {
        AnimatorParameter {
            name: name.into(),
            param_type: ParamType::Trigger,
        }
    }
}

/// One animator layer (an `AnimatorControllerLayer` entry in `m_AnimatorLayers`), pointing at the
/// state machine that backs it by local fileID.
#[derive(Debug, Clone)]
pub struct AnimatorLayer {
    pub name: String,
    pub state_machine_id: i64,
}

/// A full FX `AnimatorController` (class 91): its declared parameters and its layers.
///
/// This models *only* the class-91 object. The state machines / states / blend trees a layer
/// references live in separate documents (emitted by [`BlendTree::to_state_fragment`]); the layer's
/// `state_machine_id` is the local fileID linking the two.
#[derive(Debug, Clone)]
pub struct AnimatorController {
    pub name: String,
    pub parameters: Vec<AnimatorParameter>,
    pub layers: Vec<AnimatorLayer>,
}

impl AnimatorController {
    /// A new, empty controller named `name`.
    pub fn new(name: impl Into<String>) -> Self {
        AnimatorController {
            name: name.into(),
            parameters: Vec::new(),
            layers: Vec::new(),
        }
    }

    /// Declare a parameter (builder-style).
    pub fn parameter(mut self, p: AnimatorParameter) -> Self {
        self.parameters.push(p);
        self
    }

    /// Add a layer backed by the state machine at `state_machine_id` (builder-style).
    pub fn layer(mut self, name: impl Into<String>, state_machine_id: i64) -> Self {
        self.layers.push(AnimatorLayer {
            name: name.into(),
            state_machine_id,
        });
        self
    }

    /// Emit only the class-91 `AnimatorController` document into `e` with the given file id.
    ///
    /// Matches Unity's field set/order for a real authored FX controller — including the
    /// `serializedVersion` markers the importer checks (91 → 5, each layer sub-struct → 5;
    /// individual parameter entries carry none).
    pub fn emit_controller(&self, e: &mut Emitter, file_id: i64) {
        e.doc_header(91, file_id);
        e.line("AnimatorController:");
        e.indented(|e| {
            e.kv("m_ObjectHideFlags", "0");
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            e.kv("serializedVersion", "5");

            // m_AnimatorParameters
            if self.parameters.is_empty() {
                e.kv("m_AnimatorParameters", "[]");
            } else {
                e.key("m_AnimatorParameters");
                for p in &self.parameters {
                    e.line(&format!("- m_Name: {}", p.name));
                    e.indented(|e| {
                        e.kv_i64("m_Type", p.param_type.raw());
                        e.kv_i64("m_DefaultFloat", 0);
                        e.kv_i64("m_DefaultInt", 0);
                        e.kv_i64("m_DefaultBool", 0);
                        e.kv("m_Controller", "{fileID: 0}");
                    });
                }
            }

            // m_AnimatorLayers
            if self.layers.is_empty() {
                e.kv("m_AnimatorLayers", "[]");
            } else {
                e.key("m_AnimatorLayers");
                for layer in &self.layers {
                    e.line("- serializedVersion: 5");
                    e.indented(|e| {
                        e.kv("m_Name", &layer.name);
                        e.kv_ref("m_StateMachine", &ObjectRef::local(layer.state_machine_id));
                        e.kv("m_Mask", "{fileID: 0}");
                        e.kv("m_Motions", "[]");
                        e.kv("m_Behaviours", "[]");
                        e.kv_i64("m_BlendingMode", 0);
                        e.kv_i64("m_SyncedLayerIndex", -1);
                        e.kv_i64("m_DefaultWeight", 1);
                        e.kv_i64("m_IKPass", 0);
                        e.kv_i64("m_SyncedLayerAffectsTiming", 0);
                        e.kv("m_Controller", "{fileID: 0}");
                    });
                }
            }
        });
    }
}

/// Assemble a complete FX `.controller` stream wrapping `tree` in a single layer.
///
/// Allocates the controller id first (the lowest, stablest id in the file), then builds the
/// state-machine/state/blend-tree fragment via [`BlendTree::to_state_fragment`], auto-declares the
/// tree's blend parameter as a `Float` (de-duplicated by name so a caller who pre-declared it isn't
/// double-listed), wires a single [`AnimatorLayer`] named `layer_name` to the fragment's state
/// machine, and returns the full multi-document text (`%YAML` preamble + class-91 doc + fragment).
pub fn fx_blend_tree(name: &str, layer_name: &str, tree: &BlendTree, ids: &mut IdGen) -> String {
    // Allocate the controller id first so it is the lowest / stablest id in the file.
    let controller_id = ids.alloc();
    let (fragment, sm_id) = tree.to_state_fragment(ids);

    let controller = AnimatorController::new(name)
        .parameter(AnimatorParameter::float(&tree.blend_parameter))
        .layer(layer_name, sm_id);

    let mut e = Emitter::new();
    controller.emit_controller(&mut e, controller_id);

    format!("{UNITY_PREAMBLE}{}{fragment}", e.into_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml_emit;
    use avatar_unity_asset::AnimatorController as ReaderController;
    use avatar_unity_yaml::UnityFile;

    fn analog_tree() -> BlendTree {
        BlendTree::analog_gesture("Fist", "GestureLeftWeight")
            .clip("1234567890abcdef1234567890abcdef", 0.0)
            .clip("abcdef1234567890abcdef1234567890", 1.0)
    }

    #[test]
    fn fx_blend_tree_emits_expected_headers_and_fields() {
        let mut ids = IdGen::new("FX");
        let tree = analog_tree();
        // Pre-compute the sm_id the fragment will use: controller takes the first id, then the
        // fragment allocates sm/state/tree in order, so sm_id is the second id.
        let mut probe = ids.clone();
        let _controller_id = probe.alloc();
        let sm_id = probe.alloc();

        let yaml = fx_blend_tree("FX", "Base Layer", &tree, &mut ids);

        // The class-91 controller and its key fields.
        assert!(yaml.contains("--- !u!91 &"));
        assert!(yaml.contains("AnimatorController:"));
        assert!(yaml.contains("m_Name: FX"));
        assert!(yaml.contains("serializedVersion: 5"));
        assert!(yaml.contains("m_AnimatorParameters:"));
        assert!(yaml.contains("m_AnimatorLayers:"));
        assert!(yaml.contains("m_Name: Base Layer"));
        // The layer points at the fragment's state machine.
        assert!(yaml.contains(&format!("m_StateMachine: {{fileID: {sm_id}}}")));
        // The auto-declared blend parameter.
        assert!(yaml.contains("GestureLeftWeight"));
        // The wrapped fragment's documents.
        assert!(yaml.contains("--- !u!1107"));
        assert!(yaml.contains("--- !u!1102"));
        assert!(yaml.contains("--- !u!206"));
    }

    #[test]
    fn fx_blend_tree_roundtrips_through_reader() {
        let mut ids = IdGen::new("FX");
        let tree = analog_tree();
        let yaml = fx_blend_tree("FX", "Base Layer", &tree, &mut ids);

        let file = UnityFile::parse(&yaml).expect("generated controller must parse");
        let c = ReaderController::from_file(&file).expect("controller document present");

        assert_eq!(c.name.as_deref(), Some("FX"));

        // The blend parameter is declared as a Float.
        let blend_param = c
            .parameters
            .iter()
            .find(|p| p.name == "GestureLeftWeight")
            .expect("blend parameter declared");
        assert_eq!(blend_param.type_name(), "Float");

        // Exactly one state machine, with a default state and one child.
        assert_eq!(c.state_machines.len(), 1);
        assert!(c.state_machines[0].has_default_state);
        assert_eq!(c.state_machines[0].child_state_count, 1);

        // The blend tree reads the blend parameter.
        assert_eq!(c.blend_trees.len(), 1);
        assert_eq!(
            c.blend_trees[0].referenced_parameters(),
            vec!["GestureLeftWeight"]
        );

        // Write Defaults OFF on the single state.
        assert_eq!(c.write_defaults, vec![false]);

        // Layer -> state-machine linkage: the raw 91 doc's first layer m_StateMachine.fileID must
        // equal the 1107 document's fileID.
        let controller_doc = file
            .documents
            .iter()
            .find(|d| d.class_id == 91)
            .expect("class-91 doc");
        let layers = controller_doc.body["m_AnimatorLayers"]
            .as_vec()
            .expect("m_AnimatorLayers seq");
        let layer_sm_id = layers[0]["m_StateMachine"]["fileID"]
            .as_i64()
            .expect("layer m_StateMachine.fileID");
        let sm_doc = file
            .documents
            .iter()
            .find(|d| d.class_id == 1107)
            .expect("class-1107 doc");
        assert_eq!(layer_sm_id, sm_doc.file_id);
    }

    #[test]
    fn fx_blend_tree_is_deterministic() {
        let tree = analog_tree();
        let mut a = IdGen::new("FX");
        let mut b = IdGen::new("FX");
        let ya = fx_blend_tree("FX", "Base Layer", &tree, &mut a);
        let yb = fx_blend_tree("FX", "Base Layer", &tree, &mut b);
        assert_eq!(ya, yb);
    }

    #[test]
    fn emit_controller_handles_empty_collections() {
        let mut e = Emitter::new();
        AnimatorController::new("Empty").emit_controller(&mut e, 9100000);
        let yaml = format!("{}{}", yaml_emit::UNITY_PREAMBLE, e.into_string());
        assert!(yaml.contains("m_AnimatorParameters: []"));
        assert!(yaml.contains("m_AnimatorLayers: []"));
    }
}
