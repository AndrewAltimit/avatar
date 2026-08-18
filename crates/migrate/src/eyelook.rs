//! Deriving SDK3 eye-look rotation states from the rig geometry.
//!
//! The SDK3 descriptor stores, for each look state (straight / up / down / left / right), the
//! **local rotation of each eye bone** — the value you'd get by rotating the eye in the
//! inspector and pressing "set". Authoring that by hand needs Unity; deriving it does not: a
//! look state is the rest orientation turned by a fixed angle about an *avatar-space* axis
//! (X for up/down, Y for left/right), and that turn expressed back in the eye's parent space is
//!
//! ```text
//! local_state = (R_parent⁻¹ · R_delta · R_parent) · local_rest
//! ```
//!
//! where `R_parent` is the eye's parent (Head) orientation in avatar space and `R_delta` the
//! avatar-space turn. This works no matter how the eye bone's own axes are rolled — which on
//! ripped/MMD-derived rigs (this crate's motivating case) they very much are.
//!
//! Sign conventions are Unity's: a negative rotation about +X pitches +Z (forward) *up*; a
//! negative rotation about +Y yaws forward to the avatar's *left* (−X).

use crate::math::{Quat, Vec3};
use crate::scene::Scene;
use crate::sdk3::{EyeLook, EyeRotations};

/// The look angles (degrees) to derive.
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct EyeLookAngles {
    pub up: f64,
    pub down: f64,
    pub left: f64,
    pub right: f64,
}

impl Default for EyeLookAngles {
    /// Modest angles that read as "eyes glancing" on a stylised face without rolling.
    fn default() -> Self {
        EyeLookAngles {
            up: 10.0,
            down: 10.0,
            left: 12.0,
            right: 12.0,
        }
    }
}

/// Derive the five look states for eye Transforms `left_eye` / `right_eye`.
///
/// `eyelid_blendshapes` is passed through (see [`EyeLook::eyelid_blendshapes`]).
pub fn derive_eye_look(
    scene: &Scene,
    left_eye: i64,
    right_eye: i64,
    angles: EyeLookAngles,
    eyelid_blendshapes: Option<(i64, [i32; 3])>,
) -> EyeLook {
    let state = |axis: Vec3, deg: f64| -> EyeRotations {
        EyeRotations {
            left: local_after_turn(scene, left_eye, axis, deg),
            right: local_after_turn(scene, right_eye, axis, deg),
        }
    };
    EyeLook {
        left_eye,
        right_eye,
        straight: state(Vec3::X, 0.0),
        up: state(Vec3::X, -angles.up),
        down: state(Vec3::X, angles.down),
        left: state(Vec3::Y, -angles.left),
        right: state(Vec3::Y, angles.right),
        eyelid_blendshapes,
    }
}

/// The eye's local rotation after turning it `deg` degrees about avatar-space `axis`.
fn local_after_turn(scene: &Scene, eye: i64, axis: Vec3, deg: f64) -> Quat {
    let Some(tr) = scene.transforms.get(&eye) else {
        return Quat::IDENTITY;
    };
    let rest = tr.local.rotation;
    if deg == 0.0 {
        return rest;
    }
    let parent_world = if tr.parent != 0 {
        scene.world(tr.parent).rotation
    } else {
        Quat::IDENTITY
    };
    let delta = Quat::axis_angle(axis, deg);
    (parent_world.inverse() * delta * parent_world * rest).normalized()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Trs;
    use crate::scene::{GameObject, Transform};
    use std::collections::HashMap;

    /// A tiny scene: root -> Head (rotated) -> Eye (with an arbitrary roll).
    fn scene(head_rot: Quat, eye_rot: Quat) -> Scene {
        let mut transforms = HashMap::new();
        let mut game_objects = HashMap::new();
        for (id, parent, children, rot) in [
            (1, 0, vec![2], Quat::IDENTITY),
            (2, 1, vec![3], head_rot),
            (3, 2, vec![], eye_rot),
        ] {
            transforms.insert(
                id,
                Transform {
                    file_id: id,
                    game_object: id + 100,
                    parent,
                    children,
                    local: Trs {
                        position: Vec3::ZERO,
                        rotation: rot,
                        scale: Vec3::ONE,
                    },
                },
            );
            game_objects.insert(
                id + 100,
                GameObject {
                    file_id: id + 100,
                    name: format!("n{id}"),
                    transform: id,
                    components: vec![id],
                    active: true,
                },
            );
        }
        Scene {
            docs: HashMap::new(),
            game_objects,
            transforms,
            roots: vec![1],
        }
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-6
    }

    #[test]
    fn looking_up_pitches_the_eye_forward_vector_up_regardless_of_bone_roll() {
        // Head yawed 30deg, eye bone rolled by a wild rotation (like an MMD-derived rig).
        let head = Quat::axis_angle(Vec3::Y, 30.0);
        let eye = Quat::axis_angle(Vec3::new(1.0, 2.0, 3.0), 123.0);
        let s = scene(head, eye);
        let el = derive_eye_look(&s, 3, 3, EyeLookAngles::default(), None);
        // Straight = rest local.
        assert_eq!(el.straight.left, eye);
        // Compose to world and compare the world "eye forward" (whatever the rolled bone's world
        // forward was at rest) before/after: it must have pitched up by 10deg about world X.
        let rest_world = head * eye;
        let up_world = head * el.up.left;
        let expected = Quat::axis_angle(Vec3::X, -10.0) * rest_world;
        let probe = Vec3::new(0.2, 0.3, 0.9);
        assert!(close(up_world.rotate(probe), expected.rotate(probe)));
        // And "left" is a −12deg yaw about world Y.
        let left_world = head * el.left.left;
        let expected = Quat::axis_angle(Vec3::Y, -12.0) * rest_world;
        assert!(close(left_world.rotate(probe), expected.rotate(probe)));
    }

    #[test]
    fn identity_rig_gives_plain_axis_rotations() {
        let s = scene(Quat::IDENTITY, Quat::IDENTITY);
        let el = derive_eye_look(&s, 3, 3, EyeLookAngles::default(), None);
        // eyesLookingUp: negative x component (matches the SDK's own sample sign).
        assert!(el.up.left.x < 0.0 && el.up.left.y.abs() < 1e-9);
        assert!(el.down.left.x > 0.0);
        assert!(el.left.left.y < 0.0);
        assert!(el.right.left.y > 0.0);
    }
}
