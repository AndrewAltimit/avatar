//! AnimationClip (`.anim`, Unity class id 74) generation.
//!
//! A Unity `.anim` is a single-document stream: one `AnimationClip` object. The interesting parts a
//! generator must get right are the **curve collections** and the `m_AnimationClipSettings` block.
//!
//! Unity stores curves in a few class-typed collections keyed by the *kind* of the animated value:
//!
//! - `m_FloatCurves` — scalar (`float`) curves: blendshape weights
//!   (`blendShape.<name>` on a SkinnedMeshRenderer, class id 137), material floats, single
//!   component fields. This is the collection we generate into.
//! - `m_PositionCurves` / `m_ScaleCurves` / `m_RotationCurves` / `m_EulerCurves` — `Vector3`/`Quat`
//!   transform curves. Out of scope here (those need `{x,y,z}` keyframes).
//! - `m_PPtrCurves` — object-reference curves (sprite swaps, material swaps).
//!
//! A GameObject **active** toggle is *also* a float curve: attribute `m_IsActive`, class id `1`
//! (GameObject), path = the object to toggle. Unity drives the bool from a 0/1 float curve. So both
//! of our worked cases — blendshape weight and active toggle — are `m_FloatCurves` entries; they
//! differ only in `(path, attribute, classID)`.
//!
//! Each `FloatCurve` carries an `m_Curve` with an `m_Curves` list of keyframes. A keyframe is
//! `{ time, value, inSlope, outSlope, ... }`; we also emit the `serializedVersion: 3`,
//! `m_PreInfinity`/`m_PostInfinity`/`m_RotationOrder` tail Unity writes on every curve. For step-y
//! toggles a slope of `0` gives constant-between-keys behaviour once tangents are flat; we leave
//! tangent *mode* at Unity's default (free) and let the slopes do the shaping, which is what
//! hand-authored ComboGesture-style clips do.

use crate::yaml_emit::{Emitter, fmt_f32};

/// Unity class ids referenced by generated curves.
pub const CLASS_GAMEOBJECT: i64 = 1;
pub const CLASS_SKINNED_MESH_RENDERER: i64 = 137;

/// One keyframe on an animation curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
}

impl Keyframe {
    /// A keyframe with flat tangents (in/out slope 0) — the common case for blendshape and toggle
    /// clips, which are linear/constant rather than eased.
    pub fn flat(time: f32, value: f32) -> Self {
        Keyframe {
            time,
            value,
            in_slope: 0.0,
            out_slope: 0.0,
        }
    }

    /// A keyframe with explicit tangents.
    pub fn new(time: f32, value: f32, in_slope: f32, out_slope: f32) -> Self {
        Keyframe {
            time,
            value,
            in_slope,
            out_slope,
        }
    }
}

/// A single scalar (`float`) curve: an entry in `m_FloatCurves`.
#[derive(Debug, Clone)]
pub struct FloatCurve {
    /// Transform path from the animator root to the animated object (e.g. `Body`, or
    /// `Armature/Hips/Accessory`). Empty string animates the root object itself.
    pub path: String,
    /// The serialized attribute, e.g. `blendShape.Smile`, `m_IsActive`, `material._Color.r`.
    pub attribute: String,
    /// The Unity class id of the component carrying the attribute (137 SkinnedMeshRenderer for
    /// blendshapes, 1 GameObject for `m_IsActive`).
    pub class_id: i64,
    /// Optional script GUID for a MonoBehaviour-typed binding (class 114). `None` for built-ins.
    pub script: Option<String>,
    pub keyframes: Vec<Keyframe>,
}

impl FloatCurve {
    /// A blendshape-weight curve: `blendShape.<name>` on a SkinnedMeshRenderer at `path`. Weight is
    /// in Unity's 0–100 blendshape range.
    pub fn blendshape(path: impl Into<String>, shape: &str, keyframes: Vec<Keyframe>) -> Self {
        FloatCurve {
            path: path.into(),
            attribute: format!("blendShape.{shape}"),
            class_id: CLASS_SKINNED_MESH_RENDERER,
            script: None,
            keyframes,
        }
    }

    /// A GameObject active-toggle curve: `m_IsActive` on the GameObject at `path`. Values are 0/1.
    pub fn game_object_active(path: impl Into<String>, keyframes: Vec<Keyframe>) -> Self {
        FloatCurve {
            path: path.into(),
            attribute: "m_IsActive".to_string(),
            class_id: CLASS_GAMEOBJECT,
            script: None,
            keyframes,
        }
    }
}

/// `m_AnimationClipSettings` — the playback envelope. We expose the fields that matter for
/// expression/gesture clips; the rest take Unity's defaults.
#[derive(Debug, Clone)]
pub struct ClipSettings {
    pub loop_time: bool,
    pub start_time: f32,
    pub stop_time: f32,
}

impl Default for ClipSettings {
    fn default() -> Self {
        ClipSettings {
            loop_time: false,
            start_time: 0.0,
            stop_time: 1.0 / 60.0,
        }
    }
}

/// An AnimationClip to be emitted as a `.anim`.
#[derive(Debug, Clone)]
pub struct AnimationClip {
    pub name: String,
    pub float_curves: Vec<FloatCurve>,
    pub settings: ClipSettings,
}

impl AnimationClip {
    /// Start a new clip with the given name and no curves.
    pub fn new(name: impl Into<String>) -> Self {
        AnimationClip {
            name: name.into(),
            float_curves: Vec::new(),
            settings: ClipSettings::default(),
        }
    }

    /// Add an arbitrary float curve. Builder-style (consumes and returns `self`).
    pub fn float_curve(mut self, curve: FloatCurve) -> Self {
        self.add_float_curve(curve);
        self
    }

    /// Add a float curve in place.
    pub fn add_float_curve(&mut self, curve: FloatCurve) {
        // Keep `stop_time` covering the latest keyframe so the clip's length matches its data
        // (Unity uses `m_StopTime` as the clip length). A clip with all keys at t=0 still needs a
        // tiny non-zero length (one frame at 60fps) or Unity treats it as zero-length.
        let latest = curve
            .keyframes
            .iter()
            .map(|k| k.time)
            .fold(0.0_f32, f32::max);
        if latest > self.settings.stop_time {
            self.settings.stop_time = latest;
        }
        self.float_curves.push(curve);
    }

    /// Override the clip settings.
    pub fn with_settings(mut self, settings: ClipSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Emit the AnimationClip document body (everything after the `--- !u!74 &<id>` header) into
    /// `e`, at the emitter's current indent. The header is emitted by the caller (the document
    /// owns its file id, allocated by the [`crate::IdGen`]).
    pub fn emit_body(&self, e: &mut Emitter) {
        e.line("AnimationClip:");
        e.indented(|e| {
            e.kv_i64("m_ObjectHideFlags", 0);
            e.kv("m_CorrespondingSourceObject", "{fileID: 0}");
            e.kv("m_PrefabInstance", "{fileID: 0}");
            e.kv("m_PrefabAsset", "{fileID: 0}");
            e.kv("m_Name", &self.name);
            e.kv_i64("serializedVersion", 6);
            e.kv_i64("m_Legacy", 0);
            e.kv_i64("m_Compressed", 0);
            e.kv_i64("m_UseHighQualityCurve", 1);
            // Rotation/position/scale/euler curves: empty (we only generate float curves).
            e.kv("m_RotationCurves", "[]");
            e.kv("m_CompressedRotationCurves", "[]");
            e.kv("m_EulerCurves", "[]");
            e.kv("m_PositionCurves", "[]");
            e.kv("m_ScaleCurves", "[]");
            // The float curves.
            if self.float_curves.is_empty() {
                e.kv("m_FloatCurves", "[]");
            } else {
                e.key("m_FloatCurves");
                for c in &self.float_curves {
                    emit_float_curve(e, c);
                }
            }
            e.kv("m_PPtrCurves", "[]");
            e.kv("m_SampleRate", "60");
            e.kv_i64("m_WrapMode", 0);
            e.key("m_Bounds");
            e.indented(|e| {
                e.kv("m_Center", "{x: 0, y: 0, z: 0}");
                e.kv("m_Extent", "{x: 0, y: 0, z: 0}");
            });
            // m_ClipBindingConstant + editor curves are optional; Unity rebuilds the binding
            // constant on import, so we omit them and let the importer regenerate.
            emit_settings(e, &self.settings);
        });
    }

    /// Render the full `.anim` file: preamble + `--- !u!74` header + body.
    pub fn to_unity_yaml(&self, file_id: i64) -> String {
        let mut e = Emitter::new();
        e.doc_header(74, file_id);
        self.emit_body(&mut e);
        format!("{}{}", crate::yaml_emit::UNITY_PREAMBLE, e.into_string())
    }
}

fn emit_float_curve(e: &mut Emitter, c: &FloatCurve) {
    // Sequence entry: `- serializedVersion: ...` at the parent indent, body one level in.
    e.line("- serializedVersion: 2");
    e.indented(|e| {
        e.key("curve");
        e.indented(|e| {
            e.kv_i64("serializedVersion", 2);
            if c.keyframes.is_empty() {
                e.kv("m_Curve", "[]");
            } else {
                e.key("m_Curve");
                for k in &c.keyframes {
                    emit_keyframe(e, *k);
                }
            }
            e.kv_i64("m_PreInfinity", 2);
            e.kv_i64("m_PostInfinity", 2);
            e.kv_i64("m_RotationOrder", 4);
        });
        e.kv("attribute", &c.attribute);
        e.kv("path", &c.path);
        e.kv_i64("classID", c.class_id);
        match &c.script {
            Some(guid) => e.kv(
                "script",
                &format!("{{fileID: 11500000, guid: {guid}, type: 3}}"),
            ),
            None => e.kv("script", "{fileID: 0}"),
        }
    });
}

fn emit_keyframe(e: &mut Emitter, k: Keyframe) {
    // `- serializedVersion: 3` introduces a keyframe; its fields follow one indent in.
    e.line("- serializedVersion: 3");
    e.indented(|e| {
        e.kv("time", &fmt_f32(k.time));
        e.kv("value", &fmt_f32(k.value));
        e.kv("inSlope", &fmt_f32(k.in_slope));
        e.kv("outSlope", &fmt_f32(k.out_slope));
        e.kv_i64("tangentMode", 0);
        e.kv_i64("weightedMode", 0);
        e.kv("inWeight", "0.33333334");
        e.kv("outWeight", "0.33333334");
    });
}

fn emit_settings(e: &mut Emitter, s: &ClipSettings) {
    e.key("m_AnimationClipSettings");
    e.indented(|e| {
        e.kv_i64("serializedVersion", 2);
        e.kv("m_AdditiveReferencePoseClip", "{fileID: 0}");
        e.kv_f32("m_AdditiveReferencePoseTime", 0.0);
        e.kv_f32("m_StartTime", s.start_time);
        e.kv_f32("m_StopTime", s.stop_time);
        e.kv_f32("m_OrientationOffsetY", 0.0);
        e.kv_f32("m_Level", 0.0);
        e.kv_f32("m_CycleOffset", 0.0);
        e.kv_i64("m_HasAdditiveReferencePose", 0);
        e.kv_i64("m_LoopTime", s.loop_time as i64);
        e.kv_i64("m_LoopBlend", 0);
        e.kv_i64("m_LoopBlendOrientation", 0);
        e.kv_i64("m_LoopBlendPositionY", 0);
        e.kv_i64("m_LoopBlendPositionXZ", 0);
        e.kv_i64("m_KeepOriginalOrientation", 0);
        e.kv_i64("m_KeepOriginalPositionY", 1);
        e.kv_i64("m_KeepOriginalPositionXZ", 0);
        e.kv_i64("m_HeightFromFeet", 0);
        e.kv_i64("m_Mirror", 0);
    });
}
