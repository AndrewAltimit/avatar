//! Typed reading of Unity asset graphs over `avatar-unity-yaml`. Currently this covers the
//! **AnimatorController** (`.controller`) — the structure VRChat avatars drive through their
//! playable layers (FX, Gesture, Action, …).
//!
//! A `.controller` file is a multi-document Unity YAML stream: one `AnimatorController` object
//! (class 91) plus the state machines (1107), states (1102), transitions (1101 / 1109) and blend
//! trees (206) it owns, linked by local `fileID`s. For the lint rules we care about — which
//! parameters are referenced, write-defaults consistency, missing default states — we don't need
//! to rebuild the full graph; we aggregate the relevant fields across the file's documents by
//! their Unity class id. This is robust to SDK version drift, since the field names are stable
//! Unity serialization, not VRChat specifics.
//!
//! Reference: <https://docs.unity3d.com/Manual/class-AnimatorController.html>.

use avatar_unity_yaml::{UnityFile, Yaml, field_bool, field_f64, field_i64, field_str};
use serde::Serialize;

// Unity class ids that appear in a `.controller` file.
const ANIMATOR_CONTROLLER: u32 = 91;
const ANIMATOR_STATE: u32 = 1102;
const ANIMATOR_STATE_TRANSITION: u32 = 1101;
const ANIMATOR_TRANSITION: u32 = 1109;
const ANIMATOR_STATE_MACHINE: u32 = 1107;
const BLEND_TREE: u32 = 206;

// `m_BlendType` values on a BlendTree.
const BLEND_TYPE_1D: i64 = 0;
const BLEND_TYPE_DIRECT: i64 = 4;

/// A declared animator parameter (`m_AnimatorParameters` entry).
#[derive(Debug, Clone, Serialize)]
pub struct AnimatorParameter {
    pub name: String,
    /// Raw `m_Type`: 1 Float, 3 Int, 4 Bool, 9 Trigger.
    pub raw_type: i64,
}

impl AnimatorParameter {
    pub fn type_name(&self) -> &'static str {
        match self.raw_type {
            1 => "Float",
            3 => "Int",
            4 => "Bool",
            9 => "Trigger",
            _ => "Unknown",
        }
    }
}

/// A single transition condition (`m_Conditions` entry).
#[derive(Debug, Clone, Serialize)]
pub struct AnimatorCondition {
    /// The parameter the condition tests (`m_ConditionEvent`).
    pub parameter: String,
    /// Raw `m_ConditionMode`: 1 If, 2 IfNot, 3 Greater, 4 Less, 6 Equals, 7 NotEqual.
    pub mode: i64,
    pub threshold: f64,
}

/// A blend tree (`BlendTree`, class 206).
#[derive(Debug, Clone, Serialize)]
pub struct BlendTreeInfo {
    /// `m_BlendType`: 0 = 1D, 1–3 = 2D variants, 4 = Direct.
    pub blend_type: i64,
    pub blend_parameter: String,
    pub blend_parameter_y: String,
    /// Per-child `m_DirectBlendParameter` (only meaningful for a Direct blend tree).
    pub direct_parameters: Vec<String>,
}

impl BlendTreeInfo {
    /// The parameter names this blend tree actually reads, given its blend type. (A 1D tree uses
    /// only X; a 2D tree uses X and Y; a Direct tree uses each child's direct parameter.)
    pub fn referenced_parameters(&self) -> Vec<&str> {
        let mut out: Vec<&str> = match self.blend_type {
            BLEND_TYPE_1D => vec![self.blend_parameter.as_str()],
            BLEND_TYPE_DIRECT => self.direct_parameters.iter().map(String::as_str).collect(),
            _ => vec![
                self.blend_parameter.as_str(),
                self.blend_parameter_y.as_str(),
            ],
        };
        out.retain(|s| !s.is_empty());
        out
    }
}

/// A state machine within the controller (`AnimatorStateMachine`, class 1107).
#[derive(Debug, Clone, Serialize)]
pub struct StateMachineInfo {
    pub child_state_count: usize,
    /// `true` if `m_DefaultState` points at a real state.
    pub has_default_state: bool,
}

/// An AnimatorController parsed from a `.controller` file.
#[derive(Debug, Clone, Serialize)]
pub struct AnimatorController {
    pub name: Option<String>,
    pub parameters: Vec<AnimatorParameter>,
    /// Every condition across every transition in the file.
    pub conditions: Vec<AnimatorCondition>,
    pub blend_trees: Vec<BlendTreeInfo>,
    pub state_machines: Vec<StateMachineInfo>,
    /// `m_WriteDefaultValues` for every state, in document order.
    pub write_defaults: Vec<bool>,
    pub state_count: usize,
}

impl AnimatorController {
    /// Parse the controller out of a Unity file, aggregating its owned objects. Returns `None` if
    /// the file contains no `AnimatorController` document.
    ///
    /// Note: a `.controller` file holds exactly one controller; if a file somehow contains more
    /// than one, the owned objects are attributed to the controller as a whole (not split).
    pub fn from_file(file: &UnityFile) -> Option<Self> {
        let controller = file
            .documents
            .iter()
            .find(|d| d.class_id == ANIMATOR_CONTROLLER)?;

        let parameters = controller.body["m_AnimatorParameters"]
            .as_vec()
            .map(|v| {
                v.iter()
                    .map(|p| AnimatorParameter {
                        name: field_str(p, "m_Name").unwrap_or_default().to_string(),
                        raw_type: field_i64(p, "m_Type").unwrap_or(-1),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut conditions = Vec::new();
        let mut blend_trees = Vec::new();
        let mut state_machines = Vec::new();
        let mut write_defaults = Vec::new();
        let mut state_count = 0;

        for doc in &file.documents {
            match doc.class_id {
                ANIMATOR_STATE_TRANSITION | ANIMATOR_TRANSITION => {
                    collect_conditions(&doc.body, &mut conditions);
                }
                BLEND_TREE => blend_trees.push(parse_blend_tree(&doc.body)),
                ANIMATOR_STATE => {
                    state_count += 1;
                    write_defaults
                        .push(field_bool(&doc.body, "m_WriteDefaultValues").unwrap_or(true));
                }
                ANIMATOR_STATE_MACHINE => state_machines.push(parse_state_machine(&doc.body)),
                _ => {}
            }
        }

        Some(AnimatorController {
            name: controller.name().map(str::to_string),
            parameters,
            conditions,
            blend_trees,
            state_machines,
            write_defaults,
            state_count,
        })
    }

    /// The set of declared parameter names.
    pub fn parameter_names(&self) -> impl Iterator<Item = &str> {
        self.parameters.iter().map(|p| p.name.as_str())
    }
}

fn collect_conditions(body: &Yaml, out: &mut Vec<AnimatorCondition>) {
    let Some(list) = body["m_Conditions"].as_vec() else {
        return;
    };
    for c in list {
        let parameter = field_str(c, "m_ConditionEvent")
            .unwrap_or_default()
            .to_string();
        if parameter.is_empty() {
            continue;
        }
        out.push(AnimatorCondition {
            parameter,
            mode: field_i64(c, "m_ConditionMode").unwrap_or(0),
            // Unity's serialized field is the (misspelled) `m_EventTreshold`.
            threshold: field_f64(c, "m_EventTreshold").unwrap_or(0.0),
        });
    }
}

fn parse_blend_tree(body: &Yaml) -> BlendTreeInfo {
    let direct_parameters = body["m_Childs"]
        .as_vec()
        .map(|v| {
            v.iter()
                .filter_map(|c| field_str(c, "m_DirectBlendParameter"))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    BlendTreeInfo {
        blend_type: field_i64(body, "m_BlendType").unwrap_or(0),
        blend_parameter: field_str(body, "m_BlendParameter")
            .unwrap_or_default()
            .to_string(),
        blend_parameter_y: field_str(body, "m_BlendParameterY")
            .unwrap_or_default()
            .to_string(),
        direct_parameters,
    }
}

fn parse_state_machine(body: &Yaml) -> StateMachineInfo {
    let child_state_count = body["m_ChildStates"].as_vec().map(Vec::len).unwrap_or(0);
    let has_default_state = field_i64(&body["m_DefaultState"], "fileID").is_some_and(|id| id != 0);
    StateMachineInfo {
        child_state_count,
        has_default_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTROLLER: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!91 &9100000
AnimatorController:
  m_Name: FX
  m_AnimatorParameters:
  - m_Name: GestureLeft
    m_Type: 3
    m_DefaultFloat: 0
  - m_Name: GestureLeftWeight
    m_Type: 1
    m_DefaultFloat: 0
  m_AnimatorLayers:
  - m_Name: Base Layer
    m_StateMachine: {fileID: 110700000}
--- !u!1107 &110700000
AnimatorStateMachine:
  m_Name: Base Layer
  m_ChildStates:
  - serializedVersion: 1
    m_State: {fileID: 110200000}
  m_DefaultState: {fileID: 110200000}
--- !u!1102 &110200000
AnimatorState:
  m_Name: Fist
  m_WriteDefaultValues: 0
  m_Motion: {fileID: 110600000}
--- !u!206 &110600000
BlendTree:
  m_Name: Fist
  m_Childs:
  - m_Motion: {fileID: 7400000, guid: 1234567890abcdef1234567890abcdef, type: 2}
    m_Threshold: 0
  m_BlendParameter: GestureLeftWeight
  m_BlendParameterY: Blend
  m_BlendType: 0
--- !u!1101 &110100000
AnimatorStateTransition:
  m_Conditions:
  - m_ConditionMode: 2
    m_ConditionEvent: GestureLeft
    m_EventTreshold: 0
  m_DstState: {fileID: 110200000}
";

    #[test]
    fn parses_controller_aggregate() {
        let file = UnityFile::parse(CONTROLLER).unwrap();
        let c = AnimatorController::from_file(&file).expect("controller");

        assert_eq!(c.name.as_deref(), Some("FX"));
        assert_eq!(c.parameters.len(), 2);
        assert_eq!(c.parameters[0].type_name(), "Int");
        assert_eq!(c.state_count, 1);
        assert_eq!(c.write_defaults, vec![false]);

        // One condition referencing GestureLeft (declared).
        assert_eq!(c.conditions.len(), 1);
        assert_eq!(c.conditions[0].parameter, "GestureLeft");

        // 1D blend tree -> only the X parameter is read (not the unused Y "Blend").
        assert_eq!(c.blend_trees.len(), 1);
        assert_eq!(
            c.blend_trees[0].referenced_parameters(),
            vec!["GestureLeftWeight"]
        );

        // State machine has a default state.
        assert_eq!(c.state_machines.len(), 1);
        assert!(c.state_machines[0].has_default_state);
        assert_eq!(c.state_machines[0].child_state_count, 1);
    }
}
