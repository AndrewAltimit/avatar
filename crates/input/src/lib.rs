//! Backend-agnostic VR tracker/controller input.
//!
//! The runtime (a VR viewport) reads a [`TrackerState`] each frame — HMD + two controllers +
//! optional extra trackers as 6-DoF transforms, plus analog triggers/grips/sticks — to drive an
//! avatar's pose (e.g. as IK targets via [`body_ik_targets`]). The *source* of that state is
//! abstracted by the [`TrackerSource`] trait so the viewport doesn't care whether it came from OSC,
//! OpenXR, or a recorded clip:
//!
//! - [`MockSource`] — a deterministic scripted source, for tests and headless dev (always available).
//! - [`osc::OscSource`] — a real UDP/OSC backend (VMC/tracker-style transform messages), behind the
//!   `osc` feature.
//! - An OpenXR action-set backend is the intended hardware path: it would implement the same
//!   [`TrackerSource`] trait and drop in without touching the viewport. Left for the on-device work
//!   (it needs the OpenXR loader + a headset, neither verifiable headless).
//!
//! This crate deals only in transforms (`glam`); it has no avatar/skeleton dependency. The glue that
//! turns a [`TrackerState`] into IK targets is the one place the two meet, and it's pure geometry.

use glam::{Quat, Vec3};

#[cfg(feature = "osc")]
pub mod osc;

/// A rigid 6-DoF pose (position + orientation). Defaults to the identity at the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose6dof {
    pub position: Vec3,
    pub orientation: Quat,
}

impl Default for Pose6dof {
    fn default() -> Self {
        Pose6dof {
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
        }
    }
}

/// A VR controller: a pose plus its analog inputs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Controller {
    pub pose: Pose6dof,
    /// Trigger pull, 0..1.
    pub trigger: f32,
    /// Grip squeeze, 0..1.
    pub grip: f32,
    /// Thumbstick / trackpad, each axis -1..1.
    pub stick: [f32; 2],
    /// Pressed-button bitmask (backend-defined).
    pub buttons: u32,
}

/// One frame of tracking: head, both hands, and any extra body trackers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackerState {
    pub hmd: Pose6dof,
    pub left: Controller,
    pub right: Controller,
    /// Extra 6-DoF trackers (waist/feet/etc.), backend order.
    pub trackers: Vec<Pose6dof>,
    /// Source timestamp in seconds (monotonic if the backend provides one).
    pub time: f64,
}

/// A source of tracking frames. Implemented by each backend; the viewport polls it once per frame.
pub trait TrackerSource {
    /// The latest tracking state. Should be non-blocking (drain what's available, return the most
    /// recent known state).
    fn poll(&mut self) -> TrackerState;
}

/// A deterministic scripted source: replays a list of frames, holding on the last. For tests and
/// headless development without hardware.
#[derive(Debug, Clone, Default)]
pub struct MockSource {
    frames: Vec<TrackerState>,
    index: usize,
}

impl MockSource {
    /// Replay these frames in order (the last is held once exhausted).
    pub fn new(frames: Vec<TrackerState>) -> Self {
        MockSource { frames, index: 0 }
    }

    /// A single static frame held forever.
    pub fn fixed(state: TrackerState) -> Self {
        MockSource::new(vec![state])
    }
}

impl TrackerSource for MockSource {
    fn poll(&mut self) -> TrackerState {
        if self.frames.is_empty() {
            return TrackerState::default();
        }
        let frame = self.frames[self.index].clone();
        if self.index + 1 < self.frames.len() {
            self.index += 1;
        }
        frame
    }
}

/// IK target for one limb: where the end effector should go, and a pole hint for the joint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IkTarget {
    pub position: Vec3,
    pub pole: Vec3,
}

/// Body IK targets derived from a tracking frame — feed these to `avatar_pose::ik::TwoBoneIk`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyIkTargets {
    pub left_hand: IkTarget,
    pub right_hand: IkTarget,
    /// Head position (drives neck/look or the camera).
    pub head: Vec3,
}

/// Map a tracking frame to arm IK targets: each hand target is the controller position; the pole
/// (elbow hint) sits below the controller in its own frame, so elbows bend down-and-out naturally.
///
/// # Example
///
/// ```
/// use avatar_input::{TrackerState, body_ik_targets};
/// use glam::Vec3;
///
/// let mut state = TrackerState::default();
/// state.left.pose.position = Vec3::new(-0.3, 1.0, 0.2);
/// state.hmd.position = Vec3::new(0.0, 1.6, 0.0);
///
/// let targets = body_ik_targets(&state);
/// assert_eq!(targets.left_hand.position, Vec3::new(-0.3, 1.0, 0.2));
/// assert_eq!(targets.head, Vec3::new(0.0, 1.6, 0.0));
/// // Identity controller orientation → the elbow pole drops 0.3 straight down from the hand.
/// assert_eq!(targets.left_hand.pole, Vec3::new(-0.3, 0.7, 0.2));
/// ```
pub fn body_ik_targets(state: &TrackerState) -> BodyIkTargets {
    let arm_pole =
        |c: &Controller| c.pose.position + c.pose.orientation * Vec3::new(0.0, -0.3, 0.0);
    BodyIkTargets {
        left_hand: IkTarget {
            position: state.left.pose.position,
            pole: arm_pole(&state.left),
        },
        right_hand: IkTarget {
            position: state.right.pose.position,
            pole: arm_pole(&state.right),
        },
        head: state.hmd.position,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_source_replays_then_holds() {
        let frame = |t: f64| TrackerState {
            time: t,
            ..Default::default()
        };
        let mut src = MockSource::new(vec![frame(0.0), frame(1.0)]);
        assert_eq!(src.poll().time, 0.0);
        assert_eq!(src.poll().time, 1.0);
        assert_eq!(src.poll().time, 1.0, "holds on the last frame");
    }

    #[test]
    fn empty_mock_source_yields_default() {
        let mut src = MockSource::default();
        assert_eq!(src.poll(), TrackerState::default());
    }

    #[test]
    fn ik_targets_track_controller_positions() {
        let mut state = TrackerState::default();
        state.left.pose.position = Vec3::new(-0.3, 1.0, 0.2);
        state.right.pose.position = Vec3::new(0.3, 1.0, 0.2);
        state.hmd.position = Vec3::new(0.0, 1.6, 0.0);
        let t = body_ik_targets(&state);
        assert_eq!(t.left_hand.position, Vec3::new(-0.3, 1.0, 0.2));
        assert_eq!(t.right_hand.position, Vec3::new(0.3, 1.0, 0.2));
        assert_eq!(t.head, Vec3::new(0.0, 1.6, 0.0));
        // Default (identity) controller orientation → pole drops straight down from the hand.
        assert_eq!(t.left_hand.pole, Vec3::new(-0.3, 0.7, 0.2));
    }
}
