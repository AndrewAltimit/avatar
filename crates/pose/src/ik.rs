//! Analytic two-bone inverse kinematics — bend a `root → mid → end` chain so `end` reaches a
//! target, with the joint (elbow/knee) steered toward a pole vector.
//!
//! The solve is **geometric**, not angle-delta: it computes the desired elbow position from the
//! law of cosines, then uses [`glam::Quat::from_rotation_arc`] to swing the root onto the elbow and
//! the mid onto the target. That makes it robust to the axis/sign bugs that plague the
//! interior-angle formulation — `from_rotation_arc` derives the correct axis and direction itself.

use glam::{Mat4, Quat, Vec3};

use crate::{Pose, PosedSkeleton};

/// A two-bone IK chain, addressed by compact bone indices in a [`PosedSkeleton`].
#[derive(Debug, Clone, Copy)]
pub struct TwoBoneIk {
    /// Upper joint (shoulder/hip).
    pub root: usize,
    /// Middle joint (elbow/knee) — bends toward the pole.
    pub mid: usize,
    /// End effector (wrist/ankle) — driven to the target.
    pub end: usize,
}

impl TwoBoneIk {
    /// Solve in place, writing new local rotations into `pose`. `target` and `pole` are in the same
    /// (world) space as [`PosedSkeleton::world_matrices`]. Out-of-reach targets clamp to the chain's
    /// extension; a degenerate target (coincident with the root) is left untouched.
    pub fn solve(&self, posed: &PosedSkeleton, pose: &mut Pose, target: Vec3, pole: Vec3) {
        const EPS: f32 = 1e-5;

        let world = posed.world_matrices(pose);
        let a = origin(&world[self.root]);
        let b = origin(&world[self.mid]);
        let c = origin(&world[self.end]);

        let len_ab = (b - a).length();
        let len_cb = (c - b).length();
        if len_ab < EPS || len_cb < EPS {
            return;
        }

        let to_target = target - a;
        if to_target.length() < EPS {
            return; // target on the root: nothing well-defined to solve.
        }
        let dir = to_target.normalize();
        // Clamp the reach into the chain's annulus [|l1-l2|, l1+l2].
        let reach = to_target
            .length()
            .clamp((len_ab - len_cb).abs() + EPS, len_ab + len_cb - EPS);
        let target = a + dir * reach;

        // Desired elbow: project distance `x` along `dir`, height `h` perpendicular, toward `pole`.
        let x = (len_ab * len_ab - len_cb * len_cb + reach * reach) / (2.0 * reach);
        let h = (len_ab * len_ab - x * x).max(0.0).sqrt();
        let pole_v = pole - a;
        let mut perp = pole_v - dir * pole_v.dot(dir);
        if perp.length() < EPS {
            // Pole degenerate (parallel to `dir`): any perpendicular will do.
            perp = dir.cross(Vec3::Y);
            if perp.length() < EPS {
                perp = dir.cross(Vec3::X);
            }
        }
        let elbow = a + dir * x + perp.normalize() * h;

        // 1) Swing the root so the mid lands on the desired elbow.
        rotate_joint(
            posed,
            pose,
            self.root,
            Quat::from_rotation_arc((b - a).normalize(), (elbow - a).normalize()),
        );
        // 2) Swing the mid so the end lands on the target.
        let world = posed.world_matrices(pose);
        let b2 = origin(&world[self.mid]);
        let c2 = origin(&world[self.end]);
        rotate_joint(
            posed,
            pose,
            self.mid,
            Quat::from_rotation_arc((c2 - b2).normalize(), (target - b2).normalize()),
        );
    }
}

fn origin(m: &Mat4) -> Vec3 {
    m.w_axis.truncate()
}

/// Apply a world-space rotation `delta` about `joint`'s origin, writing the result back as a local
/// rotation (translation/scale of the joint's local transform preserved, so its origin is fixed and
/// its children swing about it).
fn rotate_joint(posed: &PosedSkeleton, pose: &mut Pose, joint: usize, delta: Quat) {
    let world = posed.world_matrices(pose);
    let parent_rot = match posed.parent[joint] {
        Some(p) => world[p].to_scale_rotation_translation().1,
        None => Quat::IDENTITY,
    };
    let world_rot = world[joint].to_scale_rotation_translation().1;
    let new_local_rot = parent_rot.inverse() * (delta * world_rot);

    let (s, _r, t) = pose.local[joint].to_scale_rotation_translation();
    pose.local[joint] = Mat4::from_scale_rotation_translation(s, new_local_rot, t);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3-bone chain laid out along +X: root@(0,0,0), mid@(1,0,0), end@(2,0,0). Segment lengths 1.
    fn chain() -> PosedSkeleton {
        let bind = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        PosedSkeleton::from_parts(
            vec![0, 1, 2],
            vec![None, Some(0), Some(1)],
            vec![bind(0.0), bind(1.0), bind(2.0)],
            vec![
                bind(0.0).inverse(),
                bind(1.0).inverse(),
                bind(2.0).inverse(),
            ],
        )
    }

    fn end_pos(posed: &PosedSkeleton, pose: &Pose) -> Vec3 {
        origin(&posed.world_matrices(pose)[2])
    }

    #[test]
    fn reachable_target_is_hit() {
        let posed = chain();
        let mut pose = posed.rest_pose();
        let target = Vec3::new(0.5, 1.2, 0.0); // within reach (|.|≈1.3 < 2)
        let ik = TwoBoneIk {
            root: 0,
            mid: 1,
            end: 2,
        };
        ik.solve(&posed, &mut pose, target, Vec3::Y);
        assert!(
            (end_pos(&posed, &pose) - target).length() < 1e-4,
            "end effector should reach the target"
        );
    }

    #[test]
    fn unreachable_target_extends_toward_it() {
        let posed = chain();
        let mut pose = posed.rest_pose();
        let target = Vec3::new(10.0, 0.0, 0.0); // far out of reach
        TwoBoneIk {
            root: 0,
            mid: 1,
            end: 2,
        }
        .solve(&posed, &mut pose, target, Vec3::Y);
        let end = end_pos(&posed, &pose);
        // Fully extended: ~2 units from the root, pointing at the target.
        assert!((end.length() - 2.0).abs() < 1e-3, "chain is fully extended");
        assert!(end.x > 1.9, "extension points toward the target");
    }

    #[test]
    fn degenerate_target_does_not_nan() {
        let posed = chain();
        let mut pose = posed.rest_pose();
        TwoBoneIk {
            root: 0,
            mid: 1,
            end: 2,
        }
        .solve(&posed, &mut pose, Vec3::ZERO, Vec3::Y);
        assert!(end_pos(&posed, &pose).is_finite());
    }
}
