//! Reading the SDK2-era avatar out of a prefab: the `VRC_AvatarDescriptor` (SDK2), the
//! `PipelineManager`, DynamicBone / DynamicBoneCollider, Unity `Cloth` + `CapsuleCollider`, and
//! the root `Animator`. Recognition is **structural** (by the fields a component serializes),
//! not by script GUID — an SDK2 export may carry any vintage of `VRCSDK2.dll`/`DynamicBone.cs`
//! and the fields are what stayed stable.

use anyhow::{Context, Result};
use avatar_unity_yaml::{Yaml, field_f64, field_i64, field_str};

use crate::math::{Quat, Vec3};
use crate::scene::{ANIMATOR, CAMERA, CAPSULE_COLLIDER, CLOTH, MONO_BEHAVIOUR, Scene, quat, vec3};

/// The SDK2 `VRC_AvatarDescriptor` fields the SDK3 descriptor inherits.
#[derive(Debug, Clone)]
pub struct Sdk2Descriptor {
    pub file_id: i64,
    pub game_object: i64,
    pub view_position: Vec3,
    pub scale_ipd: bool,
    /// SDK2 `lipSync` enum (same numbering as SDK3: 3 = VisemeBlendShape).
    pub lip_sync: i64,
    pub viseme_mesh: i64,
    pub viseme_blendshapes: Vec<String>,
    pub mouth_open_blendshape: String,
    pub portrait_camera_position_offset: Vec3,
    pub portrait_camera_rotation_offset: Quat,
    /// `CustomStandingAnims` override controller `{fileID, guid}`.
    pub custom_standing_anims: Option<(i64, String)>,
    /// `CustomSittingAnims` override controller `{fileID, guid}`.
    pub custom_sitting_anims: Option<(i64, String)>,
}

/// A DynamicBone component.
#[derive(Debug, Clone)]
pub struct DynamicBoneInfo {
    pub file_id: i64,
    pub game_object: i64,
    pub enabled: bool,
    /// `m_Root` Transform fileID (0 = unset → the component's own transform).
    pub root: i64,
    pub exclusions: Vec<i64>,
    pub damping: f64,
    pub elasticity: f64,
    pub stiffness: f64,
    pub inert: f64,
    pub radius: f64,
    pub end_length: f64,
    pub end_offset: Vec3,
    pub gravity: Vec3,
    pub force: Vec3,
    /// 0 none, 1 X, 2 Y, 3 Z.
    pub freeze_axis: i64,
    /// DynamicBoneCollider component fileIDs.
    pub colliders: Vec<i64>,
}

/// A DynamicBoneCollider component.
#[derive(Debug, Clone)]
pub struct DynamicBoneColliderInfo {
    pub file_id: i64,
    pub game_object: i64,
    /// 0 X, 1 Y, 2 Z.
    pub direction: i64,
    pub center: Vec3,
    /// 0 outside, 1 inside.
    pub bound: i64,
    pub radius: f64,
    pub height: f64,
}

/// A Unity `CapsuleCollider` (class 136).
#[derive(Debug, Clone)]
pub struct CapsuleColliderInfo {
    pub file_id: i64,
    pub game_object: i64,
    pub radius: f64,
    pub height: f64,
    /// 0 X, 1 Y, 2 Z.
    pub direction: i64,
    pub center: Vec3,
}

/// The root `Animator`.
#[derive(Debug, Clone)]
pub struct AnimatorInfo {
    pub file_id: i64,
    pub apply_root_motion: bool,
    /// `m_Avatar` `{fileID, guid}` (the humanoid Avatar inside the FBX).
    pub avatar: Option<(i64, String)>,
}

/// Everything the migration reads off the SDK2 prefab.
#[derive(Debug, Clone)]
pub struct Sdk2Avatar {
    pub root_transform: i64,
    pub root_game_object: i64,
    pub animator: Option<AnimatorInfo>,
    pub descriptor: Option<Sdk2Descriptor>,
    /// `PipelineManager` component fileID (SDK2's, on the root).
    pub pipeline_manager: Option<i64>,
    pub dynamic_bones: Vec<DynamicBoneInfo>,
    pub dynamic_bone_colliders: Vec<DynamicBoneColliderInfo>,
    pub capsule_colliders: Vec<CapsuleColliderInfo>,
    /// `Cloth` component fileIDs.
    pub cloths: Vec<i64>,
    /// `Camera` component fileIDs.
    pub cameras: Vec<i64>,
}

impl Sdk2Avatar {
    /// Read the SDK2 avatar out of a parsed prefab graph.
    pub fn read(scene: &Scene) -> Result<Self> {
        let root = scene.root()?;
        let root_transform = root.file_id;
        let root_game_object = root.game_object;
        let root_go = scene
            .game_objects
            .get(&root_game_object)
            .context("root Transform has no GameObject")?;

        let mut animator = None;
        for c in &root_go.components {
            if let Some(d) = scene.doc(*c)
                && d.class_id == ANIMATOR
            {
                animator = Some(AnimatorInfo {
                    file_id: *c,
                    apply_root_motion: field_i64(&d.body, "m_ApplyRootMotion").unwrap_or(0) != 0,
                    avatar: asset_ref(&d.body["m_Avatar"]),
                });
            }
        }

        let mut descriptor = None;
        let mut pipeline_manager = None;
        let mut dynamic_bones = Vec::new();
        let mut dynamic_bone_colliders = Vec::new();
        for (d, _guid) in scene.monobehaviours() {
            let b = &d.body;
            let go = field_i64(&b["m_GameObject"], "fileID").unwrap_or(0);
            if is_sdk2_descriptor(b) {
                descriptor = Some(Sdk2Descriptor {
                    file_id: d.file_id,
                    game_object: go,
                    view_position: vec3(&b["ViewPosition"]),
                    scale_ipd: field_i64(b, "ScaleIPD").unwrap_or(1) != 0,
                    lip_sync: field_i64(b, "lipSync").unwrap_or(0),
                    viseme_mesh: field_i64(&b["VisemeSkinnedMesh"], "fileID").unwrap_or(0),
                    viseme_blendshapes: b["VisemeBlendShapes"]
                        .as_vec()
                        .map(|v| {
                            v.iter()
                                .map(|s| s.as_str().unwrap_or("").to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                    mouth_open_blendshape: field_str(b, "MouthOpenBlendShapeName")
                        .unwrap_or("")
                        .to_string(),
                    portrait_camera_position_offset: vec3(&b["portraitCameraPositionOffset"]),
                    portrait_camera_rotation_offset: quat(&b["portraitCameraRotationOffset"]),
                    custom_standing_anims: asset_ref(&b["CustomStandingAnims"]),
                    custom_sitting_anims: asset_ref(&b["CustomSittingAnims"]),
                });
            } else if is_pipeline_manager(b) {
                pipeline_manager = Some(d.file_id);
            } else if is_dynamic_bone(b) {
                dynamic_bones.push(DynamicBoneInfo {
                    file_id: d.file_id,
                    game_object: go,
                    enabled: field_i64(b, "m_Enabled").unwrap_or(1) != 0,
                    root: field_i64(&b["m_Root"], "fileID").unwrap_or(0),
                    exclusions: id_list(&b["m_Exclusions"]),
                    damping: field_f64(b, "m_Damping").unwrap_or(0.1),
                    elasticity: field_f64(b, "m_Elasticity").unwrap_or(0.1),
                    stiffness: field_f64(b, "m_Stiffness").unwrap_or(0.1),
                    inert: field_f64(b, "m_Inert").unwrap_or(0.0),
                    radius: field_f64(b, "m_Radius").unwrap_or(0.0),
                    end_length: field_f64(b, "m_EndLength").unwrap_or(0.0),
                    end_offset: vec3(&b["m_EndOffset"]),
                    gravity: vec3(&b["m_Gravity"]),
                    force: vec3(&b["m_Force"]),
                    freeze_axis: field_i64(b, "m_FreezeAxis").unwrap_or(0),
                    colliders: id_list(&b["m_Colliders"]),
                });
            } else if is_dynamic_bone_collider(b) {
                dynamic_bone_colliders.push(DynamicBoneColliderInfo {
                    file_id: d.file_id,
                    game_object: go,
                    direction: field_i64(b, "m_Direction").unwrap_or(1),
                    center: vec3(&b["m_Center"]),
                    bound: field_i64(b, "m_Bound").unwrap_or(0),
                    radius: field_f64(b, "m_Radius").unwrap_or(0.0),
                    height: field_f64(b, "m_Height").unwrap_or(0.0),
                });
            }
        }

        let mut capsule_colliders = Vec::new();
        let mut cloths = Vec::new();
        let mut cameras = Vec::new();
        for d in scene.docs.values() {
            let go = field_i64(&d.body["m_GameObject"], "fileID").unwrap_or(0);
            match d.class_id {
                CAPSULE_COLLIDER => capsule_colliders.push(CapsuleColliderInfo {
                    file_id: d.file_id,
                    game_object: go,
                    radius: field_f64(&d.body, "m_Radius").unwrap_or(0.5),
                    height: field_f64(&d.body, "m_Height").unwrap_or(1.0),
                    direction: field_i64(&d.body, "m_Direction").unwrap_or(1),
                    center: vec3(&d.body["m_Center"]),
                }),
                CLOTH => cloths.push(d.file_id),
                CAMERA => cameras.push(d.file_id),
                _ => {}
            }
        }
        // Deterministic order for reports and generated ids.
        dynamic_bones.sort_by_key(|d| d.file_id);
        dynamic_bone_colliders.sort_by_key(|d| d.file_id);
        capsule_colliders.sort_by_key(|d| d.file_id);
        cloths.sort();
        cameras.sort();

        Ok(Sdk2Avatar {
            root_transform,
            root_game_object,
            animator,
            descriptor,
            pipeline_manager,
            dynamic_bones,
            dynamic_bone_colliders,
            capsule_colliders,
            cloths,
            cameras,
        })
    }
}

/// SDK2 `VRC_AvatarDescriptor`: has the viseme/view fields *and* the SDK2-only override slots
/// (`CustomStandingAnims`); an SDK3 descriptor has `baseAnimationLayers` instead.
pub fn is_sdk2_descriptor(b: &Yaml) -> bool {
    !b["ViewPosition"].is_badvalue()
        && !b["VisemeBlendShapes"].is_badvalue()
        && !b["CustomStandingAnims"].is_badvalue()
        && b["baseAnimationLayers"].is_badvalue()
}

/// SDK3 `VRCAvatarDescriptor` (for "already migrated" detection).
pub fn is_sdk3_descriptor(b: &Yaml) -> bool {
    !b["ViewPosition"].is_badvalue() && !b["baseAnimationLayers"].is_badvalue()
}

/// `VRC.Core.PipelineManager`.
pub fn is_pipeline_manager(b: &Yaml) -> bool {
    !b["blueprintId"].is_badvalue() && !b["completedSDKPipeline"].is_badvalue()
}

/// DynamicBone: root + the four physics scalars.
pub fn is_dynamic_bone(b: &Yaml) -> bool {
    !b["m_Root"].is_badvalue()
        && !b["m_Damping"].is_badvalue()
        && !b["m_Elasticity"].is_badvalue()
        && !b["m_Stiffness"].is_badvalue()
        && !b["m_Inert"].is_badvalue()
}

/// DynamicBoneCollider: direction/center/bound/radius/height, and no `m_Root`.
pub fn is_dynamic_bone_collider(b: &Yaml) -> bool {
    b["m_Root"].is_badvalue()
        && !b["m_Direction"].is_badvalue()
        && !b["m_Center"].is_badvalue()
        && !b["m_Bound"].is_badvalue()
        && !b["m_Radius"].is_badvalue()
        && !b["m_Height"].is_badvalue()
}

fn asset_ref(node: &Yaml) -> Option<(i64, String)> {
    let id = field_i64(node, "fileID")?;
    if id == 0 {
        return None;
    }
    let guid = field_str(node, "guid")?.to_string();
    Some((id, guid))
}

fn id_list(node: &Yaml) -> Vec<i64> {
    node.as_vec()
        .map(|v| {
            v.iter()
                .filter_map(|c| field_i64(c, "fileID"))
                .filter(|id| *id != 0)
                .collect()
        })
        .unwrap_or_default()
}

/// Which components a MonoBehaviour body is (for reports).
pub fn classify(b: &Yaml, class_id: u32) -> &'static str {
    match class_id {
        MONO_BEHAVIOUR if is_sdk2_descriptor(b) => "VRC_AvatarDescriptor (SDK2)",
        MONO_BEHAVIOUR if is_sdk3_descriptor(b) => "VRCAvatarDescriptor (SDK3)",
        MONO_BEHAVIOUR if is_pipeline_manager(b) => "PipelineManager",
        MONO_BEHAVIOUR if is_dynamic_bone(b) => "DynamicBone",
        MONO_BEHAVIOUR if is_dynamic_bone_collider(b) => "DynamicBoneCollider",
        MONO_BEHAVIOUR => "MonoBehaviour",
        CLOTH => "Cloth",
        CAPSULE_COLLIDER => "CapsuleCollider",
        CAMERA => "Camera",
        ANIMATOR => "Animator",
        _ => "other",
    }
}
