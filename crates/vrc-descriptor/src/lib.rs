//! Typed extraction of the VRChat SDK3 avatar data assets that carry the most well-defined
//! rules: **Expression Parameters** (the 256-bit sync budget) and **Expression Menus** (the
//! 8-control limit, sub-menu nesting).
//!
//! VRChat assets are identified *structurally* (by the shape of the serialized fields) rather
//! than by a hardcoded script GUID, so this keeps working across SDK versions. The `m_Script`
//! GUID is still captured for reference.
//!
//! References:
//! - <https://creators.vrchat.com/avatars/animator-parameters/>
//! - <https://creators.vrchat.com/avatars/expression-menu-and-controls/>

use avatar_unity_yaml::{
    UnityDocument, UnityFile, Yaml, field_bool, field_f64, field_i64, field_str,
};
use serde::Serialize;

/// Total sync memory available to an avatar's synced expression parameters, in bits.
pub const SYNC_BUDGET_BITS: u32 = 256;
/// Maximum number of controls in a single expression menu.
pub const MAX_MENU_CONTROLS: usize = 8;

/// An expression parameter's value type. Serialized as `valueType`: `Int=0, Float=1, Bool=2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ValueType {
    Int,
    Float,
    Bool,
}

impl ValueType {
    pub fn from_raw(v: i64) -> Option<Self> {
        match v {
            0 => Some(ValueType::Int),
            1 => Some(ValueType::Float),
            2 => Some(ValueType::Bool),
            _ => None,
        }
    }

    /// Sync cost in bits: Bool is 1 bit, Int and Float are 8 bits each.
    pub fn cost_bits(self) -> u32 {
        match self {
            ValueType::Bool => 1,
            ValueType::Int | ValueType::Float => 8,
        }
    }
}

/// A single custom expression parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ExpressionParameter {
    pub name: String,
    pub value_type: ValueType,
    pub default_value: f64,
    pub saved: bool,
    /// Whether the parameter is synced over the network (counts toward the budget).
    pub synced: bool,
}

impl ExpressionParameter {
    /// Bits this parameter consumes from the sync budget (0 if not synced).
    pub fn sync_cost_bits(&self) -> u32 {
        if self.synced {
            self.value_type.cost_bits()
        } else {
            0
        }
    }
}

/// A parsed VRCExpressionParameters asset.
#[derive(Debug, Clone, Serialize)]
pub struct ExpressionParameters {
    pub asset_name: Option<String>,
    pub script_guid: Option<String>,
    pub parameters: Vec<ExpressionParameter>,
}

impl ExpressionParameters {
    /// Total synced cost across all parameters, in bits.
    pub fn synced_bits(&self) -> u32 {
        self.parameters.iter().map(|p| p.sync_cost_bits()).sum()
    }

    fn from_doc(doc: &UnityDocument) -> Option<Self> {
        let list = doc.body["parameters"].as_vec()?;
        // Guard against false positives: a real expression-parameters list either is empty or has
        // entries carrying a `valueType`.
        if let Some(first) = list.first()
            && first["valueType"].is_badvalue()
            && first["name"].is_badvalue()
        {
            return None;
        }
        let parameters = list.iter().map(parse_parameter).collect::<Vec<_>>();
        Some(ExpressionParameters {
            asset_name: doc.name().map(str::to_string),
            script_guid: doc.script_guid().map(str::to_string),
            parameters,
        })
    }
}

fn parse_parameter(node: &Yaml) -> ExpressionParameter {
    let value_type = field_i64(node, "valueType")
        .and_then(ValueType::from_raw)
        .unwrap_or(ValueType::Int);
    // Current SDK serializes the sync flag as `networkSynced`; older assets omit it (synced).
    let synced = field_bool(node, "networkSynced")
        .or_else(|| field_bool(node, "synced"))
        .unwrap_or(true);
    ExpressionParameter {
        name: field_str(node, "name").unwrap_or_default().to_string(),
        value_type,
        default_value: field_f64(node, "defaultValue").unwrap_or(0.0),
        saved: field_bool(node, "saved").unwrap_or(false),
        synced,
    }
}

/// A single expression menu control.
#[derive(Debug, Clone, Serialize)]
pub struct MenuControl {
    pub name: String,
    /// The raw `type` field. VRChat's enum values vary by SDK; we surface the raw value and use
    /// the `subMenu` reference to detect nesting rather than depending on the enum.
    pub control_type: i64,
    /// The driven parameter name, if any.
    pub parameter: Option<String>,
    /// Additional parameter names for puppet controls (`subParameters`).
    pub sub_parameters: Vec<String>,
    /// Whether this control points at a sub-menu asset.
    pub has_submenu: bool,
}

/// A parsed VRCExpressionsMenu asset.
#[derive(Debug, Clone, Serialize)]
pub struct ExpressionsMenu {
    pub asset_name: Option<String>,
    pub script_guid: Option<String>,
    pub controls: Vec<MenuControl>,
}

impl ExpressionsMenu {
    fn from_doc(doc: &UnityDocument) -> Option<Self> {
        let list = doc.body["controls"].as_vec()?;
        // Distinguish from an unrelated `controls` field: menu controls carry a `type`.
        if let Some(first) = list.first()
            && first["type"].is_badvalue()
            && first["parameter"].is_badvalue()
        {
            return None;
        }
        let controls = list.iter().map(parse_control).collect();
        Some(ExpressionsMenu {
            asset_name: doc.name().map(str::to_string),
            script_guid: doc.script_guid().map(str::to_string),
            controls,
        })
    }
}

fn parse_control(node: &Yaml) -> MenuControl {
    let parameter = node["parameter"]["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let sub_parameters = node["subParameters"]
        .as_vec()
        .map(|v| {
            v.iter()
                .filter_map(|p| p["name"].as_str().filter(|s| !s.is_empty()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // A sub-menu reference is a non-zero fileID (and usually a guid).
    let sub = &node["subMenu"];
    let has_submenu =
        sub["guid"].as_str().is_some() || field_i64(sub, "fileID").is_some_and(|id| id != 0);

    MenuControl {
        name: field_str(node, "name").unwrap_or_default().to_string(),
        control_type: field_i64(node, "type").unwrap_or(0),
        parameter,
        sub_parameters,
        has_submenu,
    }
}

/// A reference to another Unity object or asset: `{fileID, guid, type}`.
///
/// A reference *within the same file* carries only a `fileID`; a *cross-asset* reference also
/// carries a `guid`, which resolves against the project's `.meta` files.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AssetRef {
    pub file_id: i64,
    pub guid: Option<String>,
}

impl AssetRef {
    /// Parse a `{fileID:.., guid:.., type:..}` mapping. Missing fields default to null/zero.
    pub fn parse(node: &Yaml) -> Self {
        AssetRef {
            file_id: field_i64(node, "fileID").unwrap_or(0),
            guid: node["guid"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        }
    }

    /// True if this reference points at something (a non-zero local id or an external guid). A
    /// bare `{fileID: 0}` is Unity's null reference.
    pub fn is_set(&self) -> bool {
        self.file_id != 0 || self.guid.is_some()
    }

    /// True if this references another asset file (i.e. carries a guid).
    pub fn is_external(&self) -> bool {
        self.guid.is_some()
    }
}

/// One entry in an avatar's playable-animation-layer list.
#[derive(Debug, Clone, Serialize)]
pub struct AnimationLayer {
    /// Raw `type` index — VRChat's `AnimLayerType`: 0 Base, 1 Additive, 2 Gesture, 3 Action,
    /// 4 FX, 5 Sitting, 6 TPose, 7 IKPose.
    pub layer_type: i64,
    /// `true` if the layer uses VRChat's built-in default controller.
    pub is_default: bool,
    pub is_enabled: bool,
    /// The custom animator controller assigned to this layer (null when default).
    pub controller: AssetRef,
}

impl AnimationLayer {
    /// Human-readable name of the layer type.
    pub fn type_name(&self) -> &'static str {
        match self.layer_type {
            0 => "Base",
            1 => "Additive",
            2 => "Gesture",
            3 => "Action",
            4 => "FX",
            5 => "Sitting",
            6 => "TPose",
            7 => "IKPose",
            _ => "Unknown",
        }
    }
}

/// The `lipSync` value for blend-shape visemes (VRChat `LipSyncStyle::VisemeBlendShape`).
pub const LIPSYNC_VISEME_BLENDSHAPE: i64 = 3;
/// The number of visemes VRChat expects when using blend-shape lip-sync.
pub const VISEME_COUNT: usize = 15;
/// The `eyelidType` value for blend-shape eyelids (VRChat `EyelidType::Blendshapes`).
pub const EYELID_TYPE_BLENDSHAPES: i64 = 1;

/// The avatar's eye-look configuration (`customEyeLookSettings`).
#[derive(Debug, Clone, Default, Serialize)]
pub struct EyeLookSettings {
    /// The left eye bone (a Transform reference within the avatar).
    pub left_eye: AssetRef,
    /// The right eye bone.
    pub right_eye: AssetRef,
    /// `eyelidType`: 0 None, 1 Blendshapes, 2 Bones.
    pub eyelid_type: i64,
    /// The skinned mesh holding eyelid blend shapes (`eyelidsSkinnedMesh`).
    pub eyelids_mesh: AssetRef,
}

impl EyeLookSettings {
    fn parse(node: &Yaml) -> Self {
        EyeLookSettings {
            left_eye: AssetRef::parse(&node["leftEye"]),
            right_eye: AssetRef::parse(&node["rightEye"]),
            eyelid_type: field_i64(node, "eyelidType").unwrap_or(0),
            eyelids_mesh: AssetRef::parse(&node["eyelidsSkinnedMesh"]),
        }
    }

    /// True if at least one eye bone is assigned.
    pub fn has_eye_bones(&self) -> bool {
        self.left_eye.is_set() || self.right_eye.is_set()
    }

    /// True if eyelids are driven by blend shapes (`eyelidType == 1`).
    pub fn uses_eyelid_blendshapes(&self) -> bool {
        self.eyelid_type == EYELID_TYPE_BLENDSHAPES
    }
}

/// A parsed VRChat **Avatar Descriptor** (`VRCAvatarDescriptor`) — the component that turns a
/// GameObject into an uploadable avatar. Lives in a scene (`.unity`) or prefab (`.prefab`).
#[derive(Debug, Clone, Serialize)]
pub struct AvatarDescriptor {
    pub asset_name: Option<String>,
    pub script_guid: Option<String>,
    /// First-person view / eye position `[x, y, z]`, if present.
    pub view_position: Option<[f64; 3]>,
    /// Raw `lipSync` mode.
    pub lip_sync_mode: i64,
    /// Names listed in `VisemeBlendShapes`.
    pub viseme_blendshapes: Vec<String>,
    /// The skinned mesh assigned for visemes (`VisemeSkinnedMesh`).
    pub viseme_mesh: AssetRef,
    pub enable_eye_look: bool,
    /// Eye-look configuration (`customEyeLookSettings`); only meaningful when `enable_eye_look`.
    pub eye_look: EyeLookSettings,
    /// `customExpressions`: whether the avatar uses custom Expression Menu/Parameters assets.
    pub custom_expressions: bool,
    pub expressions_menu: AssetRef,
    pub expression_parameters: AssetRef,
    /// Base playable layers (Base/Additive/Gesture/Action/FX).
    pub base_animation_layers: Vec<AnimationLayer>,
    /// Special playable layers (Sitting/TPose/IKPose).
    pub special_animation_layers: Vec<AnimationLayer>,
}

impl AvatarDescriptor {
    /// `lipSync` is set to viseme-blend-shape mode.
    pub fn uses_viseme_blendshapes(&self) -> bool {
        self.lip_sync_mode == LIPSYNC_VISEME_BLENDSHAPE
    }

    /// All playable layers (base + special), in order.
    pub fn animation_layers(&self) -> impl Iterator<Item = &AnimationLayer> {
        self.base_animation_layers
            .iter()
            .chain(self.special_animation_layers.iter())
    }

    fn from_doc(doc: &UnityDocument) -> Option<Self> {
        let body = &doc.body;
        // Discriminate the VRCAvatarDescriptor by two of its distinctive fields.
        if body["baseAnimationLayers"].is_badvalue() || body["ViewPosition"].is_badvalue() {
            return None;
        }

        let view_position = {
            let v = &body["ViewPosition"];
            match (field_f64(v, "x"), field_f64(v, "y"), field_f64(v, "z")) {
                (Some(x), Some(y), Some(z)) => Some([x, y, z]),
                _ => None,
            }
        };

        let viseme_blendshapes = body["VisemeBlendShapes"]
            .as_vec()
            .map(|v| {
                v.iter()
                    .filter_map(|s| s.as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        Some(AvatarDescriptor {
            asset_name: doc.name().map(str::to_string),
            script_guid: doc.script_guid().map(str::to_string),
            view_position,
            lip_sync_mode: field_i64(body, "lipSync").unwrap_or(0),
            viseme_blendshapes,
            viseme_mesh: AssetRef::parse(&body["VisemeSkinnedMesh"]),
            enable_eye_look: field_bool(body, "enableEyeLook").unwrap_or(false),
            eye_look: EyeLookSettings::parse(&body["customEyeLookSettings"]),
            custom_expressions: field_bool(body, "customExpressions").unwrap_or(false),
            expressions_menu: AssetRef::parse(&body["expressionsMenu"]),
            expression_parameters: AssetRef::parse(&body["expressionParameters"]),
            base_animation_layers: parse_layers(&body["baseAnimationLayers"]),
            special_animation_layers: parse_layers(&body["specialAnimationLayers"]),
        })
    }
}

fn parse_layers(node: &Yaml) -> Vec<AnimationLayer> {
    node.as_vec()
        .map(|v| {
            v.iter()
                .map(|l| AnimationLayer {
                    layer_type: field_i64(l, "type").unwrap_or(-1),
                    is_default: field_bool(l, "isDefault").unwrap_or(false),
                    is_enabled: field_bool(l, "isEnabled").unwrap_or(false),
                    controller: AssetRef::parse(&l["animatorController"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A VRChat asset recognized within a Unity YAML file.
#[derive(Debug, Clone, Serialize)]
pub enum VrcAsset {
    Parameters(ExpressionParameters),
    Menu(ExpressionsMenu),
    /// Boxed because an `AvatarDescriptor` is much larger than the other variants.
    Descriptor(Box<AvatarDescriptor>),
}

/// Classify a single Unity document as a known VRChat asset, if it is one.
pub fn classify(doc: &UnityDocument) -> Option<VrcAsset> {
    if !doc.is_monobehaviour() {
        return None;
    }
    // The three shapes are mutually exclusive (distinct top-level fields), so order is only a
    // micro-optimization: check menu first (its `controls`/`parameter` mapping is distinctive),
    // then parameters, then the descriptor.
    if let Some(menu) = ExpressionsMenu::from_doc(doc) {
        return Some(VrcAsset::Menu(menu));
    }
    if let Some(params) = ExpressionParameters::from_doc(doc) {
        return Some(VrcAsset::Parameters(params));
    }
    if let Some(desc) = AvatarDescriptor::from_doc(doc) {
        return Some(VrcAsset::Descriptor(Box::new(desc)));
    }
    None
}

/// Extract every recognized VRChat asset from a parsed Unity file.
pub fn extract(file: &UnityFile) -> Vec<VrcAsset> {
    file.documents.iter().filter_map(classify).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: &str = "\
--- !u!114 &11400000
MonoBehaviour:
  m_Name: Parameters
  parameters:
  - name: VRCEmote
    valueType: 0
    saved: 1
    defaultValue: 0
    networkSynced: 1
  - name: Toggle
    valueType: 2
    saved: 0
    defaultValue: 0
    networkSynced: 1
  - name: LocalFloat
    valueType: 1
    saved: 0
    defaultValue: 0
    networkSynced: 0
";

    const MENU: &str = "\
--- !u!114 &11400000
MonoBehaviour:
  m_Name: Menu
  controls:
  - name: Emotes
    type: 103
    parameter:
      name: ''
    subMenu: {fileID: 11400000, guid: deadbeefdeadbeefdeadbeefdeadbeef, type: 2}
    subParameters: []
  - name: Dance
    type: 102
    parameter:
      name: VRCEmote
    value: 1
    subMenu: {fileID: 0}
    subParameters: []
";

    #[test]
    fn parses_parameters_and_budget() {
        let file = UnityFile::parse(PARAMS).unwrap();
        let assets = extract(&file);
        assert_eq!(assets.len(), 1);
        let VrcAsset::Parameters(p) = &assets[0] else {
            panic!("expected parameters");
        };
        assert_eq!(p.parameters.len(), 3);
        // Int(8, synced) + Bool(1, synced) + Float(8, NOT synced) = 9 bits synced.
        assert_eq!(p.synced_bits(), 9);
        assert_eq!(p.parameters[2].value_type, ValueType::Float);
        assert!(!p.parameters[2].synced);
    }

    #[test]
    fn parses_menu_controls_and_submenu() {
        let file = UnityFile::parse(MENU).unwrap();
        let assets = extract(&file);
        let VrcAsset::Menu(m) = &assets[0] else {
            panic!("expected menu");
        };
        assert_eq!(m.controls.len(), 2);
        assert!(m.controls[0].has_submenu);
        assert_eq!(m.controls[0].parameter, None);
        assert!(!m.controls[1].has_submenu);
        assert_eq!(m.controls[1].parameter.as_deref(), Some("VRCEmote"));
    }

    const DESCRIPTOR: &str = "\
--- !u!1 &100000
GameObject:
  m_Name: Avatar
  m_Component:
  - component: {fileID: 114000}
--- !u!114 &114000
MonoBehaviour:
  m_GameObject: {fileID: 100000}
  m_Script: {fileID: 11500000, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}
  ViewPosition: {x: 0, y: 1.2, z: 0.15}
  lipSync: 3
  VisemeSkinnedMesh: {fileID: 0}
  VisemeBlendShapes:
  - vrc.v_sil
  - vrc.v_pp
  enableEyeLook: 1
  customExpressions: 1
  expressionsMenu: {fileID: 11400000, guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, type: 2}
  expressionParameters: {fileID: 11400000, guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, type: 2}
  baseAnimationLayers:
  - isEnabled: 1
    type: 4
    animatorController: {fileID: 9100000, guid: dddddddddddddddddddddddddddddddd, type: 2}
    isDefault: 0
  - isEnabled: 0
    type: 2
    animatorController: {fileID: 0}
    isDefault: 1
  specialAnimationLayers:
  - isEnabled: 0
    type: 6
    animatorController: {fileID: 0}
    isDefault: 1
";

    #[test]
    fn parses_avatar_descriptor() {
        let file = UnityFile::parse(DESCRIPTOR).unwrap();
        let assets = extract(&file);
        // Only the descriptor MonoBehaviour is recognized; the GameObject is ignored.
        assert_eq!(assets.len(), 1);
        let VrcAsset::Descriptor(d) = &assets[0] else {
            panic!("expected descriptor");
        };
        assert_eq!(d.view_position, Some([0.0, 1.2, 0.15]));
        assert!(d.custom_expressions);
        assert!(d.uses_viseme_blendshapes());
        assert!(!d.viseme_mesh.is_set());
        assert_eq!(d.viseme_blendshapes.len(), 2);
        assert_eq!(
            d.expression_parameters.guid.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            d.expressions_menu.guid.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );

        // FX layer is custom + external (a real guid); Gesture/TPose are defaults.
        let fx = d
            .base_animation_layers
            .iter()
            .find(|l| l.layer_type == 4)
            .unwrap();
        assert!(!fx.is_default);
        assert!(fx.controller.is_external());
        let externals: Vec<_> = d
            .animation_layers()
            .filter(|l| !l.is_default && l.controller.is_external())
            .collect();
        assert_eq!(externals.len(), 1);
    }
}
