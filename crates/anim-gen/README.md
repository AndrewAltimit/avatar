# avatar-anim-gen

Unity `.anim` clip and FX-layer blend-tree **generation**. Package `avatar-anim-gen` · library
`avatar_anim_gen`. Part of the [avatar](../../README.md) monorepo.

## What it does

Emits Unity-YAML assets a generator can drop into a project and Unity will import — the M4
"asset generation" band (`PLAN.md` §4, §9). Two outputs:

- **AnimationClip** (`--- !u!74`, a `.anim`): a keyframed `m_FloatCurves` clip. Two worked cases —
  a **blendshape weight** curve (`blendShape.<name>` on a SkinnedMeshRenderer, class 137) and a
  **GameObject active toggle** (`m_IsActive`, class 1) — plus the `m_AnimationClipSettings` block.
- **Analog-gesture 1D BlendTree** (`--- !u!206`): the headline feature. A blend tree in the Fist
  gesture slot that blends `GestureLeftWeight`/`GestureRightWeight` (float 0→1) across child clips,
  so any gesture reaches any fraction. Emitted either as the bare 206 document (to graft into an
  existing FX controller) or as a self-contained `StateMachine`+`State`+`BlendTree` fragment.

The crate is purely a text generator: the hard part is faithful Unity serialization (exact field
names, 2-space block indentation, inline `{x: 0, y: 0}` / `{fileID: N}` flow maps, deterministic
`fileID`s). It depends on [`avatar-unity-yaml`](../unity-yaml/README.md) only to **read its own
output back** in tests.

## Key API

- `AnimationClip::new(name)` + `.float_curve(FloatCurve)` → `.to_unity_yaml(file_id)`.
  - `FloatCurve::blendshape(path, shape, keyframes)` / `FloatCurve::game_object_active(path, kf)`.
  - `Keyframe::flat(time, value)` / `Keyframe::new(time, value, in_slope, out_slope)`.
- `BlendTree::analog_gesture(name, param)` + `.clip(guid, threshold)` / `.child(ChildMotion)`.
  - `.emit_tree(&mut Emitter, file_id)` — the 206 document only.
  - `.to_state_fragment(&mut IdGen)` — the wired `StateMachine`/`State`/`BlendTree` trio.
  - `.wiring_note(tree_file_id)` — prose on how to graft the tree into an existing controller.
- `controller`: `AnimatorController`/`AnimatorLayer`/`AnimatorParameter`/`ParamType` + the
  `fx_blend_tree(name, layer_name, &BlendTree, &mut IdGen)` convenience — emit a full FX
  `AnimatorController` (class 91: `m_AnimatorParameters` + `m_AnimatorLayers` + state machine)
  wrapping a blend-tree layer.
- `IdGen::new(seed)` + `.alloc()` — deterministic (FNV-1a–seeded) `fileID` allocation; no randomness.
- `yaml_emit`: `Emitter`, `ObjectRef`, `fmt_f32` — the low-level Unity-YAML emitter.

## Status

Built: **M4**. Covers `.anim` float-curve clips, 1D analog-gesture blend trees, and full FX
`AnimatorController` emission (the layer list / class-91 object, via `controller::fx_blend_tree`).
All three are driven from the CLI (`avatar anim-gen clip|blendtree|controller`, each with `--json`
+ write-safe `--dry-run`/`--force`). They round-trip through `avatar-unity-asset`'s reader in-repo,
**and** — when a `UNITY_LICENSE` is configured — CLI-generated `.anim`/`.controller` assets are
imported into a real editor by the Unity-acceptance workflow (`GeneratedAssetAcceptance.cs`), so the
"live Unity import" gap is closed in CI. Out of scope for now: transform (position/scale/rotation)
curves and PPtr curves.

## See also

- [`docs/reference/anim-gen.md`](../../docs/reference/anim-gen.md) — the generator, the worked
  cases, and the fileID strategy.
- [`avatar-unity-asset`](../unity-asset/README.md) — the *reader* side of AnimatorControllers /
  blend trees, used by lint.
