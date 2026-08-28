# Asset generation (`avatar-anim-gen`)

`avatar-anim-gen` generates Unity assets as text — `.anim` AnimationClips and FX-layer
analog-gesture blend trees — for the M4 "asset generation" milestone (`PLAN.md` §4, §9). It is the
*generator* counterpart to the readers in `avatar-unity-yaml` / `avatar-unity-asset`: where those
parse Unity YAML, this emits it, in the exact shape Unity's own serializer writes so the result
imports cleanly.

Everything below is a typed Rust builder that renders to a Unity-YAML string; the CLI surface that
drives it (`avatar anim-gen clip|blendtree|controller`) is documented in [CLI surface](#cli-surface).

## Why a generator, not an editor

`PLAN.md` risk 2 records the decision: *prefer generating fresh assets (new GUIDs/fileIDs) over
surgical edits.* Round-tripping arbitrary Unity YAML byte-for-byte is fragile; emitting a brand-new
asset whose every fileID we control is not. So this crate never mutates an existing file — it
produces a complete `.anim`, or a blend-tree fragment a user pastes into their controller.

## What it generates

### 1. AnimationClip (`.anim`, class id 74)

A `.anim` is a single-document stream containing one `AnimationClip`. Unity sorts animated values
into class-typed curve collections; the scalar one is `m_FloatCurves`, and that is the one we
generate into. Each entry binds a curve to `(path, attribute, classID)`:

- `path` — the transform path from the animator root to the animated object (`Body`,
  `Armature/Head/Hat`, or `""` for the root).
- `attribute` — the serialized property name.
- `classID` — the Unity class of the component that owns the property.

Two worked cases are provided, and they differ *only* in that triple:

| Case | `attribute` | `classID` | Builder |
|------|-------------|-----------|---------|
| **Blendshape weight** | `blendShape.<name>` | 137 (SkinnedMeshRenderer) | `FloatCurve::blendshape(path, shape, keyframes)` |
| **GameObject active toggle** | `m_IsActive` | 1 (GameObject) | `FloatCurve::game_object_active(path, keyframes)` |

A GameObject's active state is animated as a `float` curve (values `0`/`1`), which is why the
toggle is a `m_FloatCurves` entry and not a special collection.

A keyframe is `{ time, value, in_slope, out_slope }`. `Keyframe::flat(t, v)` is the common case
(flat tangents) for the step/linear clips expressions and toggles use; `Keyframe::new` takes
explicit slopes. Each keyframe is emitted with the `serializedVersion: 3`, `tangentMode`,
`weightedMode`, `inWeight`/`outWeight` tail Unity writes, and each curve with its
`m_PreInfinity`/`m_PostInfinity`/`m_RotationOrder` tail.

The clip's `m_AnimationClipSettings` (length, loop, the loop-blend flags) is emitted from
`ClipSettings`; `m_StopTime` is auto-extended to cover the latest keyframe (a clip whose keys are
all at `t=0` still gets a one-frame length, since Unity treats a zero-length clip as empty). The
transform-curve collections (`m_PositionCurves`, `m_ScaleCurves`, `m_RotationCurves`,
`m_EulerCurves`, `m_PPtrCurves`) are emitted as empty lists — present but unused, which is what
Unity expects.

```rust
use avatar_anim_gen::*;
let mut ids = IdGen::new("Smile");
let clip = AnimationClip::new("Smile").float_curve(FloatCurve::blendshape(
    "Body",
    "Smile",
    vec![Keyframe::flat(0.0, 0.0), Keyframe::flat(1.0 / 60.0, 100.0)],
));
let anim_text = clip.to_unity_yaml(ids.alloc());
```

### 2. Analog-gesture 1D BlendTree (class id 206)

The headline `PLAN.md` §4 feature. VRChat exposes the trigger pull as `GestureLeftWeight` /
`GestureRightWeight` (a float 0→1). A **1D BlendTree** in the Fist gesture state of the FX layer,
blending on that weight across child motions, lets any gesture reach any fraction — the generator
analogue of what ComboGestureExpressions does by hand.

The blend tree's `m_Childs` each carry an `m_Motion` (an `ObjectRef`), an `m_Threshold` (the
blend-parameter value at which the child is fully weighted), and Unity's `m_TimeScale` /
`m_CycleOffset` / `m_DirectBlendParameter` / `m_Mirror` tail. The tree's `m_BlendParameter` is the
weight parameter, `m_BlendType` is `0` (Simple 1D), and `m_UseAutomaticThresholds` defaults to
`false` so the analog mapping is exact (each clip pinned to its threshold rather than evenly
spread).

> **Field-name gotchas verified against Unity serialization:** `m_BlendParameter`,
> `m_BlendParameterY`, and a child's `m_DirectBlendParameter` are **string** fields (parameter
> names), not numeric — a common mistake when hand-writing these. `m_MaxThreshold` /
> `m_MinThreshold` bound the range; a child motion that is an external clip is referenced as
> `{fileID: 7400000, guid: <anim-guid>, type: 2}` (fileID `7400000` is the canonical local id of
> the single AnimationClip inside a `.anim`).

Two emission modes:

- **`emit_tree(&mut Emitter, file_id)`** — just the `--- !u!206` document. Pair it with
  `wiring_note(file_id)`, which explains how to graft it in: set the Fist gesture state's
  `m_Motion` to `{fileID: <id>}`, paste the 206 document into the controller, and declare the
  weight parameter if it isn't already present (VRChat populates it from the trigger automatically).
- **`to_state_fragment(&mut IdGen)`** — a self-contained `AnimatorStateMachine` (1107) +
  `AnimatorState` (1102) + `BlendTree` (206) trio, wired so the state machine's default state plays
  the tree, for callers who want a drop-in sub-state-machine. The state is emitted with
  `m_WriteDefaultValues: 0` (VRChat's recommendation for FX clips). It returns the fragment text
  and the state-machine fileID (the entry point a layer's `m_StateMachine` references).

### 3. Full FX AnimatorController (`.controller`, class id 91)

The fragment-plus-note approach above leaves the orchestration to the user's project — the right
default when grafting into an avatar's *existing* FX controller. For the case where a brand-new
controller is wanted, the `controller` module reverses that scope decision and emits the enclosing
class-91 `AnimatorController` object: its `m_AnimatorParameters` and `m_AnimatorLayers`, each
layer's `m_StateMachine` referencing a fragment's state machine by local fileID.

- `AnimatorController::new(name).parameter(p).layer(name, sm_id)` is the typed builder;
  `emit_controller(&mut Emitter, file_id)` writes only the class-91 document.
- `ParamType` (`Float`/`Int`/`Bool`/`Trigger`) carries Unity's raw `m_Type` ints (1/3/4/9);
  `AnimatorParameter::{float,int,bool,trigger}` are the constructors.
- **`fx_blend_tree(name, layer_name, tree, ids)`** is the headline: it allocates the controller id
  first (the lowest, stablest id in the file), assembles the state-machine/state/blend-tree
  fragment via `to_state_fragment`, auto-declares the tree's `blend_parameter` as a `Float`, wires
  one layer to the fragment's state machine, and returns the complete multi-document `.controller`
  text (`%YAML` preamble + class-91 doc + fragment).

The class-91 field set/order is matched against a real Unity-authored FX controller — not just the
minimal subset our reader needs — including the `serializedVersion` markers the importer checks
(91 → 5, each `AnimatorControllerLayer` sub-struct → 5; the per-parameter entries carry no
top-level `serializedVersion`). Empty sequences are emitted as `[]` (e.g. `m_Motions: []`,
`m_Behaviours: []`).

**Round-trip validation.** The generated controller is parsed back through
`avatar-unity-asset`'s `AnimatorController::from_file` (the repo's typed `.controller` reader):
the test confirms the controller name, that the blend parameter is read as a `Float`, that there is
exactly one state machine with a default state and one child state, that the blend tree references
the blend parameter, that Write Defaults is OFF on the state, and — explicitly — that the class-91
doc's first layer `m_StateMachine.fileID` equals the `AnimatorStateMachine` (1107) document's
fileID. Determinism is pinned by a same-seed-byte-identical test.

> **Unity-import caveat (the same "last mile" as below).** The round-trip proves the controller
> parses through *our* reader and that its internal cross-references resolve; it does **not** prove
> a *specific* Unity editor accepts it on import. The fields and `serializedVersion` markers are
> matched against Unity's stable serialization, but final acceptance — importing the `.controller`
> and seeing the layer drive the gesture in-game — remains the manual Unity/VRChat step this
> toolchain deliberately does not own (`PLAN.md` §1, §5).

```rust
use avatar_anim_gen::*;
let mut ids = IdGen::new("FX");
let tree = BlendTree::analog_gesture("Fist", "GestureLeftWeight")
    .clip("1234567890abcdef1234567890abcdef", 0.0)
    .clip("abcdef1234567890abcdef1234567890", 1.0);
let controller_text = fx_blend_tree("FX", "Base Layer", &tree, &mut ids);
```

```rust
use avatar_anim_gen::*;
let tree = BlendTree::analog_gesture("Fist", "GestureLeftWeight")
    .clip("1234567890abcdef1234567890abcdef", 0.0)  // relaxed
    .clip("abcdef1234567890abcdef1234567890", 1.0); // full fist
let mut e = Emitter::new();
tree.emit_tree(&mut e, 110600000);
let tree_doc = format!("{}{}", yaml_emit::UNITY_PREAMBLE, e.into_string());
println!("{}", tree.wiring_note(110600000));
```

### 4. Expression Parameters & Expressions Menu (`.asset`, class id 114)

`VRCExpressionParameters` and `VRCExpressionsMenu` are plain MonoBehaviour ScriptableObjects with
no internal fileID graph, so generation is a single-document emit (`expressions` module). The
contract points:

- **Script references.** `m_Script` points at the SDK class. Both classes are compiled into the
  SDK's `VRCSDK3A.dll` (GUID `67cc4cb7839cd3741b63733d5adf0442`), so the reference is
  `{fileID: <class hash>, guid: <dll guid>, type: 3}` — `-1506855854` for `VRCExpressionParameters`,
  `-340790334` for `VRCExpressionsMenu` (read off the SDK's own `DefaultExpressionParameters.asset`
  / `DefaultExpressionsMenu.asset` in `com.vrchat.avatars` 3.10.4; stable since SDK3 launched).
  `VRC_EXPRESSION_PARAMETERS_SCRIPT` / `VRC_EXPRESSIONS_MENU_SCRIPT` are the defaults;
  `.script(ScriptRef)` overrides both halves and `.script_guid(g)` (CLI `--script-guid`) treats
  the override as a loose `.cs` script (`11500000`) — a future SDK relocation is a flag, not a
  code change (`PLAN.md` risk 3).
- **Main-object fileID.** Both emit at Unity's ScriptableObject convention `&11400000`
  (`EXPRESSIONS_MAIN_FILE_ID`), which cross-asset references (`expressionsMenu:` on the descriptor,
  `subMenu:` on a control) expect.
- `ExpressionParams::new(name).parameter(ExpressionParamSpec::bool("Hat"))` /
  `ExpressionsMenu::new(name).control(MenuControlSpec::toggle("Hat", "Hat"))` are the builders;
  `ExpressionParamSpec` carries `valueType`/`saved`/`networkSynced`/`defaultValue`, and
  `MenuControlSpec::{toggle,button,sub_menu,radial}` cover control types 102/101/103/203.
  `ExpressionParams::synced_bits()` reports the 256-bit-budget cost before Unity is involved.

**Round-trip validation** is through `avatar-vrc-descriptor`'s *structural* reader — the same
classifier `avatar lint` trusts — so a generated asset is proven to read back as a Parameters/Menu
asset with the expected budget and controls.

### 4b. Gesture-driven FX layers (`gesture` module)

`GestureLayer` emits the idiomatic SDK3 face-expression layer: an `AnimatorStateMachine` with a
`Neutral` default state plus one state per gesture value that has a clip, and Any-State
`AnimatorStateTransition`s conditioned `GestureLeft`/`GestureRight` **Equals n** (`m_ConditionMode`
6), no exit time, fixed 0.1 s, `m_CanTransitionToSelf: 0` so a held gesture doesn't retrigger.
Gesture values with no clip route to `Neutral`, so the layer is authoritative for all eight; states
are Write Defaults off, so the `Neutral` clip should reset every shape the gesture clips touch.

A layer may read **several** parameters (`GestureLayer::either_hand`): each gesture state then gets
one transition per parameter and `Neutral` requires *all* of them to be 0 (multiple
`m_Conditions` = AND). One either-hand layer is SDK2's semantics (an override slot fired for
whichever hand made the gesture) and avoids the two-per-hand-layer clobber where the upper layer's
Neutral, resetting shared shapes under WD off, wipes the lower hand's expression. `fx_gestures`
wraps layers in the class-91 controller, declaring each `Int` parameter once. Used by
[`avatar-migrate`](migrate.md) to rebuild SDK2 gesture overrides.

**Analog mode** (`GestureLayer::analog()`): each gesture state's motion becomes a 1D
[BlendTree](#2-analog-gesture-1d-blendtree-class-id-206) on the gesture parameter's weight float
(`GestureLeft` → `GestureLeftWeight`), blending `Neutral` (threshold 0) → the gesture clip
(threshold 1) — trigger depth *is* expression depth, SDK2's Vive-wand "advanced controls"
semantics. In a multi-parameter (either-hand) layer each gesture gets one state **per parameter**
(`Fist L` / `Fist R`) so each hand blends on its own weight, still inside the single layer, and
`fx_gestures` declares the weight `Float`s.

Transition conditions are built **mutually exclusive** — two simultaneously-valid Any-State
transitions to different states ping-pong every crossfade (a visible oscillation whenever both
hands gesture). At most one target is valid at a time: **later parameters win** (`GestureLeft,
GestureRight` → the right hand takes the face when both act, VRChat's hands-layer convention),
and in analog mode "act" is weight-gated (`WEIGHT_ON` 0.05 to claim, `WEIGHT_OFF` 0.02 to
release; the gap is hysteresis) — necessary because a Vive-wand thumb resting on the touchpad
centre reports **Fist at weight 0**, and an ungated phantom would mask or oscillate against the
other hand's real expression. A lower-priority hand's transitions carry one alternative per way
the winning hand can be inactive (`== 0` or `weight < off`). When **both hands hold the same
gesture** (two-parameter analog layers), a dedicated `<Gesture> LR` state plays a **2D
freeform-cartesian tree** ([`BlendTree::freeform_2d`] + [`ChildMotion::at`]) over both weights
whose samples encode the **capped sum** `min(left + right, 1)`: Neutral at the origin, the full
clip on and past the `x + y = 1` diagonal, and optional half-strength clips
(`GestureLayer::motion_half`) as `(0.5, 0)`/`(0, 0.5)` midpoints — so 50 % + 10 % lands near
60 % instead of the second hand restarting the expression; per-hand transitions gain `NotEqual`
guards so the exclusivity invariant holds. Caveat: on controllers whose weight only tracks an
analog axis for some gestures (Index: Fist), other gestures need the trigger held to show — the
same trade SDK2 made on wands.

### 4c. Radial-puppet grafting (`avatar anim-gen puppet`)

`avatar anim-gen puppet --controller FX.controller --parameters Parameters.asset --menu
Menu.asset --param Blink --clip <neutral-guid>@0 --clip <pose-guid>@1 [--menu-name N]
[--layer-name L] [--on 0.01 --off 0.005] [--unsaved] [--default-value V]` grafts an analog dial
into an **existing** avatar, splicing (via `EditableUnityFile`, fileIDs/formatting preserved):

- the float parameter + a new layer into the FX controller, whose state machine is
  `BlendTree::to_gated_layer_fragment`: a default `Off` state that plays **nothing** (WD off —
  the layer is inert and lower layers keep the properties) and an `On` state playing the 1D tree,
  entered above `--on` and left below `--off` (hysteresis);
- the float into the `VRCExpressionParameters` asset (8 sync bits, saved by default);
- a `RadialPuppet` control into the `VRCExpressionsMenu` asset.

Built for the mikunpc calm-blink dial (the wand touchpad centre wouldn't deliver the Fist
gesture, so the same Neutral→eyes-closed blend the Fist state plays became an Action-Menu radial
— dial depth = blink depth). `avatar lint` cross-checks the three assets after the splice.

### 5. The toggle bundle (`avatar toggle`) — the end-to-end composite

A working in-game toggle needs five cooperating assets; the `toggle` module assembles all of them
as one internally-consistent bundle (`generate_toggle(ToggleSpec) -> ToggleBundle`):

| File | Content |
|------|---------|
| `<N>_On.anim` / `<N>_Off.anim` | every target held at its on-value / written back to 0 (authoritative both ways under Write Defaults OFF) |
| `<N>_FX.controller` | a `Bool` parameter + a two-state layer: `Off` (default) ⇄ `On`, instant transitions (`m_HasExitTime: 0`, duration 0) conditioned `If`/`IfNot` on the parameter |
| `<N>_Params.asset` | the `Bool` expression parameter (1 sync bit) |
| `<N>_Menu.asset` | a Toggle control driving it |
| `*.meta` sidecars | **deterministic GUIDs** (`deterministic_guid`, double-FNV-1a, first char forced to a letter — see the `CLAUDE.md` all-digit-guid gotcha) so the controller's clip references resolve on first import: Unity adopts an existing `.meta`'s guid instead of minting one |

Targets are GameObject-active paths (`ToggleTarget::GameObject`) and/or blendshape weights
(`ToggleTarget::Blendshape`); `default_on` flips both the layer's default state and the parameter's
default value. The bundle's `wiring_note` walks the user (or agent) through descriptor hookup and
merging into existing params/menu assets.

## CLI surface

Seven subcommands in `avatar-cli` (`cmd/anim_gen.rs`, `cmd/toggle.rs`) drive the builders above:

| Command | Emits | Key flags |
|---------|-------|-----------|
| `avatar anim-gen clip --name N [--blendshape PATH:SHAPE:VALUE]… [--toggle PATH]…` | a `.anim` AnimationClip | — |
| `avatar anim-gen blendtree --name N [--parameter P] [--clip GUID@THRESHOLD]… [--tree-only]` | the blend-tree fragment (state-machine trio, or `--tree-only` the bare 206 doc) | — |
| `avatar anim-gen controller --name N [--layer L] [--parameter P] [--clip GUID@THRESHOLD]…` | a complete FX `.controller` (`fx_blend_tree`) | — |
| `avatar anim-gen params --param NAME:TYPE[:DEFAULT][:unsaved][:local]…` | a `VRCExpressionParameters` `.asset` | `--script-guid` |
| `avatar anim-gen menu [--toggle L:P[:V]]… [--button L:P[:V]]… [--radial L:P]… [--submenu L:GUID]…` | a `VRCExpressionsMenu` `.asset` (≤ 8 controls enforced) | `--script-guid` |
| `avatar anim-gen puppet --controller C --param P [--parameters A] [--menu M] [--clip GUID@THRESHOLD]…` | a radial-puppet dial grafted into the *existing* controller/params/menu, in place (§4c) | `--menu-name`, `--layer-name`, `--on`/`--off`, `--unsaved`, `--default-value` |
| `avatar toggle --name N [--toggle PATH]… [--blendshape PATH:SHAPE:VALUE]… -o DIR` | the ten-file toggle bundle above | `--param`, `--menu-label`, `--unsaved`, `--default-on` |

`avatar toggle` writes into a *directory* (`-o DIR`, created if missing); the overwrite check runs
across the whole bundle before any file is written, so a partial bundle is never left behind.

Shared flags on the five single-asset emitters (`clip`/`blendtree`/`controller`/`params`/`menu`;
`puppet` edits its targets in place behind the same `--dry-run`/`--force` guard):

- **`-o, --output <file>`** writes the generated YAML *asset* to a file; without it the YAML goes to
  stdout.
- **`--dry-run`** previews without writing (reports the byte count and target to stderr, creates
  nothing). **`--force`** permits overwriting an existing output file; without it a write to an
  existing path is refused, so a generator run can never silently clobber an asset.
- **`--json`** switches stdout from the raw YAML to a machine-readable report — the allocated
  fileID(s), the parsed child clips, the wiring note (for `blendtree`), the output path, a `written`
  flag, and the YAML itself embedded under `yaml`. This lets an agent wire the asset (e.g. point a
  layer's `m_StateMachine` at the reported `state_machine_file_id`) without parsing YAML. With
  `--json`, `-o` still controls where the YAML asset is written; the JSON report is the stdout
  channel. The schema of these reports is informal (they are `serde_json` objects); the *report
  crates'* schemas are published via `avatar schema` (see [CLI README](../../crates/cli/README.md)).

## The fileID strategy

Unity identifies every object in a file by a 64-bit local `fileID`. Generated ids must be:

1. **Deterministic** — no randomness. The same input yields byte-identical output, which keeps
   generated assets diffable and CI reproducible. (`Math.random`/`Date`-style entropy is also simply
   unavailable in this generator, by design.)
2. **Collision-free within a file** — two objects in the same stream must not share an id.

`IdGen` provides both. `IdGen::new(seed)` hashes a caller-supplied name (the asset/object name)
with **FNV-1a** — chosen because it is stable across platforms and runs, unlike `DefaultHasher`,
whose output is explicitly not guaranteed stable — and masks the result into a positive `~10^15`
range to form a base. `IdGen::alloc()` then hands out sequential ids from that base. The
name-derived base means two independently-generated assets land in different ranges (so they don't
accidentally collide if later combined), and the counter guarantees uniqueness within one file.

The canonical fixed ids Unity uses for sub-asset references are reproduced where they are
load-bearing: a `.anim`'s AnimationClip is referenced from a blend tree as local fileID `7400000`,
and a loose `.cs` MonoBehaviour script as `11500000` (a class inside a DLL uses Unity's per-class
hash instead — see the expression assets above).

## How the YAML is emitted

`yaml_emit` is a small string-based emitter, not a generic YAML serializer — the documents are
fixed in shape, so a typed-then-rendered approach is clearer and easier to diff against real Unity
output. It enforces the conventions Unity's serializer uses and an import depends on:

- two-space block indentation; sequence entries are `- ` at the parent's indent;
- small fixed structs as **inline flow maps** (`{x: 0, y: 0, z: 0}`, `{fileID: N}`,
  `{fileID: N, guid: G, type: T}`); a null reference is `{fileID: 0}`;
- floats printed Unity-style — integral values as bare integers (`0`, `1`, not `0.0`), fractional
  values at shortest round-tripping precision (`fmt_f32`);
- the `%YAML 1.1` / `%TAG !u! tag:unity3d.com,2011:` preamble on a full file.

`ObjectRef` models a reference (`local`, `external`, `null`) and renders the inline form.

## Acceptance: what's proven, and the last mile

The in-repo tests (inline `#[cfg(test)]` in each module) prove the generator is **self-consistent
and produces valid multi-document Unity YAML**: they build a clip and a blend tree, assert the
emitted text contains the expected document headers (`--- !u!74`, `--- !u!206`, `!u!1107`,
`!u!1102`) and the exact field names/values, and — the key invariant — **parse the generated output
back through `avatar-unity-yaml`** (the repo's Unity-YAML reader), confirming the class ids, file
ids, names, and nested binding fields survive a real parse and that the fragment's cross-references
resolve (state machine → default state → motion → blend tree). The fileID allocator's determinism
is pinned by a same-seed-same-sequence test.

What the in-repo tests *cannot* prove is that a *specific* Unity editor accepts the asset on import.
That gap is now closed by a headless **Unity-acceptance gate** (`GeneratedAssetAcceptance.cs`, run
by `.github/workflows/unity-acceptance.yml`): it imports CLI-generated `.anim`/`.controller` assets
in a real editor and asserts each parses into the expected object type (`AnimationClip` with curves;
`AnimatorController` with parameters, a layer, and a state whose motion is a `BlendTree`) with **no
import errors logged**. Like the `armature fix` humanoid gate it shares a workflow with, it
self-skips until a `UNITY_LICENSE` secret is configured, so the field shapes here are matched against
Unity's stable serialization *and* — when the license is present — verified against the real
importer. The only step that remains genuinely manual is the in-game behaviour check (pulling a
trigger and seeing the gesture blend), the interactive Unity/VRChat step this toolchain deliberately
doesn't own (`PLAN.md` §1, §5).
