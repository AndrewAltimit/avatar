//! `avatar-osc-gestures` — the analog-gesture daemon (PLAN §4, the "Vive advanced controls on any
//! hardware" feature).
//!
//! VRChat already exposes the analog signal a Fist blend tree blends on: `GestureLeftWeight` /
//! `GestureRightWeight` are floats 0→1, and `GestureLeft` / `GestureRight` are ints 0–7 selecting
//! *which* gesture. On Vive wands the trigger pull drives those automatically; on most other
//! hardware it doesn't. This daemon closes that gap: it reads a controller's analog **trigger** (and
//! optionally **grip**), maps it to a gesture + weight, and sends them over OSC — so any headset
//! gets the same fractional-gesture control.
//!
//! ## Shape
//!
//! - [`AnalogSource`] is where the input comes from: per-hand `trigger`/`grip` floats. Deliberately
//!   **glam-free and minimal** (no 6-DoF pose) so this crate — and the `avatar` CLI that drives it —
//!   stay out of the `glam` graph. The on-device path is an adapter from an OpenXR / `avatar-input`
//!   [`Controller`](https://docs.rs/) into an [`AnalogState`]; [`DemoSource`] and [`ScriptedSource`]
//!   cover headless demos and tests.
//! - [`GestureConfig`] is the **pure** mapping: trigger → ([`Gesture`], weight) per hand, with a
//!   deadzone and optional grip gesture. Unit-tested in isolation.
//! - [`GestureDaemon`] is the loop: poll the source, resolve the frame, and emit only the parameters
//!   that **changed** through a [`ParamSink`] (implemented for [`avatar_osc::ParamClient`]).
//!
//! ```no_run
//! use avatar_osc_gestures::{DemoSource, GestureDaemon};
//! use avatar_osc::ParamClient;
//! use std::time::Duration;
//!
//! let client = ParamClient::connect_default()?;
//! let mut daemon = GestureDaemon::new(DemoSource::new(120));
//! daemon.run(&client, Duration::from_millis(10))?; // 100 Hz, until Ctrl-C
//! # anyhow::Ok(())
//! ```

use std::time::{Duration, Instant};

use anyhow::Result;
use avatar_osc::{ParamClient, ParamValue};

/// A VRChat hand gesture. The int value is what `GestureLeft` / `GestureRight` carry; VRChat's
/// default Fist blend tree is the one that actually reads the analog *weight*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Neutral,
    Fist,
    HandOpen,
    FingerPoint,
    Victory,
    RockNRoll,
    HandGun,
    ThumbsUp,
}

impl Gesture {
    /// The integer VRChat uses for this gesture (0–7).
    pub fn as_int(self) -> i32 {
        match self {
            Gesture::Neutral => 0,
            Gesture::Fist => 1,
            Gesture::HandOpen => 2,
            Gesture::FingerPoint => 3,
            Gesture::Victory => 4,
            Gesture::RockNRoll => 5,
            Gesture::HandGun => 6,
            Gesture::ThumbsUp => 7,
        }
    }
}

/// The analog inputs for one hand. Values outside `0..=1` are clamped by the mapping.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HandInput {
    /// Trigger pull, 0..1 — the analog axis driving the trigger gesture's weight.
    pub trigger: f32,
    /// Grip squeeze, 0..1 — optionally activates a separate static gesture.
    pub grip: f32,
}

/// One frame of both hands' analog inputs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AnalogState {
    pub left: HandInput,
    pub right: HandInput,
}

/// A source of analog hand input, polled once per tick. Backend-agnostic: an OpenXR action set, a
/// VMC/OSC feed, an `avatar-input` adapter, or a scripted clip each implement this.
pub trait AnalogSource {
    /// The latest analog state. Should be non-blocking (return the most recent known state).
    fn poll(&mut self) -> AnalogState;
}

/// How one hand's analog inputs map to a gesture + weight.
#[derive(Debug, Clone, Copy)]
pub struct HandMapping {
    /// The gesture the trigger drives; its weight is the (deadzoned, rescaled) trigger pull.
    pub trigger_gesture: Gesture,
    /// An optional static gesture the grip activates when squeezed past `grip_threshold`. It takes
    /// priority over the trigger gesture and reports full weight. `None` (the default) means the
    /// grip is ignored — pure trigger-to-Fist analog, the headline behaviour.
    pub grip_gesture: Option<Gesture>,
    /// Trigger values at or below this read as neutral (weight 0) — removes resting jitter.
    pub deadzone: f32,
    /// Grip squeeze at or above this activates `grip_gesture`.
    pub grip_threshold: f32,
}

impl Default for HandMapping {
    fn default() -> Self {
        HandMapping {
            trigger_gesture: Gesture::Fist,
            grip_gesture: None,
            deadzone: 0.05,
            grip_threshold: 0.5,
        }
    }
}

impl HandMapping {
    /// Resolve one hand's analog input to a gesture + weight.
    pub fn map(&self, input: HandInput) -> HandGesture {
        let trigger = input.trigger.clamp(0.0, 1.0);
        let grip = input.grip.clamp(0.0, 1.0);

        // Grip (if configured) wins and is a static, full-weight gesture.
        if let Some(g) = self.grip_gesture
            && grip >= self.grip_threshold
        {
            return HandGesture {
                gesture: g.as_int(),
                weight: 1.0,
            };
        }

        let weight = remap_deadzone(trigger, self.deadzone);
        if weight > 0.0 {
            HandGesture {
                gesture: self.trigger_gesture.as_int(),
                weight,
            }
        } else {
            HandGesture {
                gesture: Gesture::Neutral.as_int(),
                weight: 0.0,
            }
        }
    }
}

/// Rescale `trigger` so the range `(deadzone, 1]` maps linearly onto `(0, 1]`, and anything at or
/// below the deadzone is exactly 0 — a smooth analog response with the resting jitter removed.
fn remap_deadzone(trigger: f32, deadzone: f32) -> f32 {
    let deadzone = deadzone.clamp(0.0, 1.0);
    if trigger <= deadzone {
        return 0.0;
    }
    let span = 1.0 - deadzone;
    if span <= f32::EPSILON {
        // Deadzone at (or above) 1.0: only a full pull registers, as 1.0.
        return if trigger >= 1.0 { 1.0 } else { 0.0 };
    }
    ((trigger - deadzone) / span).clamp(0.0, 1.0)
}

/// The resolved gesture for one hand: which gesture, and the analog weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandGesture {
    pub gesture: i32,
    pub weight: f32,
}

/// The full mapping config: one [`HandMapping`] per hand.
#[derive(Debug, Clone, Copy, Default)]
pub struct GestureConfig {
    pub left: HandMapping,
    pub right: HandMapping,
}

impl GestureConfig {
    /// Resolve both hands of an analog frame.
    pub fn resolve(&self, state: &AnalogState) -> GestureFrame {
        GestureFrame {
            left: self.left.map(state.left),
            right: self.right.map(state.right),
        }
    }
}

/// Both hands resolved — the daemon's per-tick output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureFrame {
    pub left: HandGesture,
    pub right: HandGesture,
}

/// One OSC parameter the daemon emits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamUpdate {
    /// Parameter name without the `/avatar/parameters/` prefix (e.g. `GestureLeftWeight`).
    pub name: &'static str,
    pub value: ParamValue,
}

/// Weights this close are treated as unchanged, so a steady hand stops re-sending its weight.
const WEIGHT_EPSILON: f32 = 1e-4;

impl GestureFrame {
    /// The parameter updates needed to move VRChat from `prev` to this frame: the changed `Gesture*`
    /// int(s) and `Gesture*Weight` float(s). With `prev = None` (the first tick) all four are
    /// emitted so VRChat starts from a known state.
    pub fn updates_since(&self, prev: Option<&GestureFrame>) -> Vec<ParamUpdate> {
        let mut out = Vec::new();
        hand_updates(
            &mut out,
            prev.map(|p| &p.left),
            &self.left,
            "GestureLeft",
            "GestureLeftWeight",
        );
        hand_updates(
            &mut out,
            prev.map(|p| &p.right),
            &self.right,
            "GestureRight",
            "GestureRightWeight",
        );
        out
    }
}

fn hand_updates(
    out: &mut Vec<ParamUpdate>,
    prev: Option<&HandGesture>,
    cur: &HandGesture,
    gesture_name: &'static str,
    weight_name: &'static str,
) {
    if prev.is_none_or(|p| p.gesture != cur.gesture) {
        out.push(ParamUpdate {
            name: gesture_name,
            value: ParamValue::Int(cur.gesture),
        });
    }
    if prev.is_none_or(|p| (p.weight - cur.weight).abs() > WEIGHT_EPSILON) {
        out.push(ParamUpdate {
            name: weight_name,
            value: ParamValue::Float(cur.weight),
        });
    }
}

/// Where the daemon sends parameter updates. Implemented for [`avatar_osc::ParamClient`]; tests use
/// a recording sink. Kept as a trait so the daemon loop is unit-testable without a socket.
pub trait ParamSink {
    fn emit(&self, name: &str, value: ParamValue) -> Result<()>;
}

impl ParamSink for ParamClient {
    fn emit(&self, name: &str, value: ParamValue) -> Result<()> {
        self.send_param(name, value)
    }
}

/// The analog-gesture daemon: a source + a mapping + the last frame sent (for change detection).
pub struct GestureDaemon<S: AnalogSource> {
    source: S,
    config: GestureConfig,
    last: Option<GestureFrame>,
}

impl<S: AnalogSource> GestureDaemon<S> {
    /// A daemon over `source` with the default mapping (trigger → Fist, no grip gesture).
    pub fn new(source: S) -> Self {
        GestureDaemon {
            source,
            config: GestureConfig::default(),
            last: None,
        }
    }

    /// A daemon with an explicit mapping config.
    pub fn with_config(source: S, config: GestureConfig) -> Self {
        GestureDaemon {
            source,
            config,
            last: None,
        }
    }

    /// The mapping config in use.
    pub fn config(&self) -> &GestureConfig {
        &self.config
    }

    /// Poll the source once, resolve the frame, and send the parameters that changed since the last
    /// tick through `sink`. Returns what it sent (empty when nothing changed).
    pub fn tick(&mut self, sink: &impl ParamSink) -> Result<Vec<ParamUpdate>> {
        let state = self.source.poll();
        let frame = self.config.resolve(&state);
        let updates = frame.updates_since(self.last.as_ref());
        for u in &updates {
            sink.emit(u.name, u.value)?;
        }
        self.last = Some(frame);
        Ok(updates)
    }

    /// Tick forever at `period` (until the process is interrupted).
    pub fn run(&mut self, sink: &impl ParamSink, period: Duration) -> Result<()> {
        loop {
            self.tick(sink)?;
            std::thread::sleep(period);
        }
    }

    /// Tick at `period` for `total` wall-clock time, then stop — for bounded demos and tests.
    pub fn run_for(
        &mut self,
        sink: &impl ParamSink,
        period: Duration,
        total: Duration,
    ) -> Result<()> {
        let end = Instant::now() + total;
        while Instant::now() < end {
            self.tick(sink)?;
            std::thread::sleep(period);
        }
        Ok(())
    }
}

/// A scripted source: replays a list of analog frames, holding on the last. For precise tests.
#[derive(Debug, Clone, Default)]
pub struct ScriptedSource {
    frames: Vec<AnalogState>,
    index: usize,
}

impl ScriptedSource {
    pub fn new(frames: Vec<AnalogState>) -> Self {
        ScriptedSource { frames, index: 0 }
    }
}

impl AnalogSource for ScriptedSource {
    fn poll(&mut self) -> AnalogState {
        if self.frames.is_empty() {
            return AnalogState::default();
        }
        let frame = self.frames[self.index];
        if self.index + 1 < self.frames.len() {
            self.index += 1;
        }
        frame
    }
}

/// A deterministic demo source: a triangle wave (0→1→0) on the left trigger over `period_ticks`
/// ticks, with the right trigger in opposite phase. Drives a visible gesture sweep against a real
/// VRChat without any hardware. Uses an internal counter (no wall-clock / RNG), so it's reproducible.
#[derive(Debug, Clone)]
pub struct DemoSource {
    tick: u64,
    period: u64,
}

impl DemoSource {
    /// A demo wave with the given period in ticks (a full 0→1→0 sweep). Clamped to ≥ 2.
    pub fn new(period_ticks: u64) -> Self {
        DemoSource {
            tick: 0,
            period: period_ticks.max(2),
        }
    }
}

impl AnalogSource for DemoSource {
    fn poll(&mut self) -> AnalogState {
        let phase = (self.tick % self.period) as f32 / self.period as f32;
        let tri = 1.0 - (2.0 * phase - 1.0).abs(); // 0 → 1 → 0 across the period
        self.tick = self.tick.wrapping_add(1);
        AnalogState {
            left: HandInput {
                trigger: tri,
                grip: 0.0,
            },
            right: HandInput {
                trigger: 1.0 - tri,
                grip: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn trig(t: f32) -> HandInput {
        HandInput {
            trigger: t,
            grip: 0.0,
        }
    }

    // --- mapping --------------------------------------------------------------------------------

    #[test]
    fn below_deadzone_is_neutral() {
        let m = HandMapping::default(); // deadzone 0.05
        let g = m.map(trig(0.02));
        assert_eq!(g.gesture, Gesture::Neutral.as_int());
        assert_eq!(g.weight, 0.0);
    }

    #[test]
    fn trigger_drives_fist_with_rescaled_weight() {
        let m = HandMapping {
            deadzone: 0.1,
            ..Default::default()
        };
        // Exactly at the deadzone → still neutral.
        assert_eq!(m.map(trig(0.1)).gesture, Gesture::Neutral.as_int());
        // Midway past the deadzone → Fist at half weight ((0.55 - 0.1) / 0.9 = 0.5).
        let mid = m.map(trig(0.55));
        assert_eq!(mid.gesture, Gesture::Fist.as_int());
        assert!((mid.weight - 0.5).abs() < 1e-6, "weight was {}", mid.weight);
        // Full pull → weight 1.
        let full = m.map(trig(1.0));
        assert_eq!(full.gesture, Gesture::Fist.as_int());
        assert!((full.weight - 1.0).abs() < 1e-6);
    }

    #[test]
    fn inputs_are_clamped() {
        let m = HandMapping::default();
        assert_eq!(
            m.map(trig(5.0)).weight,
            1.0,
            "over-range trigger clamps to 1"
        );
        assert_eq!(
            m.map(trig(-1.0)).weight,
            0.0,
            "negative trigger clamps to 0"
        );
    }

    #[test]
    fn grip_gesture_overrides_trigger_when_configured() {
        let m = HandMapping {
            grip_gesture: Some(Gesture::HandOpen),
            grip_threshold: 0.5,
            ..Default::default()
        };
        // Trigger pulled, grip not squeezed → Fist (analog).
        let g = m.map(HandInput {
            trigger: 1.0,
            grip: 0.1,
        });
        assert_eq!(g.gesture, Gesture::Fist.as_int());
        // Grip squeezed → HandOpen at full weight, regardless of trigger.
        let g = m.map(HandInput {
            trigger: 1.0,
            grip: 0.9,
        });
        assert_eq!(g.gesture, Gesture::HandOpen.as_int());
        assert_eq!(g.weight, 1.0);
    }

    // --- change detection -----------------------------------------------------------------------

    #[test]
    fn first_frame_emits_all_four_params() {
        let frame = GestureConfig::default().resolve(&AnalogState {
            left: trig(1.0),
            right: trig(0.0),
        });
        let updates = frame.updates_since(None);
        assert_eq!(updates.len(), 4, "gesture + weight for each hand");
        assert!(updates.iter().any(|u| u.name == "GestureLeft"));
        assert!(updates.iter().any(|u| u.name == "GestureLeftWeight"));
    }

    #[test]
    fn steady_frame_emits_nothing() {
        let cfg = GestureConfig::default();
        let a = cfg.resolve(&AnalogState {
            left: trig(0.7),
            right: trig(0.0),
        });
        assert!(a.updates_since(Some(&a)).is_empty());
    }

    #[test]
    fn only_changed_weight_is_re_sent() {
        let cfg = GestureConfig::default();
        let prev = cfg.resolve(&AnalogState {
            left: trig(0.7),
            right: trig(0.0),
        });
        // Left weight changes, gesture stays Fist; right unchanged.
        let cur = cfg.resolve(&AnalogState {
            left: trig(0.8),
            right: trig(0.0),
        });
        let updates = cur.updates_since(Some(&prev));
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "GestureLeftWeight");
    }

    #[test]
    fn gesture_change_emits_the_int() {
        let cfg = GestureConfig::default();
        // Below deadzone (Neutral) → above (Fist): the int flips and the weight moves.
        let prev = cfg.resolve(&AnalogState {
            left: trig(0.0),
            right: trig(0.0),
        });
        let cur = cfg.resolve(&AnalogState {
            left: trig(1.0),
            right: trig(0.0),
        });
        let updates = cur.updates_since(Some(&prev));
        assert!(
            updates
                .iter()
                .any(|u| u.name == "GestureLeft"
                    && u.value == ParamValue::Int(Gesture::Fist.as_int()))
        );
        assert!(updates.iter().any(|u| u.name == "GestureLeftWeight"));
    }

    // --- daemon ---------------------------------------------------------------------------------

    #[derive(Default)]
    struct RecordingSink {
        sent: RefCell<Vec<(String, ParamValue)>>,
    }

    impl ParamSink for RecordingSink {
        fn emit(&self, name: &str, value: ParamValue) -> Result<()> {
            self.sent.borrow_mut().push((name.to_string(), value));
            Ok(())
        }
    }

    #[test]
    fn daemon_tick_sends_then_dedups() {
        let source = ScriptedSource::new(vec![
            AnalogState {
                left: trig(1.0),
                right: trig(0.0),
            },
            // identical frame → nothing new
            AnalogState {
                left: trig(1.0),
                right: trig(0.0),
            },
            // left releases → Neutral
            AnalogState {
                left: trig(0.0),
                right: trig(0.0),
            },
        ]);
        let mut daemon = GestureDaemon::new(source);
        let sink = RecordingSink::default();

        let first = daemon.tick(&sink).unwrap();
        assert_eq!(first.len(), 4, "first tick establishes all params");

        let second = daemon.tick(&sink).unwrap();
        assert!(second.is_empty(), "identical frame sends nothing");

        let third = daemon.tick(&sink).unwrap();
        // Left fell to Neutral: its gesture int and weight both changed.
        assert!(
            third.iter().any(|u| u.name == "GestureLeft"
                && u.value == ParamValue::Int(Gesture::Neutral.as_int()))
        );
        assert!(
            third
                .iter()
                .any(|u| u.name == "GestureLeftWeight" && u.value == ParamValue::Float(0.0))
        );

        // The recording sink saw exactly the non-empty ticks' params (first tick's 4, the
        // identical second tick's 0, plus the third tick's changes).
        assert_eq!(sink.sent.borrow().len(), 4 + third.len());
    }

    #[test]
    fn demo_source_sweeps_zero_to_one() {
        let mut src = DemoSource::new(4);
        // period 4: phases 0, .25, .5, .75 → triangle 0, .5, 1, .5
        let l: Vec<f32> = (0..4).map(|_| src.poll().left.trigger).collect();
        assert_eq!(l, vec![0.0, 0.5, 1.0, 0.5]);
    }
}
