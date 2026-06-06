# Overview

These tools work on the *files* a VRChat avatar (Unity, SDK3 / Avatars 3.0) is made of: parse them,
validate them against the VRChat/Unity specs, transform them, and generate new ones — plus a runtime
OSC layer. They do **not** replace Unity or the VRChat SDK upload step.

For the full architecture, crate plan, and roadmap see [`../PLAN.md`](../PLAN.md). For repo-internal
conventions see [`../CLAUDE.md`](../CLAUDE.md).

## Layers

1. **3D asset layer — `.fbx`** (`avatar-fbx`, `avatar-armature`). The armature/skeleton lives here.
   Binary FBX only.
2. **Unity/VRChat project layer — UnityYAML** (`avatar-unity-yaml`, `avatar-unity-asset`,
   `avatar-vrc-descriptor`, `avatar-vpm`). Avatar Descriptor, humanoid mapping, animators, menus.
3. **Logic** (`avatar-lint`, `avatar-anim-gen`). Diagnostics and asset generation.
4. **Runtime** (`avatar-osc`, `avatar-osc-gestures`). Drive avatar parameters over OSC.

## Docs map

See [`docs/README.md`](README.md) for the full index. In short:

- `docs/reference/` *(exists)* — VRChat SDK3 lint rules, the Unity humanoid bone table, and the
  armature-repair behaviour reference.
- `docs/formats/` *(planned)* — byte/field-level format references (FBX, UnityYAML, `.anim`, `.controller`).
- `docs/subsystems/` *(planned)* — how each subsystem works (asset generation, OSC daemon) as it lands.

## External references

- VRChat: [Avatars 3.0](https://creators.vrchat.com/avatars/), [Rig Requirements](https://creators.vrchat.com/avatars/rig-requirements/),
  [Animator Parameters](https://creators.vrchat.com/avatars/animator-parameters/),
  [OSC overview](https://docs.vrchat.com/docs/osc-overview), [OSC Avatar Parameters](https://docs.vrchat.com/docs/osc-avatar-parameters).
- VPM / Creator Companion: [vcc.docs.vrchat.com](https://vcc.docs.vrchat.com/).
- FBX: [fbxcel](https://github.com/lo48576/fbxcel).
