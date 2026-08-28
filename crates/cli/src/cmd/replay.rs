//! `avatar osc replay` — run a captured parameter log through an FX controller's state machines
//! **offline** and print the state timeline each layer actually went through.
//!
//! The missing half of `avatar osc capture`: capture proves what VRChat *delivered*; replay
//! proves what the controller *did with it*, with no Unity in the loop. Feed it the same
//! `.controller` the avatar wears and the capture's `.jsonl`, and every gesture the user made
//! becomes a visible `Neutral → Fist R → Neutral` line with dwell times and the blend-parameter
//! range covered while inside each state — so "the input is fine but the face never moved"
//! versus "the state was entered exactly as designed" stops being a guess.
//!
//! Simulated semantics (the subset our generated controllers use — evaluated on every event):
//! Any-State transitions in `m_AnyStateTransitions` order with `m_CanTransitionToSelf`
//! honoured, the current state's own `m_Transitions`, and conditions If/IfNot (bool),
//! Greater/Less (float), Equals/NotEqual (int, rounded floats accepted). Transition durations
//! are treated as instantaneous (a crossfade shorter than the inter-event gap either way);
//! exit-time transitions are not modelled (ours set `m_HasExitTime: 0`).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_unity_yaml::{UnityDocument, UnityFile, field_f64, field_i64};
use clap::Args;
use serde_json::json;

#[derive(Args, Debug)]
pub struct OscReplayArgs {
    /// The capture's raw event log (`avatar osc capture -o …` / `gesture-capture.jsonl`).
    events: PathBuf,
    /// The `.controller` the avatar wears (its gesture layers are simulated).
    #[arg(long)]
    controller: PathBuf,
    /// Only report layers whose name contains this (case-insensitive).
    #[arg(long)]
    layer: Option<String>,
    /// Also print every state change as it happens (the timeline), not just the summary.
    #[arg(long)]
    timeline: bool,
    /// Emit a machine-readable JSON report.
    #[arg(long)]
    json: bool,
}

/// One transition condition, reduced to what evaluation needs.
struct Cond {
    parameter: String,
    mode: i64,
    threshold: f64,
}

impl Cond {
    fn holds(&self, params: &HashMap<String, f64>) -> bool {
        let v = params.get(&self.parameter).copied().unwrap_or(0.0);
        match self.mode {
            1 => v != 0.0,                                          // If (bool true)
            2 => v == 0.0,                                          // IfNot
            3 => v > self.threshold,                                // Greater
            4 => v < self.threshold,                                // Less
            6 => v.round() as i64 == self.threshold.round() as i64, // Equals
            7 => v.round() as i64 != self.threshold.round() as i64, // NotEqual
            _ => false,
        }
    }
}

struct Transition {
    dst: i64,
    can_self: bool,
    conditions: Vec<Cond>,
}

struct State {
    name: String,
    /// Local blend-tree fileID this state plays, if its motion is a local tree.
    tree: Option<i64>,
    /// The state's own outgoing transitions, in order.
    transitions: Vec<Transition>,
}

struct Layer {
    name: String,
    default_state: i64,
    any_state: Vec<Transition>,
    states: HashMap<i64, State>,
}

fn parse_transition(doc: &UnityDocument) -> Transition {
    let mut conditions = Vec::new();
    if let Some(list) = doc.body["m_Conditions"].as_vec() {
        for c in list {
            conditions.push(Cond {
                parameter: c["m_ConditionEvent"].as_str().unwrap_or("").to_string(),
                mode: field_i64(c, "m_ConditionMode").unwrap_or(0),
                threshold: field_f64(c, "m_EventTreshold").unwrap_or(0.0),
            });
        }
    }
    Transition {
        dst: field_i64(&doc.body["m_DstState"], "fileID").unwrap_or(0),
        can_self: field_i64(&doc.body, "m_CanTransitionToSelf").unwrap_or(0) != 0,
        conditions,
    }
}

/// Parse the controller's layers (gesture-style: one SM each, Any-State + state transitions).
fn parse_layers(file: &UnityFile) -> Result<Vec<Layer>> {
    let by_id: HashMap<i64, &UnityDocument> =
        file.documents.iter().map(|d| (d.file_id, d)).collect();
    let root = file
        .documents
        .iter()
        .find(|d| d.class_id == 91)
        .context("no AnimatorController (class 91) document")?;
    let mut layers = Vec::new();
    let Some(layer_list) = root.body["m_AnimatorLayers"].as_vec() else {
        bail!("controller has no m_AnimatorLayers");
    };
    for l in layer_list {
        let name = l["m_Name"].as_str().unwrap_or("?").to_string();
        let sm_id = field_i64(&l["m_StateMachine"], "fileID").unwrap_or(0);
        let Some(sm) = by_id.get(&sm_id).filter(|d| d.class_id == 1107) else {
            continue;
        };
        let default_state = field_i64(&sm.body["m_DefaultState"], "fileID").unwrap_or(0);
        let mut any_state = Vec::new();
        if let Some(refs) = sm.body["m_AnyStateTransitions"].as_vec() {
            for r in refs {
                if let Some(t) = field_i64(r, "fileID").and_then(|id| by_id.get(&id)) {
                    any_state.push(parse_transition(t));
                }
            }
        }
        let mut states = HashMap::new();
        if let Some(children) = sm.body["m_ChildStates"].as_vec() {
            for c in children {
                let Some(state) = field_i64(&c["m_State"], "fileID")
                    .and_then(|id| by_id.get(&id).map(|d| (id, d)))
                else {
                    continue;
                };
                let (id, doc) = state;
                let motion_id = field_i64(&doc.body["m_Motion"], "fileID").unwrap_or(0);
                let tree = (motion_id != 0
                    && doc.body["m_Motion"]["guid"].as_str().is_none()
                    && by_id.get(&motion_id).is_some_and(|d| d.class_id == 206))
                .then_some(motion_id);
                let mut transitions = Vec::new();
                if let Some(refs) = doc.body["m_Transitions"].as_vec() {
                    for r in refs {
                        if let Some(t) = field_i64(r, "fileID").and_then(|id| by_id.get(&id)) {
                            transitions.push(parse_transition(t));
                        }
                    }
                }
                states.insert(
                    id,
                    State {
                        name: doc.body["m_Name"].as_str().unwrap_or("?").to_string(),
                        tree,
                        transitions,
                    },
                );
            }
        }
        layers.push(Layer {
            name,
            default_state,
            any_state,
            states,
        });
    }
    Ok(layers)
}

/// The blend parameter of a local blend tree, so visits can report the dial/weight range.
fn tree_blend_parameter(file: &UnityFile, tree_id: i64) -> Option<String> {
    file.documents
        .iter()
        .find(|d| d.file_id == tree_id && d.class_id == 206)
        .and_then(|d| d.body["m_BlendParameter"].as_str())
        .map(str::to_string)
}

#[derive(serde::Serialize)]
struct Visit {
    layer: String,
    state: String,
    enter: f64,
    leave: Option<f64>,
    /// Blend-parameter range observed while inside (states playing a local 1D tree).
    blend_min: Option<f64>,
    blend_max: Option<f64>,
}

pub fn replay(args: &OscReplayArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.controller)
        .with_context(|| format!("reading {}", args.controller.display()))?;
    let file = UnityFile::parse(&text)?;
    let layers: Vec<Layer> = parse_layers(&file)?
        .into_iter()
        .filter(|l| {
            args.layer
                .as_ref()
                .is_none_or(|f| l.name.to_lowercase().contains(&f.to_lowercase()))
        })
        .collect();
    if layers.is_empty() {
        bail!("no matching layers in {}", args.controller.display());
    }
    let blend_param: HashMap<i64, String> = layers
        .iter()
        .flat_map(|l| l.states.values())
        .filter_map(|s| s.tree)
        .filter_map(|t| tree_blend_parameter(&file, t).map(|p| (t, p)))
        .collect();

    // Event-driven simulation across all layers sharing one parameter store.
    let mut params: HashMap<String, f64> = HashMap::new();
    let mut current: Vec<i64> = layers.iter().map(|l| l.default_state).collect();
    let mut visits: Vec<Visit> = Vec::new();
    // Seed a visit's blend range with the blend parameter's value *at entry* — a constant weight
    // sends no update events during the visit, but the state still plays at that value.
    let entry_blend = |layer: &Layer,
                       state_id: i64,
                       params: &HashMap<String, f64>,
                       blend_param: &HashMap<i64, String>|
     -> Option<f64> {
        layer.states[&state_id]
            .tree
            .and_then(|t| blend_param.get(&t))
            .map(|p| params.get(p).copied().unwrap_or(0.0))
    };
    let mut open: Vec<usize> = layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let b = entry_blend(l, current[i], &params, &blend_param);
            visits.push(Visit {
                layer: l.name.clone(),
                state: l.states[&current[i]].name.clone(),
                enter: 0.0,
                leave: None,
                blend_min: b,
                blend_max: b,
            });
            visits.len() - 1
        })
        .collect();

    let raw = std::fs::read_to_string(&args.events)
        .with_context(|| format!("reading {}", args.events.display()))?;
    let mut events = 0usize;
    let mut last_t = 0.0f64;
    for line in raw.lines().filter(|l| !l.trim().is_empty()) {
        let e: serde_json::Value =
            serde_json::from_str(line).with_context(|| format!("bad JSONL line: {line}"))?;
        let (Some(t), Some(name)) = (e["t"].as_f64(), e["name"].as_str()) else {
            bail!("JSONL line missing t/name: {line}");
        };
        let v = &e["value"];
        let value = v
            .as_f64()
            .or_else(|| v.as_bool().map(|b| b as i64 as f64))
            .unwrap_or(0.0);
        params.insert(name.to_string(), value);
        events += 1;
        last_t = t;

        for (i, layer) in layers.iter().enumerate() {
            // Track the blend range of the open visit.
            let state = &layer.states[&current[i]];
            if let Some(tree) = state.tree
                && let Some(p) = blend_param.get(&tree)
                && p == name
            {
                let vis = &mut visits[open[i]];
                vis.blend_min = Some(vis.blend_min.map_or(value, |m: f64| m.min(value)));
                vis.blend_max = Some(vis.blend_max.map_or(value, |m: f64| m.max(value)));
            }
            // First valid transition wins: Any-State (in order), then the state's own.
            let next = layer
                .any_state
                .iter()
                .chain(state.transitions.iter())
                .find(|tr| {
                    (tr.can_self || tr.dst != current[i])
                        && layer.states.contains_key(&tr.dst)
                        && tr.conditions.iter().all(|c| c.holds(&params))
                })
                .map(|tr| tr.dst);
            if let Some(dst) = next
                && dst != current[i]
            {
                visits[open[i]].leave = Some(t);
                current[i] = dst;
                let b = entry_blend(layer, dst, &params, &blend_param);
                visits.push(Visit {
                    layer: layer.name.clone(),
                    state: layer.states[&dst].name.clone(),
                    enter: t,
                    leave: None,
                    blend_min: b,
                    blend_max: b,
                });
                open[i] = visits.len() - 1;
            }
        }
    }
    for &i in &open {
        visits[i].leave = Some(last_t);
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "events": events,
                "seconds": last_t,
                "visits": visits,
            }))?
        );
        return Ok(());
    }

    println!(
        "replayed {events} event(s) over {last_t:.1}s through {} layer(s) of {}",
        layers.len(),
        args.controller.display()
    );
    if args.timeline {
        println!("\n  timeline:");
        for v in &visits {
            let blend = match (v.blend_min, v.blend_max) {
                (Some(a), Some(b)) => format!("  blend {a:.3}..{b:.3}"),
                _ => String::new(),
            };
            println!(
                "  {:7.1}s..{:>7} {:<12} {}{blend}",
                v.enter,
                v.leave.map_or("end".into(), |t| format!("{t:.1}s")),
                v.layer,
                v.state
            );
        }
    }
    // Summary: per (layer, state): visits, total dwell, blend coverage.
    println!("\n  dwell summary:");
    let header = format!(
        "  {:<12} {:<12} {:>7} {:>9}  blend range",
        "layer", "state", "visits", "total"
    );
    println!("{header}");
    let mut keys: Vec<(String, String)> = Vec::new();
    for v in &visits {
        let k = (v.layer.clone(), v.state.clone());
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    for (layer, state) in keys {
        let vs: Vec<&Visit> = visits
            .iter()
            .filter(|v| v.layer == layer && v.state == state)
            .collect();
        let total: f64 = vs.iter().map(|v| v.leave.unwrap_or(last_t) - v.enter).sum();
        let (mut bmin, mut bmax) = (None::<f64>, None::<f64>);
        for v in &vs {
            if let Some(a) = v.blend_min {
                bmin = Some(bmin.map_or(a, |m: f64| m.min(a)));
            }
            if let Some(b) = v.blend_max {
                bmax = Some(bmax.map_or(b, |m: f64| m.max(b)));
            }
        }
        let blend = match (bmin, bmax) {
            (Some(a), Some(b)) => format!("{a:.3}..{b:.3}"),
            _ => "-".into(),
        };
        println!(
            "  {:<12} {:<12} {:>7} {:>8.1}s  {}",
            layer,
            state,
            vs.len(),
            total,
            blend
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replay a synthetic capture through a generated analog gesture controller: the Fist R
    /// state engages when GestureRight=1 with weight above the gate, and the left hand's state
    /// engages when the right releases — the mutual-exclusion semantics, offline.
    #[test]
    fn replay_enters_gated_states_in_order() {
        use avatar_anim_gen::{GestureLayer, IdGen, ObjectRef, fx_gestures};
        let neutral = ObjectRef::external(7400000, "a000000000000000000000000000000a", 2);
        let fist = ObjectRef::external(7400000, "a000000000000000000000000000000b", 2);
        let layer = GestureLayer::either_hand("Gestures", neutral)
            .motion(1, fist)
            .analog();
        let yaml = fx_gestures("FX", &[layer], &[], &mut IdGen::new("FX"));
        let file = UnityFile::parse(&yaml).unwrap();
        let layers = parse_layers(&file).unwrap();
        assert_eq!(layers.len(), 1);
        let l = &layers[0];
        let mut params: HashMap<String, f64> = HashMap::new();
        let mut cur = l.default_state;
        let step = |params: &HashMap<String, f64>, cur: &mut i64| {
            let state = &l.states[cur];
            if let Some(dst) = l
                .any_state
                .iter()
                .chain(state.transitions.iter())
                .find(|tr| {
                    (tr.can_self || tr.dst != *cur)
                        && l.states.contains_key(&tr.dst)
                        && tr.conditions.iter().all(|c| c.holds(params))
                })
                .map(|tr| tr.dst)
            {
                *cur = dst;
            }
        };
        let name = |id: i64| l.states[&id].name.clone();
        assert_eq!(name(cur), "Neutral");
        // Right fist at weight 0.5 -> Fist R.
        params.insert("GestureRight".into(), 1.0);
        params.insert("GestureRightWeight".into(), 0.5);
        step(&params, &mut cur);
        assert_eq!(name(cur), "Fist R");
        // Left fist joins: right still squeezing -> right keeps the face.
        params.insert("GestureLeft".into(), 1.0);
        params.insert("GestureLeftWeight".into(), 1.0);
        step(&params, &mut cur);
        assert_eq!(name(cur), "Fist R");
        // Right releases below the off threshold -> left takes over.
        params.insert("GestureRightWeight".into(), 0.0);
        step(&params, &mut cur);
        assert_eq!(name(cur), "Fist L");
        // Both to neutral -> Neutral.
        params.insert("GestureLeft".into(), 0.0);
        params.insert("GestureRight".into(), 0.0);
        step(&params, &mut cur);
        assert_eq!(name(cur), "Neutral");
    }
}
