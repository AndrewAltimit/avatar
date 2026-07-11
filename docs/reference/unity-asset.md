# AnimatorController + AnimationClip reading (`avatar-unity-asset`)

`avatar-unity-asset` is the **typed** layer over `avatar-unity-yaml`: where that crate splits a Unity
file into class-tagged documents of raw YAML, this one reads a specific asset *graph* into Rust
structs. It covers the **AnimatorController** (`.controller`) — the asset VRChat avatars drive
through their playable layers (FX, Gesture, Action, …) — and the **AnimationClip** (`.anim`) those
controllers play. It is the reader the `avatar lint` rules use to check a controller's parameters,
default states, Write Defaults consistency, blend trees, and clip contents
(`avatar-lint` consumes the structs below; lint codes live in
[`sdk3-lint-rules.md`](sdk3-lint-rules.md)).

It is a library crate (no CLI surface of its own). Reference:
<https://docs.unity3d.com/Manual/class-AnimatorController.html>.

## What a `.controller` file is

A `.controller` is a multi-document Unity YAML stream. One `AnimatorController` object (class id 91)
owns a graph of further objects, linked by local `fileID`s:

| Class id | Object | What this crate reads from it |
|----------|--------|-------------------------------|
| `91` | `AnimatorController` | name, declared parameters (`m_AnimatorParameters`) |
| `1107` | `AnimatorStateMachine` | child-state count, whether `m_DefaultState` resolves |
| `1102` | `AnimatorState` | `m_WriteDefaultValues` and the `m_Motion` reference (one per state) |
| `1101` / `1109` | `AnimatorStateTransition` / `AnimatorTransition` | each `m_Conditions` entry |
| `206` | `BlendTree` | blend type, blend parameter(s), per-child direct parameters + external child-motion guids |

### Aggregate, not full graph

For the questions the lint rules ask — *which parameters are referenced, are write-defaults
consistent, is a default state set* — the full linked graph is unnecessary. So `from_file` walks the
file's documents and **aggregates the relevant fields by Unity class id** rather than rebuilding the
state-machine topology. This is robust to SDK version drift, because the field names are stable Unity
serialization rather than VRChat specifics. (A `.controller` holds exactly one controller; if a file
somehow held more, the owned objects are attributed to the controller as a whole, not split.)

## Key types

- **`AnimatorController`** — the parsed result: `name`, `parameters: Vec<AnimatorParameter>`,
  `conditions: Vec<AnimatorCondition>` (every condition across every transition), `blend_trees:
  Vec<BlendTreeInfo>`, `state_machines: Vec<StateMachineInfo>`, `write_defaults: Vec<bool>` (one per
  state, in document order), and `state_count`.
  - `AnimatorController::from_file(&UnityFile) -> Option<Self>` — parse; `None` if the file has no
    `AnimatorController` document.
  - `parameter_names() -> impl Iterator<Item = &str>` — the declared parameter names.
- **`AnimatorParameter { name, raw_type }`** — `raw_type` is Unity's `m_Type` (1 Float, 3 Int,
  4 Bool, 9 Trigger); `type_name()` renders it as a string.
- **`AnimatorCondition { parameter, mode, threshold }`** — one transition condition. `mode` is the
  raw `m_ConditionMode` (1 If, 2 IfNot, 3 Greater, 4 Less, 6 Equals, 7 NotEqual). (Note: the
  serialized threshold field is Unity's misspelled `m_EventTreshold`.)
- **`BlendTreeInfo { blend_type, blend_parameter, blend_parameter_y, direct_parameters }`** —
  `blend_type` is `m_BlendType` (0 = 1D, 1–3 = 2D variants, 4 = Direct).
  `referenced_parameters()` returns only the parameters a tree actually reads given its type: a 1D
  tree reads X; a 2D tree reads X and Y; a Direct tree reads each child's direct parameter (empty
  names dropped).
- **`StateMachineInfo { child_state_count, has_default_state }`** — `has_default_state` is `true`
  only when `m_DefaultState` points at a real state (non-zero `fileID`).
- **`StateInfo { name, write_defaults, motion }`** — one per `AnimatorState`, in document order
  (`states` on the controller). `motion` is a **`MotionRef { file_id, guid }`**: a local blend
  tree (`fileID` only), an external clip (`guid` set), or null (`is_set()` false).
  `blend_tree_motion_guids` collects every external guid a blend-tree child references.

## AnimationClip (`.anim`, class 74)

`AnimationClip::from_file(&UnityFile) -> Option<Self>` reads a clip down to its curve
**bindings** — what each curve animates, not the keyframe data, which is all the clip-content
lint rules need:

- **`float_curves: Vec<FloatCurveBinding { path, attribute, class_id }>`** — every
  `m_FloatCurves` entry (blendshapes bind class 137, GameObject toggles class 1, humanoid muscle
  curves class 95 with an empty path; `is_muscle()` tests the latter).
- **`transform_curves`** — total entries across `m_PositionCurves` / `m_RotationCurves` /
  `m_EulerCurves` / `m_ScaleCurves`; **`pptr_curves`** — `m_PPtrCurves` entries (material swaps).
- Predicates: `is_empty()` (no curves at all), `animates_transforms()`, `animates_muscles()`.

## Usage

```rust,no_run
use avatar_unity_yaml::UnityFile;
use avatar_unity_asset::AnimatorController;

let text = std::fs::read_to_string("FX.controller")?;
let file = UnityFile::parse(&text)?;
if let Some(controller) = AnimatorController::from_file(&file) {
    for p in &controller.parameters {
        println!("{}: {}", p.name, p.type_name());
    }
    // Write Defaults consistency: a controller should be all-on or all-off.
    let mixed = controller.write_defaults.iter().any(|&w| w)
        && controller.write_defaults.iter().any(|&w| !w);
    if mixed {
        println!("warning: inconsistent Write Defaults across states");
    }
    // Parameters a blend tree reads, given its blend type:
    for bt in &controller.blend_trees {
        println!("blend tree reads {:?}", bt.referenced_parameters());
    }
}
# anyhow::Ok(())
```

## Status

Built (**M2**, AnimationClip added post-M5). AnimatorController + AnimationClip reading; other
typed asset graphs (descriptor, menus, parameters) are read elsewhere — see
[`avatar-vrc-descriptor`](../../crates/vrc-descriptor/README.md) and the
[lint rules](sdk3-lint-rules.md). Material / scene typing is still to come (`PLAN.md` M4).
