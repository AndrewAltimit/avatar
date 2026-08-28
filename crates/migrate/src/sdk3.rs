//! SDK3 (Avatars 3.0) component emitters: `VRCAvatarDescriptor`, `VRCPhysBone`,
//! `VRCPhysBoneCollider`, `PipelineManager` — the MonoBehaviour bodies a migrated prefab needs,
//! rendered in the field order the SDK itself serializes.
//!
//! # Where the script references come from
//!
//! Every SDK3 runtime component is a class inside a DLL, so its `m_Script` is
//! `{fileID: <class hash>, guid: <dll guid>, type: 3}`. The DLL GUIDs below were read off the
//! `.dll.meta` files in `com.vrchat.avatars` / `com.vrchat.base` **3.10.4** and the class hashes
//! are [`avatar_unity_yaml::script_file_id`] (MD4 of `"s\0\0\0" + namespace + class`), which the
//! `unity-yaml` tests pin against the SDK's own serialized sample assets. Field layouts were taken
//! from the same packages' sample scenes (`Avatar Dynamics Robot Avatar PC.unity`) and default
//! assets, so an emitted body is a superset-compatible match for what Unity would write itself.
//!
//! # PhysBone parameter conversion
//!
//! [`PhysBoneSpec::from_dynamic_bone`] reproduces the SDK's own DynamicBone→PhysBone conversion
//! (`VRC.SDK3.Dynamics.PhysBone.PhysBoneMigration.Convert`, 3.10.4): `pull = elasticity`,
//! `spring = 1 − damping`, `immobile = inert` (world-space immobile), `stiffness = 0` with an
//! **Angle** limit whose `maxAngleX` comes from a fixed stiffness→angle table, radius scaled by
//! the DynamicBone-object/root lossy-scale ratio, Advanced integration, and gravity taken from the
//! DynamicBone's `m_Gravity.y` (or `m_Force.y`, whichever is larger) normalised by the chain's
//! average bone length. Collider conversion: capsule if `height > 2·radius` else sphere, `X`
//! direction → 90° about the local forward axis, `Z` → 90° about the local right axis,
//! `bonesAsSpheres` on.

use anyhow::{Result, bail};
use avatar_unity_yaml::{Yaml, field_f64, field_i64, ref_fileid, script_file_id};

use crate::math::{Quat, Vec3, fmt};

/// A MonoBehaviour `m_Script` reference (class hash inside a DLL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptRef {
    pub file_id: i32,
    pub guid: &'static str,
}

impl ScriptRef {
    const fn new(file_id: i32, guid: &'static str) -> Self {
        ScriptRef { file_id, guid }
    }
    /// `{fileID: N, guid: G, type: 3}`.
    pub fn render(&self) -> String {
        format!("{{fileID: {}, guid: {}, type: 3}}", self.file_id, self.guid)
    }
}

/// `VRCSDK3A.dll` (`com.vrchat.avatars`): descriptor, expression assets, avatar-side components.
pub const VRCSDK3A_DLL_GUID: &str = "67cc4cb7839cd3741b63733d5adf0442";
/// `VRC.SDK3.Dynamics.PhysBone.dll` (`com.vrchat.base`).
pub const PHYSBONE_DLL_GUID: &str = "2a2c05204084d904aa4945ccff20d8e5";
/// `VRC.SDK3.Dynamics.Contact.dll` (`com.vrchat.base`).
pub const CONTACT_DLL_GUID: &str = "80f1b8067b0760e4bb45023bc2e9de66";
/// `VRCCore-Standalone.dll` (`com.vrchat.base`): `VRC.Core.PipelineManager`.
pub const VRCCORE_DLL_GUID: &str = "b0e1c0f72d838fe49bfe88b987a471bd";

/// `VRC.SDK3.Avatars.Components.VRCAvatarDescriptor` — hash `542108242`.
pub const VRC_AVATAR_DESCRIPTOR: ScriptRef = ScriptRef::new(542108242, VRCSDK3A_DLL_GUID);
/// `VRC.SDK3.Dynamics.PhysBone.Components.VRCPhysBone` — hash `1661641543`.
pub const VRC_PHYS_BONE: ScriptRef = ScriptRef::new(1661641543, PHYSBONE_DLL_GUID);
/// `VRC.SDK3.Dynamics.PhysBone.Components.VRCPhysBoneCollider` — hash `-1631200402`.
pub const VRC_PHYS_BONE_COLLIDER: ScriptRef = ScriptRef::new(-1631200402, PHYSBONE_DLL_GUID);
/// `VRC.SDK3.Dynamics.Contact.Components.VRCContactReceiver` — hash `-1450912254`.
pub const VRC_CONTACT_RECEIVER: ScriptRef = ScriptRef::new(-1450912254, CONTACT_DLL_GUID);
/// `VRC.Core.PipelineManager` — hash `-1427037861` (computed; no SDK sample serializes one).
pub const PIPELINE_MANAGER: ScriptRef = ScriptRef::new(-1427037861, VRCCORE_DLL_GUID);

/// Sanity check that the pinned hashes are what [`script_file_id`] derives (a typo guard; the
/// derivation itself is tested against the SDK in `avatar-unity-yaml`).
pub fn verify_script_hashes() -> bool {
    script_file_id("VRC.SDK3.Avatars.Components", "VRCAvatarDescriptor")
        == VRC_AVATAR_DESCRIPTOR.file_id
        && script_file_id("VRC.SDK3.Dynamics.PhysBone.Components", "VRCPhysBone")
            == VRC_PHYS_BONE.file_id
        && script_file_id(
            "VRC.SDK3.Dynamics.PhysBone.Components",
            "VRCPhysBoneCollider",
        ) == VRC_PHYS_BONE_COLLIDER.file_id
        && script_file_id("VRC.Core", "PipelineManager") == PIPELINE_MANAGER.file_id
}

// ---------------------------------------------------------------------------------------------
// Small YAML helpers

fn local_ref(id: i64) -> String {
    if id == 0 {
        "{fileID: 0}".to_string()
    } else {
        format!("{{fileID: {id}}}")
    }
}

fn asset_ref(r: Option<&(i64, String)>) -> String {
    match r {
        Some((file_id, guid)) => format!("{{fileID: {file_id}, guid: {guid}, type: 2}}"),
        None => "{fileID: 0}".to_string(),
    }
}

/// A PhysBone per-chain curve: keys of `(position along the chain 0..1, multiplier 0..1)`. The
/// SDK evaluates it at each bone's normalised position and multiplies the base value by it (so
/// `pull 0.3` with a curve `0:0.5, 1:1` pulls 0.15 at the root and 0.3 at the tip). Empty = no
/// curve (the base value everywhere). Rendered with **linear** segments: every key gets free
/// tangents equal to the secants to its neighbours, which makes the Hermite curve Unity evaluates
/// exactly piecewise-linear — what you'd expect from `0:0.5, 1:1`, with no editor smoothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve(pub Vec<(f64, f64)>);

impl Curve {
    /// No curve.
    pub const NONE: Curve = Curve(Vec::new());

    /// Parse the CLI form `t:v,t:v,…` (e.g. `0:0.5,1:1`); keys are sorted by time.
    pub fn parse(s: &str) -> Result<Curve> {
        let mut keys = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((t, v)) = part.split_once(':') else {
                bail!("curve key '{part}' is not `time:value`");
            };
            let t: f64 = t
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("curve time '{t}'"))?;
            let v: f64 = v
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("curve value '{v}'"))?;
            if !(0.0..=1.0).contains(&t) {
                bail!("curve time {t} is outside 0..1 (position along the chain)");
            }
            keys.push((t, v));
        }
        keys.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(Curve(keys))
    }

    /// True if unset.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Read a serialized `AnimationCurve` block back (only `time`/`value` are kept).
    pub fn from_yaml(node: &Yaml) -> Curve {
        let mut keys = Vec::new();
        if let Some(items) = node["m_Curve"].as_vec() {
            for k in items {
                if let (Some(t), Some(v)) = (field_f64(k, "time"), field_f64(k, "value")) {
                    keys.push((t, v));
                }
            }
        }
        Curve(keys)
    }

    /// Human form: `0:0.5 → 1:1`.
    pub fn describe(&self) -> String {
        self.0
            .iter()
            .map(|(t, v)| format!("{}:{}", fmt(*t), fmt(*v)))
            .collect::<Vec<_>>()
            .join(" → ")
    }

    /// The curve's multiplier at chain position `t` (linear between keys, clamped outside).
    pub fn eval(&self, t: f64) -> f64 {
        let k = &self.0;
        match k.len() {
            0 => 1.0,
            1 => k[0].1,
            _ => {
                if t <= k[0].0 {
                    return k[0].1;
                }
                for w in k.windows(2) {
                    let ((t0, v0), (t1, v1)) = (w[0], w[1]);
                    if t <= t1 {
                        let span = t1 - t0;
                        return if span <= 0.0 {
                            v1
                        } else {
                            v0 + (v1 - v0) * (t - t0) / span
                        };
                    }
                }
                k[k.len() - 1].1
            }
        }
    }

    /// Render as the `AnimationCurve` block Unity serializes under `key`.
    fn render(&self, out: &mut String, key: &str) {
        out.push_str(&format!("  {key}:\n    serializedVersion: 2\n"));
        if self.0.is_empty() {
            out.push_str("    m_Curve: []\n");
        } else {
            out.push_str("    m_Curve:\n");
            let n = self.0.len();
            let secant = |a: usize, b: usize| -> f64 {
                let (ta, va) = self.0[a];
                let (tb, vb) = self.0[b];
                if (tb - ta).abs() < 1e-9 {
                    0.0
                } else {
                    (vb - va) / (tb - ta)
                }
            };
            for i in 0..n {
                let (t, v) = self.0[i];
                let out_slope = if i + 1 < n { secant(i, i + 1) } else { 0.0 };
                let in_slope = if i > 0 { secant(i - 1, i) } else { out_slope };
                let out_slope = if i + 1 < n { out_slope } else { in_slope };
                out.push_str(&format!(
                    "    - serializedVersion: 3\n      time: {}\n      value: {}\n      inSlope: {}\n      outSlope: {}\n      tangentMode: 0\n      weightedMode: 0\n      inWeight: 0.33333334\n      outWeight: 0.33333334\n",
                    fmt(t),
                    fmt(v),
                    fmt(in_slope),
                    fmt(out_slope)
                ));
            }
        }
        out.push_str("    m_PreInfinity: 2\n    m_PostInfinity: 2\n    m_RotationOrder: 4\n");
    }
}

/// A PhysBone `*Filter` block (`allowSelf` / `allowOthers`).
fn filter_block(out: &mut String, key: &str, (allow_self, allow_others): (bool, bool)) {
    out.push_str(&format!(
        "  {key}:\n    allowSelf: {}\n    allowOthers: {}\n",
        allow_self as u8, allow_others as u8
    ));
}

/// The MonoBehaviour header shared by every component body (through `m_EditorClassIdentifier`).
fn mono_head(out: &mut String, game_object: i64, script: &ScriptRef) {
    out.push_str("MonoBehaviour:\n");
    out.push_str("  m_ObjectHideFlags: 0\n");
    out.push_str("  m_CorrespondingSourceObject: {fileID: 0}\n");
    out.push_str("  m_PrefabInstance: {fileID: 0}\n");
    out.push_str("  m_PrefabAsset: {fileID: 0}\n");
    out.push_str(&format!("  m_GameObject: {}\n", local_ref(game_object)));
    out.push_str("  m_Enabled: 1\n");
    out.push_str("  m_EditorHideFlags: 0\n");
    out.push_str(&format!("  m_Script: {}\n", script.render()));
    out.push_str("  m_Name: \n");
    out.push_str("  m_EditorClassIdentifier: \n");
}

// ---------------------------------------------------------------------------------------------
// VRCAvatarDescriptor

/// `lipSync` mode on the descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LipSyncStyle {
    Default = 0,
    JawFlapBone = 1,
    JawFlapBlendShape = 2,
    VisemeBlendShape = 3,
    VisemeParameterOnly = 4,
}

/// One playable-layer slot on the descriptor.
#[derive(Debug, Clone, Default)]
pub struct PlayableLayer {
    /// `{fileID, guid}` of an AnimatorController asset; `None` = SDK default for this layer.
    pub controller: Option<(i64, String)>,
}

/// The eye-look rotation set: the local rotation of each eye bone in a given look state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyeRotations {
    pub left: Quat,
    pub right: Quat,
}

/// Eye-look configuration (`customEyeLookSettings`).
#[derive(Debug, Clone)]
pub struct EyeLook {
    pub left_eye: i64,
    pub right_eye: i64,
    pub straight: EyeRotations,
    pub up: EyeRotations,
    pub down: EyeRotations,
    pub left: EyeRotations,
    pub right: EyeRotations,
    /// Blink via blendshapes on `eyelids_mesh`: `(blink, looking_up, looking_down)` indices, −1 =
    /// none. `None` = no eyelid animation.
    pub eyelid_blendshapes: Option<(i64, [i32; 3])>,
}

/// Everything the migrated descriptor carries over / sets.
#[derive(Debug, Clone)]
pub struct DescriptorSpec {
    pub game_object: i64,
    pub view_position: Vec3,
    pub scale_ipd: bool,
    pub lip_sync: LipSyncStyle,
    /// SkinnedMeshRenderer fileID carrying the visemes (or 0).
    pub viseme_mesh: i64,
    pub viseme_blendshapes: Vec<String>,
    pub mouth_open_blendshape: String,
    pub portrait_camera_position_offset: Vec3,
    pub portrait_camera_rotation_offset: Quat,
    /// Base, Additive, Gesture, Action, FX (in this order).
    pub base_layers: [PlayableLayer; 5],
    /// Sitting, TPose, IKPose.
    pub special_layers: [PlayableLayer; 3],
    pub expressions_menu: Option<(i64, String)>,
    pub expression_parameters: Option<(i64, String)>,
    pub eye_look: Option<EyeLook>,
}

impl DescriptorSpec {
    /// Render the descriptor MonoBehaviour body (from `MonoBehaviour:`; no `---` header).
    pub fn to_body(&self) -> String {
        let mut o = String::new();
        mono_head(&mut o, self.game_object, &VRC_AVATAR_DESCRIPTOR);
        o.push_str("  Name: \n");
        o.push_str(&format!(
            "  ViewPosition: {}\n",
            self.view_position.to_yaml()
        ));
        o.push_str("  Animations: 0\n");
        o.push_str(&format!("  ScaleIPD: {}\n", self.scale_ipd as u8));
        o.push_str(&format!("  lipSync: {}\n", self.lip_sync as u8));
        o.push_str("  lipSyncJawBone: {fileID: 0}\n");
        o.push_str("  lipSyncJawClosed: {x: 0, y: 0, z: 0, w: 1}\n");
        o.push_str("  lipSyncJawOpen: {x: 0, y: 0, z: 0, w: 1}\n");
        o.push_str(&format!(
            "  VisemeSkinnedMesh: {}\n",
            local_ref(self.viseme_mesh)
        ));
        o.push_str(&format!(
            "  MouthOpenBlendShapeName: {}\n",
            self.mouth_open_blendshape
        ));
        if self.viseme_blendshapes.is_empty() {
            o.push_str("  VisemeBlendShapes: []\n");
        } else {
            o.push_str("  VisemeBlendShapes:\n");
            for v in &self.viseme_blendshapes {
                o.push_str(&format!("  - {v}\n"));
            }
        }
        o.push_str("  unityVersion: \n");
        o.push_str(&format!(
            "  portraitCameraPositionOffset: {}\n",
            self.portrait_camera_position_offset.to_yaml()
        ));
        o.push_str(&format!(
            "  portraitCameraRotationOffset: {}\n",
            self.portrait_camera_rotation_offset.to_yaml()
        ));
        o.push_str("  networkIDs: []\n");
        let custom_expr = self.expressions_menu.is_some() || self.expression_parameters.is_some();
        o.push_str(&format!("  customExpressions: {}\n", custom_expr as u8));
        o.push_str(&format!(
            "  expressionsMenu: {}\n",
            asset_ref(self.expressions_menu.as_ref())
        ));
        o.push_str(&format!(
            "  expressionParameters: {}\n",
            asset_ref(self.expression_parameters.as_ref())
        ));
        match &self.eye_look {
            Some(el) => {
                o.push_str("  enableEyeLook: 1\n");
                o.push_str("  customEyeLookSettings:\n");
                o.push_str("    eyeMovement:\n      confidence: 0.5\n      excitement: 0.5\n");
                o.push_str(&format!("    leftEye: {}\n", local_ref(el.left_eye)));
                o.push_str(&format!("    rightEye: {}\n", local_ref(el.right_eye)));
                for (key, rots) in [
                    ("eyesLookingStraight", el.straight),
                    ("eyesLookingUp", el.up),
                    ("eyesLookingDown", el.down),
                    ("eyesLookingLeft", el.left),
                    ("eyesLookingRight", el.right),
                ] {
                    o.push_str(&format!("    {key}:\n      linked: 0\n"));
                    o.push_str(&format!("      left: {}\n", rots.left.to_yaml()));
                    o.push_str(&format!("      right: {}\n", rots.right.to_yaml()));
                }
                let (eyelid_type, mesh, shapes) = match &el.eyelid_blendshapes {
                    Some((mesh, idx)) => (2, *mesh, *idx),
                    None => (0, 0, [-1, -1, -1]),
                };
                o.push_str(&format!("    eyelidType: {eyelid_type}\n"));
                for k in [
                    "upperLeftEyelid",
                    "upperRightEyelid",
                    "lowerLeftEyelid",
                    "lowerRightEyelid",
                ] {
                    o.push_str(&format!("    {k}: {{fileID: 0}}\n"));
                }
                for k in [
                    "eyelidsDefault",
                    "eyelidsClosed",
                    "eyelidsLookingUp",
                    "eyelidsLookingDown",
                ] {
                    o.push_str(&format!("    {k}:\n"));
                    for part in ["upper", "lower"] {
                        o.push_str(&format!(
                            "      {part}:\n        linked: 1\n        left: {{x: 0, y: 0, z: 0, w: 0}}\n        right: {{x: 0, y: 0, z: 0, w: 0}}\n"
                        ));
                    }
                }
                o.push_str(&format!("    eyelidsSkinnedMesh: {}\n", local_ref(mesh)));
                // Serialized as a raw int32[3] hex blob (little-endian).
                let mut hex = String::new();
                for v in shapes {
                    for b in v.to_le_bytes() {
                        hex.push_str(&format!("{b:02x}"));
                    }
                }
                o.push_str(&format!("    eyelidsBlendshapes: {hex}\n"));
            }
            None => {
                o.push_str("  enableEyeLook: 0\n");
                o.push_str("  customEyeLookSettings:\n");
                o.push_str("    eyeMovement:\n      confidence: 0.5\n      excitement: 0.5\n");
                o.push_str("    leftEye: {fileID: 0}\n    rightEye: {fileID: 0}\n");
                for key in [
                    "eyesLookingStraight",
                    "eyesLookingUp",
                    "eyesLookingDown",
                    "eyesLookingLeft",
                    "eyesLookingRight",
                ] {
                    o.push_str(&format!(
                        "    {key}:\n      linked: 1\n      left: {{x: 0, y: 0, z: 0, w: 1}}\n      right: {{x: 0, y: 0, z: 0, w: 1}}\n"
                    ));
                }
                o.push_str("    eyelidType: 0\n");
                for k in [
                    "upperLeftEyelid",
                    "upperRightEyelid",
                    "lowerLeftEyelid",
                    "lowerRightEyelid",
                ] {
                    o.push_str(&format!("    {k}: {{fileID: 0}}\n"));
                }
                for k in [
                    "eyelidsDefault",
                    "eyelidsClosed",
                    "eyelidsLookingUp",
                    "eyelidsLookingDown",
                ] {
                    o.push_str(&format!("    {k}:\n"));
                    for part in ["upper", "lower"] {
                        o.push_str(&format!(
                            "      {part}:\n        linked: 1\n        left: {{x: 0, y: 0, z: 0, w: 0}}\n        right: {{x: 0, y: 0, z: 0, w: 0}}\n"
                        ));
                    }
                }
                o.push_str("    eyelidsSkinnedMesh: {fileID: 0}\n");
                o.push_str("    eyelidsBlendshapes: ffffffffffffffffffffffff\n");
            }
        }
        // Playable layers. `customizeAnimationLayers: 1` so the per-layer entries are honoured.
        o.push_str("  customizeAnimationLayers: 1\n");
        o.push_str("  baseAnimationLayers:\n");
        for (i, layer) in self.base_layers.iter().enumerate() {
            // Base=0, Additive=2, Gesture=3, Action=4, FX=5 (1 is a deprecated slot).
            let ty = [0, 2, 3, 4, 5][i];
            emit_layer(&mut o, ty, layer);
        }
        o.push_str("  specialAnimationLayers:\n");
        for (i, layer) in self.special_layers.iter().enumerate() {
            let ty = [6, 7, 8][i];
            emit_layer(&mut o, ty, layer);
        }
        o.push_str("  AnimationPreset: {fileID: 0}\n");
        o.push_str("  animationHashSet: []\n");
        o.push_str("  autoFootsteps: 1\n");
        o.push_str("  autoLocomotion: 1\n");
        // Contact colliders: `state: 0` = Automatic — the SDK derives them from the humanoid rig.
        for name in [
            "collider_head",
            "collider_torso",
            "collider_footR",
            "collider_footL",
            "collider_handR",
            "collider_handL",
            "collider_fingerIndexL",
            "collider_fingerMiddleL",
            "collider_fingerRingL",
            "collider_fingerLittleL",
            "collider_fingerIndexR",
            "collider_fingerMiddleR",
            "collider_fingerRingR",
            "collider_fingerLittleR",
        ] {
            o.push_str(&format!(
                "  {name}:\n    isMirrored: 1\n    state: 0\n    transform: {{fileID: 0}}\n    radius: 0\n    height: 0\n    position: {{x: 0, y: 0, z: 0}}\n    rotation: {{x: 0, y: 0, z: 0, w: 1}}\n"
            ));
        }
        o
    }
}

fn emit_layer(o: &mut String, ty: i64, layer: &PlayableLayer) {
    o.push_str("  - isEnabled: 0\n");
    o.push_str(&format!("    type: {ty}\n"));
    o.push_str(&format!(
        "    animatorController: {}\n",
        asset_ref(layer.controller.as_ref())
    ));
    o.push_str("    mask: {fileID: 0}\n");
    o.push_str(&format!(
        "    isDefault: {}\n",
        layer.controller.is_none() as u8
    ));
}

// ---------------------------------------------------------------------------------------------
// PipelineManager

/// Render a `PipelineManager` body (blank blueprint = a new upload).
pub fn pipeline_manager_body(game_object: i64, blueprint_id: &str) -> String {
    let mut o = String::new();
    mono_head(&mut o, game_object, &PIPELINE_MANAGER);
    o.push_str("  launchedFromSDKPipeline: 0\n");
    o.push_str("  completedSDKPipeline: 0\n");
    o.push_str(&format!("  blueprintId: {blueprint_id}\n"));
    o.push_str("  contentType: 0\n");
    o.push_str("  assetBundleUnityVersion: \n");
    o.push_str("  fallbackStatus: 0\n");
    o
}

// ---------------------------------------------------------------------------------------------
// VRCPhysBoneCollider

/// `shapeType` on a PhysBone collider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColliderShape {
    Sphere = 0,
    Capsule = 1,
    Plane = 2,
}

/// A `VRCPhysBoneCollider` component.
#[derive(Debug, Clone)]
pub struct PhysBoneColliderSpec {
    pub game_object: i64,
    /// `rootTransform` (0 = the component's own transform).
    pub root_transform: i64,
    pub shape: ColliderShape,
    pub inside_bounds: bool,
    pub radius: f64,
    /// Capsule height (end to end, including the caps), Unity/DynamicBone convention.
    pub height: f64,
    pub position: Vec3,
    pub rotation: Quat,
    pub bones_as_spheres: bool,
}

impl PhysBoneColliderSpec {
    /// The SDK's DynamicBoneCollider conversion: `direction` 0 = X, 1 = Y, 2 = Z; `bound` 1 =
    /// inside. Capsule if `height > 2 · radius`, else sphere.
    pub fn from_dynamic_bone_collider(
        game_object: i64,
        direction: i64,
        bound: i64,
        radius: f64,
        height: f64,
        center: Vec3,
    ) -> Self {
        let shape = if height > radius * 2.0 {
            ColliderShape::Capsule
        } else {
            ColliderShape::Sphere
        };
        let rotation = match direction {
            0 => Quat::axis_angle(Vec3::Z, 90.0),
            2 => Quat::axis_angle(Vec3::X, 90.0),
            _ => Quat::IDENTITY,
        };
        PhysBoneColliderSpec {
            game_object,
            root_transform: 0,
            shape,
            inside_bounds: bound == 1,
            radius,
            height,
            position: center,
            rotation,
            bones_as_spheres: true,
        }
    }

    /// From a Unity `CapsuleCollider` (the physics collider a Cloth-simulated skirt used):
    /// same geometry, same direction convention (0 = X, 1 = Y, 2 = Z).
    pub fn from_capsule_collider(
        game_object: i64,
        direction: i64,
        radius: f64,
        height: f64,
        center: Vec3,
    ) -> Self {
        let mut c =
            Self::from_dynamic_bone_collider(game_object, direction, 0, radius, height, center);
        c.shape = ColliderShape::Capsule;
        c
    }

    /// Render the body.
    pub fn to_body(&self) -> String {
        let mut o = String::new();
        mono_head(&mut o, self.game_object, &VRC_PHYS_BONE_COLLIDER);
        o.push_str(&format!(
            "  rootTransform: {}\n",
            local_ref(self.root_transform)
        ));
        o.push_str(&format!("  shapeType: {}\n", self.shape as u8));
        o.push_str(&format!("  insideBounds: {}\n", self.inside_bounds as u8));
        o.push_str(&format!("  radius: {}\n", fmt(self.radius)));
        o.push_str(&format!("  height: {}\n", fmt(self.height)));
        o.push_str(&format!("  position: {}\n", self.position.to_yaml()));
        o.push_str(&format!("  rotation: {}\n", self.rotation.to_yaml()));
        o.push_str(&format!(
            "  bonesAsSpheres: {}\n",
            self.bones_as_spheres as u8
        ));
        o
    }
}

// ---------------------------------------------------------------------------------------------
// VRCPhysBone

/// `limitType` on a PhysBone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitType {
    None = 0,
    Angle = 1,
    Hinge = 2,
    Polar = 3,
}

/// A `VRCPhysBone` component. Defaults match a freshly added component in the SDK inspector.
#[derive(Debug, Clone)]
pub struct PhysBoneSpec {
    pub game_object: i64,
    /// 0 = Version_1_0, 1 = Version_1_1.
    pub version: i64,
    /// 0 = Simplified, 1 = Advanced.
    pub integration_type: i64,
    /// `rootTransform` (0 = own transform).
    pub root_transform: i64,
    pub ignore_transforms: Vec<i64>,
    pub endpoint_position: Vec3,
    /// 0 = Ignore, 1 = First, 2 = Average.
    pub multi_child_type: i64,
    pub pull: f64,
    pub pull_curve: Curve,
    pub spring: f64,
    pub spring_curve: Curve,
    pub stiffness: f64,
    pub stiffness_curve: Curve,
    pub gravity: f64,
    pub gravity_curve: Curve,
    pub gravity_falloff: f64,
    pub gravity_falloff_curve: Curve,
    /// 0 = All Motion, 1 = World (Parent) Motion.
    pub immobile_type: i64,
    pub immobile: f64,
    pub immobile_curve: Curve,
    pub allow_collision: bool,
    /// `collisionFilter` (allowSelf, allowOthers).
    pub collision_filter: (bool, bool),
    pub radius: f64,
    pub radius_curve: Curve,
    /// PhysBoneCollider component fileIDs.
    pub colliders: Vec<i64>,
    pub limit_type: LimitType,
    pub max_angle_x: f64,
    pub max_angle_x_curve: Curve,
    pub max_angle_z: f64,
    pub max_angle_z_curve: Curve,
    pub limit_rotation: Vec3,
    pub limit_rotation_curves: [Curve; 3],
    pub static_freeze_axis: Vec3,
    pub allow_grabbing: bool,
    /// `grabFilter` (allowSelf, allowOthers).
    pub grab_filter: (bool, bool),
    pub allow_posing: bool,
    /// `poseFilter` (allowSelf, allowOthers).
    pub pose_filter: (bool, bool),
    pub snap_to_hand: bool,
    pub grab_movement: f64,
    pub max_stretch: f64,
    pub max_stretch_curve: Curve,
    pub max_squish: f64,
    pub max_squish_curve: Curve,
    pub stretch_motion: f64,
    pub stretch_motion_curve: Curve,
    pub is_animated: bool,
    pub reset_when_disabled: bool,
    pub parameter: String,
}

impl PhysBoneSpec {
    /// A component with the SDK inspector's defaults, rooted at its own transform.
    pub fn new(game_object: i64) -> Self {
        PhysBoneSpec {
            game_object,
            version: 1,
            integration_type: 0,
            root_transform: 0,
            ignore_transforms: Vec::new(),
            endpoint_position: Vec3::ZERO,
            multi_child_type: 0,
            pull: 0.2,
            pull_curve: Curve::NONE,
            spring: 0.2,
            spring_curve: Curve::NONE,
            stiffness: 0.2,
            stiffness_curve: Curve::NONE,
            gravity: 0.0,
            gravity_curve: Curve::NONE,
            gravity_falloff: 0.0,
            gravity_falloff_curve: Curve::NONE,
            immobile_type: 0,
            immobile: 0.0,
            immobile_curve: Curve::NONE,
            allow_collision: true,
            collision_filter: (true, true),
            radius: 0.0,
            radius_curve: Curve::NONE,
            colliders: Vec::new(),
            limit_type: LimitType::None,
            max_angle_x: 45.0,
            max_angle_x_curve: Curve::NONE,
            max_angle_z: 45.0,
            max_angle_z_curve: Curve::NONE,
            limit_rotation: Vec3::ZERO,
            limit_rotation_curves: [Curve::NONE, Curve::NONE, Curve::NONE],
            static_freeze_axis: Vec3::ZERO,
            allow_grabbing: true,
            grab_filter: (true, true),
            allow_posing: true,
            pose_filter: (true, true),
            snap_to_hand: false,
            grab_movement: 0.5,
            max_stretch: 0.0,
            max_stretch_curve: Curve::NONE,
            max_squish: 0.0,
            max_squish_curve: Curve::NONE,
            stretch_motion: 0.0,
            stretch_motion_curve: Curve::NONE,
            is_animated: false,
            reset_when_disabled: false,
            parameter: String::new(),
        }
    }

    /// Read an existing `VRCPhysBone` MonoBehaviour body back into a spec (the inverse of
    /// [`to_body`](Self::to_body)); missing fields take the inspector defaults. `body` is the
    /// document's mapping (`UnityDocument::body`).
    pub fn from_yaml(body: &Yaml) -> Self {
        let mut pb = PhysBoneSpec::new(ref_fileid(body, "m_GameObject").unwrap_or(0));
        let f = |k: &str, d: f64| field_f64(body, k).unwrap_or(d);
        let i = |k: &str, d: i64| field_i64(body, k).unwrap_or(d);
        let b = |k: &str, d: bool| field_i64(body, k).map(|v| v != 0).unwrap_or(d);
        let refs = |k: &str| -> Vec<i64> {
            body[k]
                .as_vec()
                .map(|v| v.iter().filter_map(|r| field_i64(r, "fileID")).collect())
                .unwrap_or_default()
        };
        let filter = |k: &str| -> (bool, bool) {
            let n = &body[k];
            (
                field_i64(n, "allowSelf").map(|v| v != 0).unwrap_or(true),
                field_i64(n, "allowOthers").map(|v| v != 0).unwrap_or(true),
            )
        };
        let curve = |k: &str| Curve::from_yaml(&body[k]);
        pb.version = i("version", pb.version);
        pb.integration_type = i("integrationType", pb.integration_type);
        pb.root_transform = ref_fileid(body, "rootTransform").unwrap_or(0);
        pb.ignore_transforms = refs("ignoreTransforms");
        if !body["endpointPosition"].is_badvalue() {
            pb.endpoint_position = crate::scene::vec3(&body["endpointPosition"]);
        }
        pb.multi_child_type = i("multiChildType", pb.multi_child_type);
        pb.pull = f("pull", pb.pull);
        pb.pull_curve = curve("pullCurve");
        pb.spring = f("spring", pb.spring);
        pb.spring_curve = curve("springCurve");
        pb.stiffness = f("stiffness", pb.stiffness);
        pb.stiffness_curve = curve("stiffnessCurve");
        pb.gravity = f("gravity", pb.gravity);
        pb.gravity_curve = curve("gravityCurve");
        pb.gravity_falloff = f("gravityFalloff", pb.gravity_falloff);
        pb.gravity_falloff_curve = curve("gravityFalloffCurve");
        pb.immobile_type = i("immobileType", pb.immobile_type);
        pb.immobile = f("immobile", pb.immobile);
        pb.immobile_curve = curve("immobileCurve");
        pb.allow_collision = b("allowCollision", pb.allow_collision);
        pb.collision_filter = filter("collisionFilter");
        pb.radius = f("radius", pb.radius);
        pb.radius_curve = curve("radiusCurve");
        pb.colliders = refs("colliders");
        pb.limit_type = match i("limitType", 0) {
            1 => LimitType::Angle,
            2 => LimitType::Hinge,
            3 => LimitType::Polar,
            _ => LimitType::None,
        };
        pb.max_angle_x = f("maxAngleX", pb.max_angle_x);
        pb.max_angle_x_curve = curve("maxAngleXCurve");
        pb.max_angle_z = f("maxAngleZ", pb.max_angle_z);
        pb.max_angle_z_curve = curve("maxAngleZCurve");
        if !body["limitRotation"].is_badvalue() {
            pb.limit_rotation = crate::scene::vec3(&body["limitRotation"]);
        }
        pb.limit_rotation_curves = [
            curve("limitRotationXCurve"),
            curve("limitRotationYCurve"),
            curve("limitRotationZCurve"),
        ];
        if !body["staticFreezeAxis"].is_badvalue() {
            pb.static_freeze_axis = crate::scene::vec3(&body["staticFreezeAxis"]);
        }
        pb.allow_grabbing = b("allowGrabbing", pb.allow_grabbing);
        pb.grab_filter = filter("grabFilter");
        pb.allow_posing = b("allowPosing", pb.allow_posing);
        pb.pose_filter = filter("poseFilter");
        pb.snap_to_hand = b("snapToHand", false);
        pb.grab_movement = f("grabMovement", pb.grab_movement);
        pb.max_stretch = f("maxStretch", pb.max_stretch);
        pb.max_stretch_curve = curve("maxStretchCurve");
        pb.max_squish = f("maxSquish", pb.max_squish);
        pb.max_squish_curve = curve("maxSquishCurve");
        pb.stretch_motion = f("stretchMotion", pb.stretch_motion);
        pb.stretch_motion_curve = curve("stretchMotionCurve");
        pb.is_animated = b("isAnimated", pb.is_animated);
        pb.reset_when_disabled = b("resetWhenDisabled", false);
        pb.parameter = body["parameter"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_default();
        pb
    }

    /// The SDK's DynamicBone→PhysBone parameter conversion (see the module docs). `scale_ratio`
    /// is `|lossyScale.x(dynamic bone object)| / |lossyScale.x(root)|`; `avg_bone_length` is the
    /// chain's average world bone length (used to normalise gravity); `gravity_y`/`force_y` are
    /// the DynamicBone's `m_Gravity.y` / `m_Force.y`; `freeze_axis` 0 = none, 1 = X, 2 = Y, 3 = Z.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dynamic_bone(
        game_object: i64,
        root_transform: i64,
        exclusions: Vec<i64>,
        elasticity: f64,
        damping: f64,
        stiffness: f64,
        inert: f64,
        radius: f64,
        scale_ratio: f64,
        gravity_y: f64,
        force_y: f64,
        freeze_axis: i64,
        avg_bone_length: f64,
        object_lossy_scale_x: f64,
        colliders: Vec<i64>,
    ) -> Self {
        let mut pb = PhysBoneSpec::new(game_object);
        pb.version = 0;
        pb.is_animated = true;
        pb.root_transform = root_transform;
        pb.ignore_transforms = exclusions;
        pb.multi_child_type = 0;
        pb.immobile_type = 1;
        pb.integration_type = 1;
        pb.stiffness = 0.0;
        pb.pull = elasticity;
        pb.spring = 1.0 - damping;
        pb.immobile = inert;
        pb.radius = radius * scale_ratio;
        match freeze_axis {
            1..=3 => {
                pb.limit_type = LimitType::Hinge;
                pb.static_freeze_axis = match freeze_axis {
                    1 => Vec3::X,
                    2 => Vec3::Y,
                    _ => Vec3::Z,
                };
            }
            _ => {
                pb.limit_type = LimitType::Angle;
                pb.max_angle_x = stiffness_to_max_angle(stiffness);
            }
        }
        // Gravity: whichever of m_Gravity.y / m_Force.y is larger in magnitude, normalised by the
        // average bone length so the same "pull" reads the same on chains of any size.
        let (g, falloff) = if gravity_y.abs() >= force_y.abs() {
            (gravity_y, 1.0)
        } else {
            (force_y, 0.0)
        };
        let len = avg_bone_length.max(1e-5);
        pb.gravity = (-g * object_lossy_scale_x.abs() / len) + 0.0; // + 0.0 folds -0.0 into 0
        pb.gravity_falloff = falloff;
        pb.colliders = colliders;
        pb
    }

    /// Render the body.
    pub fn to_body(&self) -> String {
        let mut o = String::new();
        mono_head(&mut o, self.game_object, &VRC_PHYS_BONE);
        for k in [
            "foldout_transforms",
            "foldout_forces",
            "foldout_collision",
            "foldout_stretchsquish",
            "foldout_limits",
            "foldout_grabpose",
            "foldout_options",
        ] {
            o.push_str(&format!("  {k}: 1\n"));
        }
        o.push_str("  foldout_gizmos: 0\n");
        o.push_str(&format!("  version: {}\n", self.version));
        o.push_str(&format!("  integrationType: {}\n", self.integration_type));
        o.push_str(&format!(
            "  rootTransform: {}\n",
            local_ref(self.root_transform)
        ));
        if self.ignore_transforms.is_empty() {
            o.push_str("  ignoreTransforms: []\n");
        } else {
            o.push_str("  ignoreTransforms:\n");
            for t in &self.ignore_transforms {
                o.push_str(&format!("  - {}\n", local_ref(*t)));
            }
        }
        o.push_str(&format!(
            "  endpointPosition: {}\n",
            self.endpoint_position.to_yaml()
        ));
        o.push_str(&format!("  multiChildType: {}\n", self.multi_child_type));
        o.push_str(&format!("  pull: {}\n", fmt(self.pull)));
        self.pull_curve.render(&mut o, "pullCurve");
        o.push_str(&format!("  spring: {}\n", fmt(self.spring)));
        self.spring_curve.render(&mut o, "springCurve");
        o.push_str(&format!("  stiffness: {}\n", fmt(self.stiffness)));
        self.stiffness_curve.render(&mut o, "stiffnessCurve");
        o.push_str(&format!("  gravity: {}\n", fmt(self.gravity)));
        self.gravity_curve.render(&mut o, "gravityCurve");
        o.push_str(&format!(
            "  gravityFalloff: {}\n",
            fmt(self.gravity_falloff)
        ));
        self.gravity_falloff_curve
            .render(&mut o, "gravityFalloffCurve");
        o.push_str(&format!("  immobileType: {}\n", self.immobile_type));
        o.push_str(&format!("  immobile: {}\n", fmt(self.immobile)));
        self.immobile_curve.render(&mut o, "immobileCurve");
        o.push_str(&format!(
            "  allowCollision: {}\n",
            self.allow_collision as u8
        ));
        filter_block(&mut o, "collisionFilter", self.collision_filter);
        o.push_str(&format!("  radius: {}\n", fmt(self.radius)));
        self.radius_curve.render(&mut o, "radiusCurve");
        if self.colliders.is_empty() {
            o.push_str("  colliders: []\n");
        } else {
            o.push_str("  colliders:\n");
            for c in &self.colliders {
                o.push_str(&format!("  - {}\n", local_ref(*c)));
            }
        }
        o.push_str(&format!("  limitType: {}\n", self.limit_type as u8));
        o.push_str(&format!("  maxAngleX: {}\n", fmt(self.max_angle_x)));
        self.max_angle_x_curve.render(&mut o, "maxAngleXCurve");
        o.push_str(&format!("  maxAngleZ: {}\n", fmt(self.max_angle_z)));
        self.max_angle_z_curve.render(&mut o, "maxAngleZCurve");
        o.push_str(&format!(
            "  limitRotation: {}\n",
            self.limit_rotation.to_yaml()
        ));
        self.limit_rotation_curves[0].render(&mut o, "limitRotationXCurve");
        self.limit_rotation_curves[1].render(&mut o, "limitRotationYCurve");
        self.limit_rotation_curves[2].render(&mut o, "limitRotationZCurve");
        o.push_str(&format!(
            "  staticFreezeAxis: {}\n",
            self.static_freeze_axis.to_yaml()
        ));
        o.push_str(&format!("  allowGrabbing: {}\n", self.allow_grabbing as u8));
        filter_block(&mut o, "grabFilter", self.grab_filter);
        o.push_str(&format!("  allowPosing: {}\n", self.allow_posing as u8));
        filter_block(&mut o, "poseFilter", self.pose_filter);
        o.push_str(&format!("  snapToHand: {}\n", self.snap_to_hand as u8));
        o.push_str(&format!("  grabMovement: {}\n", fmt(self.grab_movement)));
        o.push_str(&format!("  maxStretch: {}\n", fmt(self.max_stretch)));
        self.max_stretch_curve.render(&mut o, "maxStretchCurve");
        o.push_str(&format!("  maxSquish: {}\n", fmt(self.max_squish)));
        self.max_squish_curve.render(&mut o, "maxSquishCurve");
        o.push_str(&format!("  stretchMotion: {}\n", fmt(self.stretch_motion)));
        self.stretch_motion_curve
            .render(&mut o, "stretchMotionCurve");
        o.push_str(&format!("  isAnimated: {}\n", self.is_animated as u8));
        o.push_str(&format!(
            "  resetWhenDisabled: {}\n",
            self.reset_when_disabled as u8
        ));
        o.push_str(&format!("  parameter: {}\n", self.parameter));
        o.push_str("  showGizmos: 1\n");
        o.push_str("  boneOpacity: 0.5\n");
        o.push_str("  limitOpacity: 0.5\n");
        o
    }
}

/// The SDK's `StiffToMaxAngle` table (DynamicBone stiffness 0..1 → PhysBone `maxAngleX`),
/// linearly interpolated (the SDK smooths tangents through the same points).
pub fn stiffness_to_max_angle(stiffness: f64) -> f64 {
    const TABLE: [(f64, f64); 11] = [
        (0.0, 180.0),
        (0.1, 129.0),
        (0.2, 106.0),
        (0.3, 89.0),
        (0.4, 74.0),
        (0.5, 60.0),
        (0.6, 47.0),
        (0.7, 35.0),
        (0.8, 23.0),
        (0.9, 11.0),
        (1.0, 0.0),
    ];
    let s = stiffness.clamp(0.0, 1.0);
    for w in TABLE.windows(2) {
        let (t0, v0) = w[0];
        let (t1, v1) = w[1];
        if s <= t1 {
            let f = ((s - t0) / (t1 - t0)).clamp(0.0, 1.0);
            return v0 + (v1 - v0) * f;
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use avatar_unity_yaml::UnityFile;

    fn parse_body(body: &str) -> UnityFile {
        let text = format!("%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!114 &1\n{body}");
        UnityFile::parse(&text).unwrap()
    }

    #[test]
    fn script_hashes_pinned() {
        assert!(verify_script_hashes());
        assert_eq!(
            VRC_AVATAR_DESCRIPTOR.render(),
            "{fileID: 542108242, guid: 67cc4cb7839cd3741b63733d5adf0442, type: 3}"
        );
    }

    #[test]
    fn physbone_spec_round_trips_through_yaml() {
        let mut pb = PhysBoneSpec::new(77);
        pb.root_transform = 5;
        pb.ignore_transforms = vec![8, 9];
        pb.pull = 0.35;
        pb.pull_curve = Curve::parse("0:0.5,1:1").unwrap();
        pb.spring_curve = Curve::parse("0:1,0.5:0.8,1:0.4").unwrap();
        pb.gravity = 0.15;
        pb.immobile_type = 1;
        pb.immobile = 0.4;
        pb.collision_filter = (true, false);
        pb.colliders = vec![11];
        pb.limit_type = LimitType::Angle;
        pb.max_angle_x = 60.0;
        pb.allow_grabbing = false;
        pb.grab_filter = (false, true);
        pb.snap_to_hand = true;
        pb.stretch_motion = 0.25;
        pb.is_animated = true;
        pb.reset_when_disabled = true;
        pb.parameter = "Hair".into();
        let body = pb.to_body();
        let file = parse_body(&body);
        let back = PhysBoneSpec::from_yaml(&file.documents[0].body);
        assert_eq!(back.game_object, 77);
        assert_eq!(back.root_transform, 5);
        assert_eq!(back.ignore_transforms, vec![8, 9]);
        assert_eq!(back.pull, 0.35);
        assert_eq!(back.pull_curve, pb.pull_curve);
        assert_eq!(back.spring_curve, pb.spring_curve);
        assert_eq!(back.gravity, 0.15);
        assert_eq!(back.immobile_type, 1);
        assert_eq!(back.collision_filter, (true, false));
        assert_eq!(back.colliders, vec![11]);
        assert_eq!(back.limit_type, LimitType::Angle);
        assert_eq!(back.max_angle_x, 60.0);
        assert!(!back.allow_grabbing);
        assert_eq!(back.grab_filter, (false, true));
        assert!(back.snap_to_hand);
        assert_eq!(back.stretch_motion, 0.25);
        assert!(back.is_animated && back.reset_when_disabled);
        assert_eq!(back.parameter, "Hair");
        // Idempotent: rendering the read-back spec gives the same text.
        assert_eq!(back.to_body(), body);
        // Linear segments: the middle spring key carries the two secants as its tangents.
        assert!(body.contains(
            "      time: 0.5\n      value: 0.8\n      inSlope: -0.4\n      outSlope: -0.8\n"
        ));
    }

    #[test]
    fn curve_parse_and_eval() {
        let c = Curve::parse("1:1, 0:0.5").unwrap();
        assert_eq!(c.0, vec![(0.0, 0.5), (1.0, 1.0)]);
        assert!((c.eval(0.5) - 0.75).abs() < 1e-12);
        assert_eq!(c.eval(-1.0), 0.5);
        assert_eq!(c.eval(2.0), 1.0);
        assert_eq!(Curve::NONE.eval(0.3), 1.0);
        assert_eq!(c.describe(), "0:0.5 → 1:1");
        assert!(Curve::parse("0:0.5,2:1").is_err());
        assert!(Curve::parse("nope").is_err());
    }

    #[test]
    fn stiffness_table_endpoints_and_interpolation() {
        assert_eq!(stiffness_to_max_angle(0.0), 180.0);
        assert_eq!(stiffness_to_max_angle(1.0), 0.0);
        assert_eq!(stiffness_to_max_angle(0.5), 60.0);
        let v = stiffness_to_max_angle(0.28);
        assert!((v - 92.4).abs() < 0.01, "{v}");
    }

    #[test]
    fn dynamic_bone_conversion_matches_sdk_rules() {
        let pb = PhysBoneSpec::from_dynamic_bone(
            10,
            20,
            vec![30],
            0.05,
            0.2,
            0.28,
            0.9,
            0.02,
            1.0,
            0.0,
            0.0,
            0,
            0.1,
            1.0,
            vec![40, 41],
        );
        assert_eq!(pb.pull, 0.05);
        assert!((pb.spring - 0.8).abs() < 1e-9);
        assert_eq!(pb.stiffness, 0.0);
        assert_eq!(pb.immobile, 0.9);
        assert_eq!(pb.immobile_type, 1);
        assert_eq!(pb.integration_type, 1);
        assert_eq!(pb.limit_type, LimitType::Angle);
        assert!((pb.max_angle_x - 92.4).abs() < 0.01);
        assert_eq!(pb.gravity, 0.0);
        let body = pb.to_body();
        let f = parse_body(&body);
        let d = &f.documents[0].body;
        assert_eq!(d["rootTransform"]["fileID"].as_i64(), Some(20));
        assert_eq!(d["ignoreTransforms"][0]["fileID"].as_i64(), Some(30));
        assert_eq!(d["colliders"].as_vec().unwrap().len(), 2);
        assert_eq!(d["m_Script"]["fileID"].as_i64(), Some(1661641543));
    }

    #[test]
    fn dynamic_bone_gravity_is_normalised_by_bone_length() {
        let pb = PhysBoneSpec::from_dynamic_bone(
            1,
            0,
            vec![],
            0.1,
            0.1,
            0.1,
            0.1,
            0.01,
            1.0,
            -0.02,
            0.0,
            0,
            0.05,
            1.0,
            vec![],
        );
        // gravity = -(-0.02) * 1 / 0.05 = 0.4, falloff 1 (came from m_Gravity).
        assert!((pb.gravity - 0.4).abs() < 1e-9);
        assert_eq!(pb.gravity_falloff, 1.0);
        // Freeze axis Y -> hinge on +Y.
        let pb = PhysBoneSpec::from_dynamic_bone(
            1,
            0,
            vec![],
            0.1,
            0.1,
            0.1,
            0.1,
            0.01,
            1.0,
            0.0,
            0.0,
            2,
            0.05,
            1.0,
            vec![],
        );
        assert_eq!(pb.limit_type, LimitType::Hinge);
        assert_eq!(pb.static_freeze_axis, Vec3::Y);
    }

    #[test]
    fn collider_conversion_shapes_and_orientation() {
        // Sphere: height <= 2r.
        let c = PhysBoneColliderSpec::from_dynamic_bone_collider(1, 1, 0, 0.07, 0.0, Vec3::ZERO);
        assert_eq!(c.shape, ColliderShape::Sphere);
        assert_eq!(c.rotation, Quat::IDENTITY);
        // Capsule along X -> 90 degrees about Z.
        let c =
            PhysBoneColliderSpec::from_capsule_collider(1, 0, 0.05, 0.4, Vec3::new(0.0, 0.1, 0.0));
        assert_eq!(c.shape, ColliderShape::Capsule);
        let up = c.rotation.rotate(Vec3::Y);
        assert!(
            (up.x.abs() - 1.0).abs() < 1e-9,
            "capsule axis should be X: {up:?}"
        );
        let body = c.to_body();
        let f = parse_body(&body);
        assert_eq!(f.documents[0].body["shapeType"].as_i64(), Some(1));
        assert_eq!(
            f.documents[0].body["m_Script"]["fileID"].as_i64(),
            Some(-1631200402)
        );
    }

    #[test]
    fn descriptor_body_parses_and_carries_fields() {
        let spec = DescriptorSpec {
            game_object: 5,
            view_position: Vec3::new(0.0, 0.995, 0.06),
            scale_ipd: true,
            lip_sync: LipSyncStyle::VisemeBlendShape,
            viseme_mesh: 77,
            viseme_blendshapes: vec!["vrc.v_sil".into(), "vrc.v_pp".into()],
            mouth_open_blendshape: "Facial_Blends.Jaw_Down".into(),
            portrait_camera_position_offset: Vec3::ZERO,
            portrait_camera_rotation_offset: Quat::IDENTITY,
            base_layers: [
                PlayableLayer::default(),
                PlayableLayer::default(),
                PlayableLayer::default(),
                PlayableLayer::default(),
                PlayableLayer {
                    controller: Some((9100000, "abcdefabcdefabcdefabcdefabcdefab".into())),
                },
            ],
            special_layers: Default::default(),
            expressions_menu: Some((11400000, "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1".into())),
            expression_parameters: Some((11400000, "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1".into())),
            eye_look: Some(EyeLook {
                left_eye: 100,
                right_eye: 101,
                straight: EyeRotations {
                    left: Quat::IDENTITY,
                    right: Quat::IDENTITY,
                },
                up: EyeRotations {
                    left: Quat::IDENTITY,
                    right: Quat::IDENTITY,
                },
                down: EyeRotations {
                    left: Quat::IDENTITY,
                    right: Quat::IDENTITY,
                },
                left: EyeRotations {
                    left: Quat::IDENTITY,
                    right: Quat::IDENTITY,
                },
                right: EyeRotations {
                    left: Quat::IDENTITY,
                    right: Quat::IDENTITY,
                },
                eyelid_blendshapes: Some((77, [12, -1, -1])),
            }),
        };
        let f = parse_body(&spec.to_body());
        let d = &f.documents[0].body;
        assert_eq!(d["m_Script"]["fileID"].as_i64(), Some(542108242));
        assert_eq!(d["lipSync"].as_i64(), Some(3));
        assert_eq!(d["VisemeSkinnedMesh"]["fileID"].as_i64(), Some(77));
        assert_eq!(d["VisemeBlendShapes"].as_vec().unwrap().len(), 2);
        assert_eq!(d["customExpressions"].as_i64(), Some(1));
        assert_eq!(d["enableEyeLook"].as_i64(), Some(1));
        assert_eq!(d["customEyeLookSettings"]["eyelidType"].as_i64(), Some(2));
        assert_eq!(
            d["customEyeLookSettings"]["eyelidsBlendshapes"].as_str(),
            Some("0c000000ffffffffffffffff")
        );
        let layers = d["baseAnimationLayers"].as_vec().unwrap();
        assert_eq!(layers.len(), 5);
        assert_eq!(layers[4]["type"].as_i64(), Some(5));
        assert_eq!(layers[4]["isDefault"].as_i64(), Some(0));
        assert_eq!(layers[0]["isDefault"].as_i64(), Some(1));
        assert_eq!(d["specialAnimationLayers"].as_vec().unwrap().len(), 3);
        assert_eq!(d["collider_head"]["state"].as_i64(), Some(0));
    }
}
