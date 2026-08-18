//! VRChat **Expression Parameters** / **Expressions Menu** asset generation.
//!
//! Both assets are plain Unity `MonoBehaviour` ScriptableObjects (class id **114**) whose
//! `m_Script` points at the SDK script that implements them. Unlike the animator assets, they have
//! no internal fileID graph — a parameters asset is a flat `parameters` list and a menu is a flat
//! `controls` list — so generation is a straightforward single-document emit. The interesting
//! contract points are:
//!
//! - **The script reference.** `VRCExpressionParameters` / `VRCExpressionsMenu` are compiled into
//!   the SDK's `VRCSDK3A.dll` (GUID `67cc4cb7839cd3741b63733d5adf0442`), so an asset's `m_Script`
//!   is `{fileID: <class hash>, guid: <dll guid>, type: 3}` — the fileID is Unity's per-class
//!   hash inside the DLL, *not* the `11500000` a loose `.cs` script gets. The values here
//!   ([`VRC_EXPRESSION_PARAMETERS_SCRIPT`], [`VRC_EXPRESSIONS_MENU_SCRIPT`]) were read off the
//!   SDK's own `DefaultExpressionParameters.asset` / `DefaultExpressionsMenu.asset` in
//!   `com.vrchat.avatars` 3.10.4 and have been stable since SDK3 launched. The caller can still
//!   override both halves ([`ExpressionParams::script`]) so a future relocation is a flag, not a
//!   code change (`PLAN.md` risk 3).
//! - **The main-object fileID.** Unity's convention for a single-object ScriptableObject asset is
//!   `&11400000` ([`EXPRESSIONS_MAIN_FILE_ID`]); cross-asset references (`expressionsMenu:` on the
//!   descriptor, `subMenu:` on a control) expect it.
//! - **Menu control shape.** A control's driven parameter is a *nested* `parameter: {name}` map,
//!   puppet axes live in `subParameters`, and sub-menu nesting is the `subMenu` asset reference.
//!
//! The generated documents are validated by round-trip through `avatar-vrc-descriptor`'s
//! structural reader in tests (the same reader `avatar lint` trusts).

use crate::yaml_emit::{Emitter, ObjectRef, UNITY_PREAMBLE};

/// A MonoBehaviour `m_Script` reference: the class fileID plus the GUID of the asset that holds
/// it (a `.cs` file → `11500000`; a class inside a DLL → Unity's per-class hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptRef {
    pub file_id: i64,
    pub guid: String,
}

impl ScriptRef {
    /// A reference to a class compiled into a DLL (fileID = Unity's class hash).
    pub fn new(file_id: i64, guid: impl Into<String>) -> Self {
        ScriptRef {
            file_id,
            guid: guid.into(),
        }
    }
    /// A reference to a loose `.cs` script (Unity's fixed `11500000` main-object id).
    pub fn cs(guid: impl Into<String>) -> Self {
        ScriptRef::new(11500000, guid)
    }
    /// Render as Unity writes it: `{fileID: N, guid: G, type: 3}`.
    pub fn render(&self) -> String {
        format!("{{fileID: {}, guid: {}, type: 3}}", self.file_id, self.guid)
    }
}

/// GUID of the SDK's `VRCSDK3A.dll`, which holds the avatar-side runtime classes.
pub const VRCSDK3A_DLL_GUID: &str = "67cc4cb7839cd3741b63733d5adf0442";
/// `m_Script` reference of `VRC.SDK3.Avatars.ScriptableObjects.VRCExpressionParameters`.
pub const VRC_EXPRESSION_PARAMETERS_SCRIPT: (i64, &str) = (-1506855854, VRCSDK3A_DLL_GUID);
/// `m_Script` reference of `VRC.SDK3.Avatars.ScriptableObjects.VRCExpressionsMenu`.
pub const VRC_EXPRESSIONS_MENU_SCRIPT: (i64, &str) = (-340790334, VRCSDK3A_DLL_GUID);
/// Unity's conventional main-object fileID for a single-object ScriptableObject asset.
pub const EXPRESSIONS_MAIN_FILE_ID: i64 = 11400000;

/// An expression parameter's `valueType`: the raw values VRChat serializes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpressionValueType {
    /// `valueType: 0` — 8 sync bits.
    Int,
    /// `valueType: 1` — 8 sync bits.
    Float,
    /// `valueType: 2` — 1 sync bit.
    Bool,
}

impl ExpressionValueType {
    /// The raw serialized `valueType` value.
    pub fn raw(self) -> i64 {
        match self {
            ExpressionValueType::Int => 0,
            ExpressionValueType::Float => 1,
            ExpressionValueType::Bool => 2,
        }
    }

    /// Sync cost in bits (Bool 1; Int/Float 8) — for budget reporting before Unity is involved.
    pub fn cost_bits(self) -> u32 {
        match self {
            ExpressionValueType::Bool => 1,
            ExpressionValueType::Int | ExpressionValueType::Float => 8,
        }
    }
}

/// One entry in a generated `VRCExpressionParameters` asset.
#[derive(Debug, Clone)]
pub struct ExpressionParamSpec {
    pub name: String,
    pub value_type: ExpressionValueType,
    pub default_value: f32,
    /// Persist the value across avatar loads/worlds.
    pub saved: bool,
    /// Sync over the network (counts toward the 256-bit budget).
    pub synced: bool,
}

impl ExpressionParamSpec {
    fn new(name: impl Into<String>, value_type: ExpressionValueType) -> Self {
        ExpressionParamSpec {
            name: name.into(),
            value_type,
            default_value: 0.0,
            saved: true,
            synced: true,
        }
    }

    /// A saved, synced `Bool` parameter (the common toggle case).
    pub fn bool(name: impl Into<String>) -> Self {
        Self::new(name, ExpressionValueType::Bool)
    }

    /// A saved, synced `Int` parameter.
    pub fn int(name: impl Into<String>) -> Self {
        Self::new(name, ExpressionValueType::Int)
    }

    /// A saved, synced `Float` parameter.
    pub fn float(name: impl Into<String>) -> Self {
        Self::new(name, ExpressionValueType::Float)
    }

    /// Set the default value (builder-style).
    pub fn default_value(mut self, v: f32) -> Self {
        self.default_value = v;
        self
    }

    /// Set the saved flag (builder-style).
    pub fn saved(mut self, saved: bool) -> Self {
        self.saved = saved;
        self
    }

    /// Set the network-synced flag (builder-style).
    pub fn synced(mut self, synced: bool) -> Self {
        self.synced = synced;
        self
    }

    /// Bits this parameter consumes from the 256-bit sync budget (0 if not synced).
    pub fn sync_cost_bits(&self) -> u32 {
        if self.synced {
            self.value_type.cost_bits()
        } else {
            0
        }
    }
}

/// A generated `VRCExpressionParameters` asset.
#[derive(Debug, Clone)]
pub struct ExpressionParams {
    pub name: String,
    pub script: ScriptRef,
    pub parameters: Vec<ExpressionParamSpec>,
}

impl ExpressionParams {
    /// A new, empty parameters asset named `name`, pointing at the SDK's script reference.
    pub fn new(name: impl Into<String>) -> Self {
        ExpressionParams {
            name: name.into(),
            script: ScriptRef::new(
                VRC_EXPRESSION_PARAMETERS_SCRIPT.0,
                VRC_EXPRESSION_PARAMETERS_SCRIPT.1,
            ),
            parameters: Vec::new(),
        }
    }

    /// Override the `m_Script` reference (for a relocated / future SDK).
    pub fn script(mut self, script: ScriptRef) -> Self {
        self.script = script;
        self
    }

    /// Override only the `m_Script` GUID, treating it as a loose `.cs` script (`11500000`).
    pub fn script_guid(self, guid: impl Into<String>) -> Self {
        self.script(ScriptRef::cs(guid))
    }

    /// Add a parameter (builder-style).
    pub fn parameter(mut self, p: ExpressionParamSpec) -> Self {
        self.parameters.push(p);
        self
    }

    /// Total synced cost across all parameters, in bits (against the 256-bit budget).
    pub fn synced_bits(&self) -> u32 {
        self.parameters.iter().map(|p| p.sync_cost_bits()).sum()
    }

    /// Render the complete single-document asset stream (preamble + class-114 document).
    pub fn to_unity_yaml(&self, file_id: i64) -> String {
        let mut e = Emitter::new();
        e.doc_header(114, file_id);
        emit_monobehaviour_head(&mut e, &self.name, &self.script);
        e.indented(|e| {
            if self.parameters.is_empty() {
                e.kv("parameters", "[]");
            } else {
                e.key("parameters");
                for p in &self.parameters {
                    e.line(&format!("- name: {}", p.name));
                    e.indented(|e| {
                        e.kv_i64("valueType", p.value_type.raw());
                        e.kv_i64("saved", p.saved as i64);
                        e.kv_f32("defaultValue", p.default_value);
                        e.kv_i64("networkSynced", p.synced as i64);
                    });
                }
            }
        });
        format!("{UNITY_PREAMBLE}{}", e.into_string())
    }
}

/// VRChat menu control `type` values.
pub mod control_type {
    /// Sets the parameter while held, resets on release.
    pub const BUTTON: i64 = 101;
    /// Sets / clears the parameter on press.
    pub const TOGGLE: i64 = 102;
    /// Opens another `VRCExpressionsMenu` asset.
    pub const SUB_MENU: i64 = 103;
    /// Two-axis puppet (two float sub-parameters).
    pub const TWO_AXIS_PUPPET: i64 = 201;
    /// Four-axis puppet (four float sub-parameters).
    pub const FOUR_AXIS_PUPPET: i64 = 202;
    /// Radial puppet (one float sub-parameter).
    pub const RADIAL_PUPPET: i64 = 203;
}

/// One control in a generated `VRCExpressionsMenu` asset.
#[derive(Debug, Clone)]
pub struct MenuControlSpec {
    pub name: String,
    pub control_type: i64,
    /// The driven parameter (`parameter: {name}`); empty for sub-menus and pure puppets.
    pub parameter: Option<String>,
    /// The value the parameter is set to (toggles/buttons; ignored otherwise).
    pub value: f32,
    /// Puppet axis parameters (`subParameters`), e.g. the radial's single float.
    pub sub_parameters: Vec<String>,
    /// The sub-menu asset reference, for [`control_type::SUB_MENU`].
    pub sub_menu: Option<ObjectRef>,
}

impl MenuControlSpec {
    fn new(name: impl Into<String>, control_type: i64) -> Self {
        MenuControlSpec {
            name: name.into(),
            control_type,
            parameter: None,
            value: 1.0,
            sub_parameters: Vec::new(),
            sub_menu: None,
        }
    }

    /// A toggle control driving `parameter` (set to `value`, default 1).
    pub fn toggle(name: impl Into<String>, parameter: impl Into<String>) -> Self {
        let mut c = Self::new(name, control_type::TOGGLE);
        c.parameter = Some(parameter.into());
        c
    }

    /// A momentary button driving `parameter` (set to `value` while held, default 1).
    pub fn button(name: impl Into<String>, parameter: impl Into<String>) -> Self {
        let mut c = Self::new(name, control_type::BUTTON);
        c.parameter = Some(parameter.into());
        c
    }

    /// A sub-menu control opening the menu asset at `sub_menu` (typically
    /// `ObjectRef::external(EXPRESSIONS_MAIN_FILE_ID, guid, 2)`).
    pub fn sub_menu(name: impl Into<String>, sub_menu: ObjectRef) -> Self {
        let mut c = Self::new(name, control_type::SUB_MENU);
        c.sub_menu = Some(sub_menu);
        c
    }

    /// A radial puppet driving the float `parameter` (as its single `subParameters` axis).
    pub fn radial(name: impl Into<String>, parameter: impl Into<String>) -> Self {
        let mut c = Self::new(name, control_type::RADIAL_PUPPET);
        c.sub_parameters.push(parameter.into());
        c
    }

    /// Set the driven value (builder-style; toggles/buttons).
    pub fn value(mut self, v: f32) -> Self {
        self.value = v;
        self
    }
}

/// A generated `VRCExpressionsMenu` asset.
#[derive(Debug, Clone)]
pub struct ExpressionsMenu {
    pub name: String,
    pub script: ScriptRef,
    pub controls: Vec<MenuControlSpec>,
}

impl ExpressionsMenu {
    /// A new, empty menu asset named `name`, pointing at the SDK's script reference.
    pub fn new(name: impl Into<String>) -> Self {
        ExpressionsMenu {
            name: name.into(),
            script: ScriptRef::new(VRC_EXPRESSIONS_MENU_SCRIPT.0, VRC_EXPRESSIONS_MENU_SCRIPT.1),
            controls: Vec::new(),
        }
    }

    /// Override the `m_Script` reference (for a relocated / future SDK).
    pub fn script(mut self, script: ScriptRef) -> Self {
        self.script = script;
        self
    }

    /// Override only the `m_Script` GUID, treating it as a loose `.cs` script (`11500000`).
    pub fn script_guid(self, guid: impl Into<String>) -> Self {
        self.script(ScriptRef::cs(guid))
    }

    /// Add a control (builder-style).
    pub fn control(mut self, c: MenuControlSpec) -> Self {
        self.controls.push(c);
        self
    }

    /// Render the complete single-document asset stream (preamble + class-114 document).
    pub fn to_unity_yaml(&self, file_id: i64) -> String {
        let mut e = Emitter::new();
        e.doc_header(114, file_id);
        emit_monobehaviour_head(&mut e, &self.name, &self.script);
        e.indented(|e| {
            if self.controls.is_empty() {
                e.kv("controls", "[]");
            } else {
                e.key("controls");
                for c in &self.controls {
                    e.line(&format!("- name: {}", c.name));
                    e.indented(|e| {
                        e.kv("icon", "{fileID: 0}");
                        e.kv_i64("type", c.control_type);
                        e.key("parameter");
                        e.indented(|e| {
                            match &c.parameter {
                                Some(p) => e.kv("name", p),
                                None => e.kv("name", "''"),
                            };
                        });
                        e.kv_f32("value", c.value);
                        e.kv_ref("subMenu", c.sub_menu.as_ref().unwrap_or(&ObjectRef::null()));
                        if c.sub_parameters.is_empty() {
                            e.kv("subParameters", "[]");
                        } else {
                            e.key("subParameters");
                            for p in &c.sub_parameters {
                                e.line(&format!("- name: {p}"));
                            }
                        }
                        e.kv("labels", "[]");
                    });
                }
            }
        });
        format!("{UNITY_PREAMBLE}{}", e.into_string())
    }
}

/// The shared `MonoBehaviour:` header fields both assets open with (through
/// `m_EditorClassIdentifier`), matching Unity's serialization order byte-for-byte.
fn emit_monobehaviour_head(e: &mut Emitter, name: &str, script: &ScriptRef) {
    e.line("MonoBehaviour:");
    e.indented(|e| {
        e.kv("m_ObjectHideFlags", "0");
        e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
        e.kv("m_PrefabInstance", "{fileID: 0}");
        e.kv("m_PrefabAsset", "{fileID: 0}");
        e.kv("m_GameObject", "{fileID: 0}");
        e.kv("m_Enabled", "1");
        e.kv("m_EditorHideFlags", "0");
        e.kv("m_Script", &script.render());
        e.kv("m_Name", name);
        e.key("m_EditorClassIdentifier");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_unity_yaml::UnityFile;
    use avatar_vrc_descriptor::{VrcAsset, extract};

    fn toggle_params() -> ExpressionParams {
        ExpressionParams::new("Parameters")
            .parameter(ExpressionParamSpec::bool("Hat"))
            .parameter(
                ExpressionParamSpec::float("Dimmer")
                    .saved(false)
                    .synced(false),
            )
            .parameter(ExpressionParamSpec::int("Outfit").default_value(2.0))
    }

    #[test]
    fn params_emit_expected_fields() {
        let yaml = toggle_params().to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        assert!(yaml.contains("--- !u!114 &11400000"));
        assert!(yaml.contains(
            "m_Script: {fileID: -1506855854, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}"
        ));
        assert!(yaml.contains("m_Name: Parameters"));
        assert!(yaml.contains("- name: Hat"));
        assert!(yaml.contains("valueType: 2"));
        assert!(yaml.contains("networkSynced: 0"));
        assert!(yaml.contains("defaultValue: 2"));
    }

    #[test]
    fn params_roundtrip_through_vrc_descriptor_reader() {
        let yaml = toggle_params().to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        let file = UnityFile::parse(&yaml).expect("generated params must parse");
        let assets = extract(&file);
        assert_eq!(assets.len(), 1);
        let VrcAsset::Parameters(p) = &assets[0] else {
            panic!("expected the structural reader to classify a Parameters asset");
        };
        assert_eq!(p.asset_name.as_deref(), Some("Parameters"));
        assert_eq!(p.script_guid.as_deref(), Some(VRCSDK3A_DLL_GUID));
        assert_eq!(p.parameters.len(), 3);
        // Bool(1) + Int(8) synced; the local Float contributes 0.
        assert_eq!(p.synced_bits(), 9);
        assert_eq!(p.synced_bits(), toggle_params().synced_bits());
        assert!(p.parameters[0].saved);
        assert!(!p.parameters[1].synced);
    }

    #[test]
    fn menu_emits_and_roundtrips() {
        let menu = ExpressionsMenu::new("Menu")
            .control(MenuControlSpec::toggle("Hat", "Hat"))
            .control(MenuControlSpec::radial("Dimmer", "Dimmer"))
            .control(MenuControlSpec::sub_menu(
                "More",
                ObjectRef::external(
                    EXPRESSIONS_MAIN_FILE_ID,
                    "abcdefabcdefabcdefabcdefabcdefab",
                    2,
                ),
            ));
        let yaml = menu.to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        assert!(yaml.contains("type: 102"));
        assert!(yaml.contains("type: 203"));
        assert!(yaml.contains(
            "subMenu: {fileID: 11400000, guid: abcdefabcdefabcdefabcdefabcdefab, type: 2}"
        ));

        let file = UnityFile::parse(&yaml).expect("generated menu must parse");
        let assets = extract(&file);
        let VrcAsset::Menu(m) = &assets[0] else {
            panic!("expected the structural reader to classify a Menu asset");
        };
        assert_eq!(m.controls.len(), 3);
        assert_eq!(m.controls[0].parameter.as_deref(), Some("Hat"));
        assert_eq!(m.controls[1].sub_parameters, vec!["Dimmer"]);
        // The empty-name parameter on the sub-menu control reads back as None.
        assert_eq!(m.controls[2].parameter, None);
        assert!(m.controls[2].has_submenu);
        assert!(!m.controls[0].has_submenu);
    }

    #[test]
    fn empty_assets_emit_empty_collections() {
        let p = ExpressionParams::new("Empty").to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        assert!(p.contains("parameters: []"));
        let m = ExpressionsMenu::new("Empty").to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        assert!(m.contains("controls: []"));
        // Both still parse.
        UnityFile::parse(&p).unwrap();
        UnityFile::parse(&m).unwrap();
    }

    #[test]
    fn generation_is_deterministic() {
        let a = toggle_params().to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        let b = toggle_params().to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID);
        assert_eq!(a, b);
    }
}
