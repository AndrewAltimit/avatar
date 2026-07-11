//! End-to-end **toggle bundle** generation — the composite that closes the authoring loop.
//!
//! A working in-game toggle needs five cooperating assets: an *On* clip, an *Off* clip, an FX
//! `AnimatorController` layer that switches between them on a `Bool` animator parameter, a
//! `VRCExpressionParameters` entry declaring that parameter, and a `VRCExpressionsMenu` control
//! driving it. Each generator existed separately; this module assembles all five as one
//! internally-consistent bundle.
//!
//! The consistency problem is **GUIDs**: the controller's states reference the clips by asset
//! GUID, which Unity normally mints at import time. We sidestep import order by emitting a
//! `.meta` sidecar for every generated file with a **deterministic GUID** derived from the bundle
//! name ([`deterministic_guid`]) — Unity adopts an existing `.meta`'s GUID instead of minting one,
//! so the cross-references hold on first import. (Same determinism rationale as [`IdGen`]:
//! byte-identical re-runs, diffable output.)
//!
//! The toggle layer itself is the standard two-state machine VRChat authors build by hand:
//! `Off` (default) and `On` states playing the respective clips, with instant (`m_HasExitTime: 0`,
//! duration 0) transitions both ways conditioned on the `Bool` parameter (`If` → On,
//! `IfNot` → Off).

use crate::IdGen;
use crate::clip::{AnimationClip, FloatCurve, Keyframe};
use crate::controller::{AnimatorController, AnimatorParameter};
use crate::expressions::{
    EXPRESSIONS_MAIN_FILE_ID, ExpressionParamSpec, ExpressionParams, ExpressionsMenu,
    MenuControlSpec,
};
use crate::yaml_emit::{Emitter, ObjectRef, UNITY_PREAMBLE};

/// `m_ConditionMode` on an `AnimatorCondition`: parameter is true.
pub const CONDITION_IF: i64 = 1;
/// `m_ConditionMode`: parameter is false.
pub const CONDITION_IF_NOT: i64 = 2;

/// Derive a stable 32-hex-char Unity GUID from `seed`.
///
/// Two FNV-1a passes (the second salted) give 128 bits; the first character is then forced into
/// `a`–`f`. That guarantees the GUID contains a letter — an all-digit "GUID" is parsed by YAML
/// readers (including `yaml-rust2`) as a *number*, which silently breaks GUID resolution (see
/// `CLAUDE.md` gotchas).
pub fn deterministic_guid(seed: &str) -> String {
    let a = avatar_unity_yaml::fnv1a(seed.as_bytes());
    let b = avatar_unity_yaml::fnv1a(format!("{seed}\u{1}guid").as_bytes());
    let hex = format!("{a:016x}{b:016x}");
    let first = char::from(b'a' + (a % 6) as u8);
    format!("{first}{}", &hex[1..])
}

/// Render a `.meta` sidecar for a generated native-format asset (`.anim`, `.controller`,
/// `.asset`), pinning its `guid` and main-object fileID so cross-asset references generated
/// alongside it resolve on first import.
pub fn native_asset_meta(guid: &str, main_object_file_id: i64) -> String {
    format!(
        "fileFormatVersion: 2\n\
         guid: {guid}\n\
         NativeFormatImporter:\n\
        \x20 externalObjects: {{}}\n\
        \x20 mainObjectFileID: {main_object_file_id}\n\
        \x20 userData:\n\
        \x20 assetBundleName:\n\
        \x20 assetBundleVariant:\n"
    )
}

/// What a toggle drives: a GameObject's active flag, or a blendshape weight.
#[derive(Debug, Clone)]
pub enum ToggleTarget {
    /// Animate `m_IsActive` on the GameObject at this hierarchy path (1 when on, 0 when off).
    GameObject { path: String },
    /// Animate `blendShape.<shape>` on the mesh at `path` (`on_value` when on, 0 when off).
    Blendshape {
        path: String,
        shape: String,
        on_value: f32,
    },
}

/// The input to [`generate_toggle`]: one named toggle over one or more targets.
#[derive(Debug, Clone)]
pub struct ToggleSpec {
    /// Bundle name — seeds every derived name, fileID range, and GUID (e.g. `Hat`).
    pub name: String,
    /// The `Bool` animator + expression parameter (defaults to `name` in the CLI).
    pub parameter: String,
    pub targets: Vec<ToggleTarget>,
    /// `saved` on the expression parameter.
    pub saved: bool,
    /// Start on: the expression default is 1 and the *On* state is the layer default.
    pub default_on: bool,
    /// Label on the generated menu control (defaults to `name` in the CLI).
    pub menu_label: String,
}

/// One generated file of a bundle: its suggested file name and full text content.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GeneratedFile {
    pub file_name: String,
    pub content: String,
}

/// The full output of [`generate_toggle`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToggleBundle {
    /// Every file to write, `.meta` sidecars included, in write order.
    pub files: Vec<GeneratedFile>,
    /// The `Bool` parameter the bundle is wired on.
    pub parameter: String,
    /// GUID pinned for the FX controller (what the descriptor's FX layer should reference).
    pub controller_guid: String,
    /// GUID pinned for the parameters asset.
    pub params_guid: String,
    /// GUID pinned for the menu asset.
    pub menu_guid: String,
    /// Sync bits the new parameter consumes from the avatar's 256-bit budget.
    pub sync_bits: u32,
    /// How to wire the bundle into an avatar.
    pub wiring_note: String,
}

/// Assemble the five-asset toggle bundle (plus `.meta` sidecars) for `spec`.
pub fn generate_toggle(spec: &ToggleSpec) -> ToggleBundle {
    let on_name = format!("{}_On", spec.name);
    let off_name = format!("{}_Off", spec.name);
    let controller_name = format!("{}_FX", spec.name);
    let params_name = format!("{}_Params", spec.name);
    let menu_name = format!("{}_Menu", spec.name);

    // Clips: every target held at its on-value in the On clip and at 0 in the Off clip. The Off
    // clip animates the same properties so the toggle is authoritative both ways under VRChat's
    // recommended Write Defaults OFF (nothing else has to restore the state).
    let mut on_clip = AnimationClip::new(&on_name);
    let mut off_clip = AnimationClip::new(&off_name);
    for t in &spec.targets {
        match t {
            ToggleTarget::GameObject { path } => {
                on_clip.add_float_curve(FloatCurve::game_object_active(
                    path.clone(),
                    vec![Keyframe::flat(0.0, 1.0)],
                ));
                off_clip.add_float_curve(FloatCurve::game_object_active(
                    path.clone(),
                    vec![Keyframe::flat(0.0, 0.0)],
                ));
            }
            ToggleTarget::Blendshape {
                path,
                shape,
                on_value,
            } => {
                on_clip.add_float_curve(FloatCurve::blendshape(
                    path.clone(),
                    shape,
                    vec![Keyframe::flat(0.0, *on_value)],
                ));
                off_clip.add_float_curve(FloatCurve::blendshape(
                    path.clone(),
                    shape,
                    vec![Keyframe::flat(0.0, 0.0)],
                ));
            }
        }
    }

    let mut on_ids = IdGen::new(&on_name);
    let on_clip_id = on_ids.alloc();
    let mut off_ids = IdGen::new(&off_name);
    let off_clip_id = off_ids.alloc();

    let on_guid = deterministic_guid(&format!("{}/on", spec.name));
    let off_guid = deterministic_guid(&format!("{}/off", spec.name));
    let controller_guid = deterministic_guid(&format!("{}/controller", spec.name));
    let params_guid = deterministic_guid(&format!("{}/params", spec.name));
    let menu_guid = deterministic_guid(&format!("{}/menu", spec.name));

    // Controller: a Bool parameter + the two-state toggle layer referencing the clips externally.
    let mut ids = IdGen::new(&controller_name);
    let controller_id = ids.alloc();
    let (fragment, sm_id) = toggle_state_fragment(
        &spec.name,
        &spec.parameter,
        &ObjectRef::external(on_clip_id, &on_guid, 2),
        &ObjectRef::external(off_clip_id, &off_guid, 2),
        spec.default_on,
        &mut ids,
    );
    let controller = AnimatorController::new(&controller_name)
        .parameter(AnimatorParameter::bool(&spec.parameter))
        .layer(&spec.name, sm_id);
    let mut e = Emitter::new();
    controller.emit_controller(&mut e, controller_id);
    let controller_yaml = format!("{UNITY_PREAMBLE}{}{fragment}", e.into_string());

    // Expression parameters + menu.
    let param_spec = ExpressionParamSpec::bool(&spec.parameter)
        .saved(spec.saved)
        .default_value(spec.default_on as i64 as f32);
    let sync_bits = param_spec.sync_cost_bits();
    let params = ExpressionParams::new(&params_name).parameter(param_spec);
    let menu = ExpressionsMenu::new(&menu_name)
        .control(MenuControlSpec::toggle(&spec.menu_label, &spec.parameter));

    let wiring_note = format!(
        "Drop all files into your project's Assets/. Then, on the avatar's VRC Avatar Descriptor: \
         (1) set the FX playable layer's controller to {controller_name}.controller — or copy its \
         `{layer}` layer and `{param}` Bool parameter into your existing FX controller; \
         (2) merge the `{param}` entry from {params_name}.asset into your expression parameters \
         asset (or assign {params_name}.asset if you have none) — it costs {sync_bits} sync bit(s); \
         (3) merge the `{label}` control from {menu_name}.asset into your expressions menu (or \
         assign {menu_name}.asset). The .meta files pin the GUIDs the bundle's cross-references \
         use — keep them next to their assets.",
        layer = spec.name,
        param = spec.parameter,
        label = spec.menu_label,
    );

    let files = vec![
        GeneratedFile {
            file_name: format!("{on_name}.anim"),
            content: on_clip.to_unity_yaml(on_clip_id),
        },
        GeneratedFile {
            file_name: format!("{on_name}.anim.meta"),
            content: native_asset_meta(&on_guid, on_clip_id),
        },
        GeneratedFile {
            file_name: format!("{off_name}.anim"),
            content: off_clip.to_unity_yaml(off_clip_id),
        },
        GeneratedFile {
            file_name: format!("{off_name}.anim.meta"),
            content: native_asset_meta(&off_guid, off_clip_id),
        },
        GeneratedFile {
            file_name: format!("{controller_name}.controller"),
            content: controller_yaml,
        },
        GeneratedFile {
            file_name: format!("{controller_name}.controller.meta"),
            content: native_asset_meta(&controller_guid, controller_id),
        },
        GeneratedFile {
            file_name: format!("{params_name}.asset"),
            content: params.to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID),
        },
        GeneratedFile {
            file_name: format!("{params_name}.asset.meta"),
            content: native_asset_meta(&params_guid, EXPRESSIONS_MAIN_FILE_ID),
        },
        GeneratedFile {
            file_name: format!("{menu_name}.asset"),
            content: menu.to_unity_yaml(EXPRESSIONS_MAIN_FILE_ID),
        },
        GeneratedFile {
            file_name: format!("{menu_name}.asset.meta"),
            content: native_asset_meta(&menu_guid, EXPRESSIONS_MAIN_FILE_ID),
        },
    ];

    ToggleBundle {
        files,
        parameter: spec.parameter.clone(),
        controller_guid,
        params_guid,
        menu_guid,
        sync_bits,
        wiring_note,
    }
}

/// Emit the two-state toggle fragment: an `AnimatorStateMachine` (1107) with `Off` and `On`
/// states (1102) playing `off_motion` / `on_motion`, and instant transitions both ways (1101)
/// conditioned on the `Bool` `parameter`. Returns the fragment text (no preamble) and the
/// state-machine fileID for the owning layer's `m_StateMachine`.
pub fn toggle_state_fragment(
    name: &str,
    parameter: &str,
    on_motion: &ObjectRef,
    off_motion: &ObjectRef,
    default_on: bool,
    ids: &mut IdGen,
) -> (String, i64) {
    let sm_id = ids.alloc();
    let off_state_id = ids.alloc();
    let on_state_id = ids.alloc();
    let to_on_id = ids.alloc();
    let to_off_id = ids.alloc();
    let default_state_id = if default_on {
        on_state_id
    } else {
        off_state_id
    };

    let mut e = Emitter::new();

    // --- AnimatorStateMachine (1107)
    e.doc_header(1107, sm_id);
    e.line("AnimatorStateMachine:");
    e.indented(|e| {
        e.kv("m_ObjectHideFlags", "1");
        e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
        e.kv("m_PrefabInstance", "{fileID: 0}");
        e.kv("m_PrefabAsset", "{fileID: 0}");
        e.kv("m_Name", name);
        e.key("m_ChildStates");
        e.indented(|e| {
            e.line("- serializedVersion: 1");
            e.indented(|e| {
                e.kv_ref("m_State", &ObjectRef::local(off_state_id));
                e.kv("m_Position", "{x: 200, y: 0, z: 0}");
            });
            e.line("- serializedVersion: 1");
            e.indented(|e| {
                e.kv_ref("m_State", &ObjectRef::local(on_state_id));
                e.kv("m_Position", "{x: 200, y: 120, z: 0}");
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
        e.kv_ref("m_DefaultState", &ObjectRef::local(default_state_id));
    });

    // --- The two AnimatorStates (1102)
    emit_toggle_state(&mut e, off_state_id, "Off", off_motion, to_on_id);
    emit_toggle_state(&mut e, on_state_id, "On", on_motion, to_off_id);

    // --- The two AnimatorStateTransitions (1101)
    emit_toggle_transition(&mut e, to_on_id, parameter, CONDITION_IF, on_state_id);
    emit_toggle_transition(&mut e, to_off_id, parameter, CONDITION_IF_NOT, off_state_id);

    (e.into_string(), sm_id)
}

fn emit_toggle_state(
    e: &mut Emitter,
    state_id: i64,
    name: &str,
    motion: &ObjectRef,
    transition_id: i64,
) {
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
        e.key("m_Transitions");
        e.indented(|e| {
            e.line(&format!("- {}", ObjectRef::local(transition_id).render()));
        });
        e.kv("m_StateMachineBehaviours", "[]");
        e.kv("m_Position", "{x: 50, y: 50, z: 0}");
        e.kv("m_IKOnFeet", "0");
        // Write Defaults OFF — VRChat's FX recommendation; the Off clip writes the same
        // properties back to their off values, so nothing depends on default-state restore.
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

fn emit_toggle_transition(
    e: &mut Emitter,
    transition_id: i64,
    parameter: &str,
    condition_mode: i64,
    dst_state_id: i64,
) {
    e.doc_header(1101, transition_id);
    e.line("AnimatorStateTransition:");
    e.indented(|e| {
        e.kv("m_ObjectHideFlags", "1");
        e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
        e.kv("m_PrefabInstance", "{fileID: 0}");
        e.kv("m_PrefabAsset", "{fileID: 0}");
        e.kv("m_Name", "");
        e.key("m_Conditions");
        e.indented(|e| {
            e.line(&format!("- m_ConditionMode: {condition_mode}"));
            e.indented(|e| {
                e.kv("m_ConditionEvent", parameter);
                // Unity's field name really is misspelled ("Treshold").
                e.kv_i64("m_EventTreshold", 0);
            });
        });
        e.kv("m_DstStateMachine", "{fileID: 0}");
        e.kv_ref("m_DstState", &ObjectRef::local(dst_state_id));
        e.kv("m_Solo", "0");
        e.kv("m_Mute", "0");
        e.kv("m_IsExit", "0");
        e.kv("serializedVersion", "3");
        // Instant switch: no exit time, zero duration — the standard VRChat toggle transition.
        e.kv_i64("m_TransitionDuration", 0);
        e.kv_i64("m_TransitionOffset", 0);
        e.kv_f32("m_ExitTime", 0.75);
        e.kv("m_HasExitTime", "0");
        e.kv("m_HasFixedDuration", "1");
        e.kv_i64("m_InterruptionSource", 0);
        e.kv("m_OrderedInterruption", "1");
        e.kv("m_CanTransitionToSelf", "1");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_unity_asset::AnimatorController as ReaderController;
    use avatar_unity_yaml::UnityFile;

    fn hat_spec() -> ToggleSpec {
        ToggleSpec {
            name: "Hat".into(),
            parameter: "Hat".into(),
            targets: vec![
                ToggleTarget::GameObject {
                    path: "Armature/Head/Hat".into(),
                },
                ToggleTarget::Blendshape {
                    path: "Body".into(),
                    shape: "HatHair".into(),
                    on_value: 100.0,
                },
            ],
            saved: true,
            default_on: false,
            menu_label: "Hat".into(),
        }
    }

    #[test]
    fn deterministic_guid_is_stable_hex_with_letter() {
        let g = deterministic_guid("Hat/on");
        assert_eq!(g, deterministic_guid("Hat/on"));
        assert_eq!(g.len(), 32);
        assert!(g.chars().all(|c| c.is_ascii_hexdigit()));
        // Forced letter first char: never parseable as a bare number (CLAUDE.md gotcha).
        assert!(g.chars().next().unwrap().is_ascii_lowercase());
        assert!(g.chars().next().unwrap().is_ascii_alphabetic());
        assert_ne!(g, deterministic_guid("Hat/off"));
    }

    #[test]
    fn bundle_has_all_files_and_is_deterministic() {
        let a = generate_toggle(&hat_spec());
        let b = generate_toggle(&hat_spec());
        let names: Vec<&str> = a.files.iter().map(|f| f.file_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Hat_On.anim",
                "Hat_On.anim.meta",
                "Hat_Off.anim",
                "Hat_Off.anim.meta",
                "Hat_FX.controller",
                "Hat_FX.controller.meta",
                "Hat_Params.asset",
                "Hat_Params.asset.meta",
                "Hat_Menu.asset",
                "Hat_Menu.asset.meta",
            ]
        );
        for (fa, fb) in a.files.iter().zip(&b.files) {
            assert_eq!(fa.content, fb.content);
        }
        assert_eq!(a.sync_bits, 1);
    }

    fn file<'a>(bundle: &'a ToggleBundle, name: &str) -> &'a str {
        &bundle
            .files
            .iter()
            .find(|f| f.file_name == name)
            .unwrap_or_else(|| panic!("bundle missing {name}"))
            .content
    }

    #[test]
    fn clips_animate_all_targets_both_ways() {
        let bundle = generate_toggle(&hat_spec());
        let on = UnityFile::parse(file(&bundle, "Hat_On.anim")).unwrap();
        let off = UnityFile::parse(file(&bundle, "Hat_Off.anim")).unwrap();
        for parsed in [&on, &off] {
            let curves = parsed.documents[0].body["m_FloatCurves"].as_vec().unwrap();
            assert_eq!(curves.len(), 2, "both targets animated");
        }
        // On holds 1/100; Off writes the same properties back to 0.
        assert!(file(&bundle, "Hat_On.anim").contains("value: 100"));
        assert!(file(&bundle, "Hat_Off.anim").contains("attribute: blendShape.HatHair"));
        assert!(file(&bundle, "Hat_Off.anim").contains("attribute: m_IsActive"));
    }

    #[test]
    fn controller_roundtrips_with_states_and_conditions() {
        let bundle = generate_toggle(&hat_spec());
        let yaml = file(&bundle, "Hat_FX.controller");
        let parsed = UnityFile::parse(yaml).unwrap();
        let c = ReaderController::from_file(&parsed).expect("controller parses");

        // The Bool parameter is declared and referenced by the transition conditions.
        let p = c.parameters.iter().find(|p| p.name == "Hat").unwrap();
        assert_eq!(p.type_name(), "Bool");
        assert!(c.conditions.iter().any(|cond| cond.parameter == "Hat"));

        // Two states, default present, Write Defaults uniformly off.
        assert_eq!(c.state_machines.len(), 1);
        assert!(c.state_machines[0].has_default_state);
        assert_eq!(c.state_machines[0].child_state_count, 2);
        assert_eq!(c.write_defaults, vec![false, false]);

        // Both transitions exist with the right modes, and their m_DstState fileIDs resolve to
        // state documents in the file.
        let transitions: Vec<_> = parsed
            .documents
            .iter()
            .filter(|d| d.class_id == 1101)
            .collect();
        assert_eq!(transitions.len(), 2);
        let modes: Vec<i64> = transitions
            .iter()
            .filter_map(|t| t.body["m_Conditions"].as_vec()?.first()?["m_ConditionMode"].as_i64())
            .collect();
        assert!(modes.contains(&CONDITION_IF) && modes.contains(&CONDITION_IF_NOT));
        for t in &transitions {
            let dst = t.body["m_DstState"]["fileID"].as_i64().unwrap();
            assert!(
                parsed
                    .documents
                    .iter()
                    .any(|d| d.class_id == 1102 && d.file_id == dst),
                "transition m_DstState resolves"
            );
        }

        // The states reference the clips by the same guid+fileID the clip .metas pin.
        let on_meta = file(&bundle, "Hat_On.anim.meta");
        let on_guid = on_meta
            .lines()
            .find_map(|l| l.strip_prefix("guid: "))
            .unwrap();
        assert!(yaml.contains(on_guid));
    }

    #[test]
    fn default_on_flips_default_state_and_param_default() {
        let mut spec = hat_spec();
        spec.default_on = true;
        let bundle = generate_toggle(&spec);
        let yaml = file(&bundle, "Hat_FX.controller");
        let parsed = UnityFile::parse(yaml).unwrap();
        let sm = parsed
            .documents
            .iter()
            .find(|d| d.class_id == 1107)
            .unwrap();
        let default_id = sm.body["m_DefaultState"]["fileID"].as_i64().unwrap();
        let default_state = parsed
            .documents
            .iter()
            .find(|d| d.class_id == 1102 && d.file_id == default_id)
            .unwrap();
        assert_eq!(default_state.name(), Some("On"));
        assert!(file(&bundle, "Hat_Params.asset").contains("defaultValue: 1"));
    }

    #[test]
    fn params_and_menu_are_recognized_by_the_structural_reader() {
        let bundle = generate_toggle(&hat_spec());
        let params = UnityFile::parse(file(&bundle, "Hat_Params.asset")).unwrap();
        let menu = UnityFile::parse(file(&bundle, "Hat_Menu.asset")).unwrap();
        let p_assets = avatar_vrc_descriptor::extract(&params);
        let m_assets = avatar_vrc_descriptor::extract(&menu);
        assert!(matches!(
            p_assets.as_slice(),
            [avatar_vrc_descriptor::VrcAsset::Parameters(p)] if p.synced_bits() == 1
        ));
        assert!(matches!(
            m_assets.as_slice(),
            [avatar_vrc_descriptor::VrcAsset::Menu(m)]
                if m.controls.len() == 1 && m.controls[0].parameter.as_deref() == Some("Hat")
        ));
    }
}
