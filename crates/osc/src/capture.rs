//! Parameter **capture**: record the parameter stream VRChat broadcasts and reduce it to the
//! report that answers "what does my controller actually deliver?" — per-parameter update
//! counts/values, plus the gesture cross-tab: for every `GestureLeft`/`GestureRight` value that
//! was held, the range the matching `GestureLeftWeight`/`GestureRightWeight` covered while it
//! was held.
//!
//! That cross-tab is the ground truth for gesture debugging (e.g. Vive-wand "advanced
//! controls"): it shows directly which touchpad regions produce which gesture values, and
//! whether the trigger's analog depth reaches the avatar for each of them. VRChat sends these
//! built-in parameters on its OSC output port (9001) whenever OSC is enabled, so a capture
//! session is: enable OSC, run the capture, sweep the touchpad and trigger, read the table.

use std::time::Instant;

use serde::Serialize;

use crate::codec::ParamValue;

/// One timestamped parameter update.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureEvent {
    /// Seconds since the capture started.
    pub t: f64,
    /// Parameter name (the path after `/avatar/parameters/`).
    pub name: String,
    /// The value, tagged by type.
    pub value: CaptureValue,
}

/// A [`ParamValue`] in serializable form.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CaptureValue {
    Bool(bool),
    Int(i32),
    Float(f32),
}

impl From<ParamValue> for CaptureValue {
    fn from(v: ParamValue) -> Self {
        match v {
            ParamValue::Bool(b) => CaptureValue::Bool(b),
            ParamValue::Int(i) => CaptureValue::Int(i),
            ParamValue::Float(f) => CaptureValue::Float(f),
        }
    }
}

/// Accumulates a capture session: every event, plus the running gesture state needed for the
/// weight cross-tab.
#[derive(Debug)]
pub struct Capture {
    start: Instant,
    events: Vec<CaptureEvent>,
    /// Current `GestureLeft` / `GestureRight` int values (None until first seen).
    current_gesture: [Option<i32>; 2],
    /// Cross-tab rows keyed by (hand index, gesture value).
    rows: Vec<GestureWeightRow>,
}

/// Weight behaviour observed while one hand held one gesture value.
#[derive(Debug, Clone, Serialize)]
pub struct GestureWeightRow {
    /// `"left"` or `"right"`.
    pub hand: &'static str,
    /// The `GestureLeft`/`GestureRight` value (0 Neutral … 7 ThumbsUp).
    pub gesture: i32,
    /// When this gesture value was first seen (seconds into the capture).
    pub first_seen: f64,
    /// How many times the hand entered this gesture value.
    pub activations: u32,
    /// Weight samples received while the value was held.
    pub weight_samples: u32,
    /// Min/max of those samples (None if no weight update arrived while held).
    pub weight_min: Option<f32>,
    pub weight_max: Option<f32>,
}

/// Per-parameter roll-up.
#[derive(Debug, Clone, Serialize)]
pub struct ParamSummary {
    pub name: String,
    pub updates: u32,
    /// Distinct values, for bool/int parameters (in first-seen order, capped at 16).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<CaptureValue>,
    /// Value range, for float parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
}

/// The reduced session report.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureSummary {
    /// Total events recorded.
    pub events: usize,
    /// Session length in seconds (time of the last event).
    pub seconds: f64,
    pub params: Vec<ParamSummary>,
    /// The gesture/weight cross-tab, sorted by hand then gesture value. Empty if no
    /// `GestureLeft`/`GestureRight` updates were seen (OSC likely not enabled, or no avatar).
    pub gestures: Vec<GestureWeightRow>,
}

impl Default for Capture {
    fn default() -> Self {
        Self::new()
    }
}

impl Capture {
    pub fn new() -> Self {
        Capture {
            start: Instant::now(),
            events: Vec::new(),
            current_gesture: [None, None],
            rows: Vec::new(),
        }
    }

    /// Record one parameter update; returns the stored event (with its timestamp).
    pub fn record(&mut self, name: &str, value: ParamValue) -> &CaptureEvent {
        let t = self.start.elapsed().as_secs_f64();
        let cv: CaptureValue = value.into();
        // Gesture cross-tab bookkeeping. VRChat's built-ins: GestureLeft/GestureRight (int),
        // GestureLeftWeight/GestureRightWeight (float). Some setups send ints as floats — accept
        // both.
        let hand = |n: &str| -> Option<usize> {
            match n {
                "GestureLeft" | "GestureLeftWeight" => Some(0),
                "GestureRight" | "GestureRightWeight" => Some(1),
                _ => None,
            }
        };
        if let Some(h) = hand(name) {
            if name.ends_with("Weight") {
                if let (Some(g), CaptureValue::Float(w)) = (self.current_gesture[h], cv)
                    && let Some(row) = self
                        .rows
                        .iter_mut()
                        .find(|r| r.hand == HANDS[h] && r.gesture == g)
                {
                    row.weight_samples += 1;
                    row.weight_min = Some(row.weight_min.map_or(w, |m| m.min(w)));
                    row.weight_max = Some(row.weight_max.map_or(w, |m| m.max(w)));
                }
            } else {
                let g = match cv {
                    CaptureValue::Int(i) => Some(i),
                    CaptureValue::Float(f) => Some(f.round() as i32),
                    CaptureValue::Bool(_) => None,
                };
                if let Some(g) = g
                    && self.current_gesture[h] != Some(g)
                {
                    self.current_gesture[h] = Some(g);
                    match self
                        .rows
                        .iter_mut()
                        .find(|r| r.hand == HANDS[h] && r.gesture == g)
                    {
                        Some(row) => row.activations += 1,
                        None => self.rows.push(GestureWeightRow {
                            hand: HANDS[h],
                            gesture: g,
                            first_seen: t,
                            activations: 1,
                            weight_samples: 0,
                            weight_min: None,
                            weight_max: None,
                        }),
                    }
                }
            }
        }
        self.events.push(CaptureEvent {
            t,
            name: name.to_string(),
            value: cv,
        });
        // Just pushed, so `last()` cannot be empty; avoid expect() under the ingest lint.
        &self.events[self.events.len() - 1]
    }

    pub fn events(&self) -> &[CaptureEvent] {
        &self.events
    }

    /// Reduce the session to its report.
    pub fn summary(&self) -> CaptureSummary {
        let mut params: Vec<ParamSummary> = Vec::new();
        for e in &self.events {
            let idx = match params.iter().position(|p| p.name == e.name) {
                Some(i) => i,
                None => {
                    params.push(ParamSummary {
                        name: e.name.clone(),
                        updates: 0,
                        values: Vec::new(),
                        min: None,
                        max: None,
                    });
                    params.len() - 1
                }
            };
            let p = &mut params[idx];
            p.updates += 1;
            match e.value {
                CaptureValue::Float(f) => {
                    p.min = Some(p.min.map_or(f, |m| m.min(f)));
                    p.max = Some(p.max.map_or(f, |m| m.max(f)));
                }
                v => {
                    if !p.values.contains(&v) && p.values.len() < 16 {
                        p.values.push(v);
                    }
                }
            }
        }
        let mut gestures = self.rows.clone();
        gestures.sort_by(|a, b| (a.hand, a.gesture).cmp(&(b.hand, b.gesture)));
        CaptureSummary {
            events: self.events.len(),
            seconds: self.events.last().map(|e| e.t).unwrap_or(0.0),
            params,
            gestures,
        }
    }
}

const HANDS: [&str; 2] = ["left", "right"];

/// Render the summary as the human-readable report the capture session prints on exit.
pub fn render_summary(s: &CaptureSummary) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "capture: {} update(s) over {:.1}s across {} parameter(s)",
        s.events,
        s.seconds,
        s.params.len()
    );
    if s.gestures.is_empty() {
        let _ = writeln!(
            out,
            "  no GestureLeft/GestureRight updates seen — is OSC enabled (Action Menu → Options \
             → OSC) and an avatar loaded?"
        );
    } else {
        let _ = writeln!(
            out,
            "\n  gesture cross-tab (weight range while each gesture value was held):"
        );
        let _ = writeln!(
            out,
            "  {:<5} {:>7} {:>12} {:>8} {:>15} {:>10}",
            "hand", "gesture", "activations", "weights", "weight range", "first at"
        );
        for r in &s.gestures {
            let range = match (r.weight_min, r.weight_max) {
                (Some(a), Some(b)) => format!("{a:.3}..{b:.3}"),
                _ => "(none)".into(),
            };
            let _ = writeln!(
                out,
                "  {:<5} {:>7} {:>12} {:>8} {:>15} {:>9.1}s",
                r.hand, r.gesture, r.activations, r.weight_samples, range, r.first_seen
            );
        }
        // The specific absence that matters for wand debugging.
        for (hand, missing) in [("left", 0..8), ("right", 0..8)].map(|(h, r)| {
            (
                h,
                r.filter(|g| {
                    !s.gestures
                        .iter()
                        .any(|row| row.hand == h && row.gesture == *g)
                })
                .collect::<Vec<_>>(),
            )
        }) {
            if !missing.is_empty() && s.gestures.iter().any(|row| row.hand == hand) {
                let _ = writeln!(
                    out,
                    "  {hand}: gesture value(s) never seen: {missing:?} (1 = Fist)"
                );
            }
        }
    }
    let _ = writeln!(out, "\n  parameters:");
    for p in &s.params {
        let detail = match (p.min, p.max) {
            (Some(a), Some(b)) => format!("float {a:.3}..{b:.3}"),
            _ => format!("{:?}", p.values),
        };
        let _ = writeln!(out, "  {:<40} {:>6}x  {}", p.name, p.updates, detail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cross-tab attributes weight samples to the gesture value held when they arrive, and
    /// tracks per-value activation counts and ranges.
    #[test]
    fn cross_tab_attributes_weights_to_the_held_gesture() {
        let mut c = Capture::new();
        c.record("GestureLeft", ParamValue::Int(0));
        c.record("GestureLeftWeight", ParamValue::Float(0.0));
        c.record("GestureLeft", ParamValue::Int(4)); // Victory held…
        c.record("GestureLeftWeight", ParamValue::Float(0.3));
        c.record("GestureLeftWeight", ParamValue::Float(0.9));
        c.record("GestureLeft", ParamValue::Int(0)); // …released
        c.record("GestureLeftWeight", ParamValue::Float(0.1));
        c.record("GestureRight", ParamValue::Float(1.0)); // int-as-float tolerated
        let s = c.summary();
        let row = |h: &str, g: i32| {
            s.gestures
                .iter()
                .find(|r| r.hand == h && r.gesture == g)
                .unwrap()
                .clone()
        };
        let v = row("left", 4);
        assert_eq!((v.activations, v.weight_samples), (1, 2));
        assert_eq!((v.weight_min, v.weight_max), (Some(0.3), Some(0.9)));
        let n = row("left", 0);
        assert_eq!(n.activations, 2, "entered at start and again on release");
        assert_eq!((n.weight_min, n.weight_max), (Some(0.0), Some(0.1)));
        assert_eq!(row("right", 1).gesture, 1);
        // Summary params: floats get ranges, ints distinct values.
        let gl = s.params.iter().find(|p| p.name == "GestureLeft").unwrap();
        assert_eq!(gl.values.len(), 2);
        let w = s
            .params
            .iter()
            .find(|p| p.name == "GestureLeftWeight")
            .unwrap();
        assert_eq!((w.min, w.max), (Some(0.0), Some(0.9)));
        let text = render_summary(&s);
        assert!(text.contains("never seen"), "{text}");
    }
}
