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
  (`VRC001`–`VRC044`), how assets are identified, and the valueType/budget encodings.
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
- [`reference/osc-runtime.md`](reference/osc-runtime.md) — `avatar-osc`: the VRChat OSC address space,
  the pure codec, the UDP `ParamClient`, and OSCQuery avatar-config parsing.
- [`reference/unitypackage.md`](reference/unitypackage.md) — `avatar-unitypackage`: reading the
  `.unitypackage` format, extracting it into a Unity project tree, and the avatar-in-world testbed
  (co-import GUID/path conflict cross-check).

## Planned directories

These are referenced by the roadmap but not yet populated:

- `formats/` — byte/field-level format references (FBX, UnityYAML, `.anim`, `.controller`).
- `subsystems/` — how each subsystem works (the analog-gesture OSC daemon) as it lands.
