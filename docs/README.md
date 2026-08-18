# docs

Documentation for the [avatar](../README.md) monorepo. Start with the root
[`README.md`](../README.md) (what this is) and [`PLAN.md`](../PLAN.md) (architecture + roadmap).

## In this directory

- [`overview.md`](overview.md) — the layered architecture in brief, with external references.
- [`tutorial.md`](tutorial.md) — end-to-end walkthrough of the `avatar` CLI: inspect an FBX, check
  and fix the armature, lint a project, and read a performance rank.

### `reference/` — specs and behaviour

- [`reference/humanoid-bones.md`](reference/humanoid-bones.md) — Unity humanoid bones and VRChat rig
  requirements; the table `avatar-armature` infers and validates against.
- [`reference/sdk3-lint-rules.md`](reference/sdk3-lint-rules.md) — every `avatar lint` rule
  (`VRC001`–`VRC052`), how assets are identified, and the valueType/budget encodings.
- [`reference/unity-asset.md`](reference/unity-asset.md) — `avatar-unity-asset`: typed
  AnimatorController (`.controller`) reading — parameters, states, Write Defaults, blend trees — the
  reader the controller lint rules consume. (Mirrors the style of the other `reference/*.md` docs.)
- [`reference/armature-repair.md`](reference/armature-repair.md) — what `avatar armature fix`
  repairs, why renaming is safe and scale/orientation are only flagged, and the FBX writer's
  characteristics.
- [`reference/rig-runtime.md`](reference/rig-runtime.md) — the runtime rig layer: skin/bind
  extraction, posing + bone-matrix palette, two-bone IK, and backend-agnostic tracker input.
- [`reference/performance-stats.md`](reference/performance-stats.md) — `avatar stats`: the metrics
  each source measures (incl. particle and constraint estimation), how components are recognized, and
  the PC/Android threshold tables.
- [`reference/anim-gen.md`](reference/anim-gen.md) — `avatar-anim-gen`: generating `.anim` clips and
  analog-gesture blend trees as Unity YAML, the emitter, and the deterministic-fileID strategy.
- [`reference/unity-yaml-edit.md`](reference/unity-yaml-edit.md) — `EditableUnityFile` /
  `avatar asset set`: surgical, round-trip-safe value + structural edits to an *existing* Unity
  asset by span-splicing raw text (fileIDs/refs/key-order/formatting preserved).
- [`reference/migrate.md`](reference/migrate.md) — `avatar migrate sdk3`: SDK2 → SDK3 migration of an
  avatar project (descriptor, PhysBones by the SDK's own rules, Cloth → PhysBone, FX from gesture
  overrides, rig-derived eye look, output project layout, limits).
- [`reference/physbone.md`](reference/physbone.md) — `avatar physbone list|set|split|stretch`: inspect
  and retune a prefab's `VRCPhysBone`s in place (values + per-chain curves, split chains onto their
  own components, lengthen a skirt/tail by scaling bone offsets), and the `avatar render --stretch`
  preview.
- [`reference/osc-runtime.md`](reference/osc-runtime.md) — `avatar-osc`: the VRChat OSC address space,
  the pure codec, the UDP `ParamClient`, and OSCQuery avatar-config parsing.
- [`reference/unitypackage.md`](reference/unitypackage.md) — `avatar-unitypackage`: reading the
  `.unitypackage` format, extracting it into a Unity project tree, and the avatar-in-world testbed
  (co-import GUID/path conflict cross-check).
- [`reference/render.md`](reference/render.md) — `avatar-render` / `avatar render`: the offscreen
  wgpu preview pipeline, avatar rest-pose rendering (auto-upright), and experimental world-scene
  rendering with its limitations.
- [`reference/mcp.md`](reference/mcp.md) — `avatar-mcp` / `avatar mcp serve`: the stdio MCP server,
  the read-only tool registry, the handshake, and the two-layer error model.
- [`reference/testing.md`](reference/testing.md) — the fixture corpus and the `avatar-testkit`
  golden-snapshot harness: the three corpus layers, `golden::assert_json`/`redact_roots`, and the
  `UPDATE_GOLDEN` workflow.

## Planned directories

These are referenced by the roadmap but not yet populated:

- `formats/` — byte/field-level format references (FBX, UnityYAML, `.anim`, `.controller`).
- `subsystems/` — how each subsystem works (the analog-gesture OSC daemon) as it lands.
