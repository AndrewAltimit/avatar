# avatar

> **Status: early / alpha (WIP).** The crates build green and are covered by tests, but those tests
> run against *synthetic fixtures* — the tools are **not yet validated against real Unity imports or a
> live VRChat**. Treat output as a starting point, expect rough edges, and verify in Unity before you
> rely on it. APIs, CLI flags, and report formats may change.

A Rust monorepo of tools for working with **VRChat avatars** (Unity, SDK3 / Avatars 3.0).

The goal: bring in an FBX or a Unity avatar project, **diagnose and fix common problems**
(armature/rig not set up right, broken humanoid mapping, SDK3-noncompliant descriptors),
**generate** animation/expression assets, and eventually **drive avatars at runtime over OSC**
(analog gestures and other custom controls).

## What this is — and isn't

These tools operate on **files**: they parse, validate, transform, and generate the formats an
avatar is made of. They do **not** replace Unity or the VRChat SDK. The actual avatar *upload*
still happens in Unity via the VRChat SDK control panel (which requires interactive
VRChat-account login and is effectively Windows-only). The split looks like:

```
FBX / Unity project ──► [avatar tools] ──► diagnose · fix · validate · generate
                                                │
                                drop generated assets into Unity
                                                │
                                    (you) build & upload via VRCSDK
                                                │
                          [avatar osc] ──► drive parameters at runtime
```

See [`PLAN.md`](PLAN.md) for the full architecture and roadmap, and [`docs/`](docs/) for
format and subsystem references.

## Crates

| Crate | Purpose |
|-------|---------|
| [`avatar-fbx`](crates/fbx/README.md) | Load **and write** binary FBX: extract the model/object graph + connections into a typed scene, edit/serialize it back (`FbxDocument`), and extract geometry + skin/bind data (`meshes()`) |
| [`avatar-armature`](crates/armature/README.md) | Skeleton model, Unity-humanoid bone inference (hierarchy-aware), VRChat SDK3 rig validation, **and repair planning** (canonical renames applied natively; topology/scale/orientation flagged + emitted as a headless-Blender repair script) |
| [`avatar-mesh`](crates/mesh/README.md) | POD mesh + skin interchange (`RawMesh`); the format-agnostic hand-off between importers and the pose layer |
| [`avatar-gltf`](crates/gltf/README.md) | glTF 2.0 importer → the same `RawMesh` + skeleton (a second rig source alongside FBX) |
| [`avatar-pose`](crates/pose/README.md) | Runtime posing + skinning: `PosedSkeleton` → world matrices, GPU bone-matrix palette, CPU skinning, two-bone IK (renderer-agnostic) |
| [`avatar-input`](crates/input/README.md) | Backend-agnostic VR tracker input (`TrackerState`/`TrackerSource`): mock + OSC backends, OpenXR planned |
| [`avatar-unity-yaml`](crates/unity-yaml/README.md) | Reader **+ surgical editor** for Unity's YAML format (`.asset`/`.prefab`/`.unity`/`.meta`): `EditableUnityFile` span-splices value edits into an existing asset, preserving fileIDs/refs/formatting. Wired to `avatar asset set` |
| [`avatar-unity-asset`](crates/unity-asset/README.md) | Typed Unity asset graphs over the YAML reader (the `AnimatorController` + `AnimationClip` readers the controller/clip lint rules consume) |
| [`avatar-unitypackage`](crates/unitypackage/README.md) | Read Unity's `.unitypackage` (gzip+tar): summarize, extract into a Unity project tree, and cross-check an avatar against a world/map for co-import GUID/path conflicts. Wired to `avatar unitypackage` |
| [`avatar-vpm`](crates/vpm/README.md) | Discover a Unity/VPM project: manifest packages, editor version, asset paths |
| [`avatar-vrc-descriptor`](crates/vrc-descriptor/README.md) | Typed extraction of the VRChat Avatar Descriptor, Expression Parameters & Menus + SDK3 rule constants |
| [`avatar-lint`](crates/lint/README.md) | Diagnostics engine: SDK3 compliance rules over a project (params, menus, descriptor refs, visemes incl. the source-FBX cross-check, Write Defaults, animation clips, PhysBones/Avatar-Dynamics, missing scripts, Quest shaders) |
| [`avatar-stats`](crates/stats/README.md) | Offline VRChat **performance ranking** (Excellent→Very Poor) from an FBX (geometry) or a project's avatars (components), against PC + Android limits |
| [`avatar-anim-gen`](crates/anim-gen/README.md) | **Generate** Unity `.anim` clips, FX-layer blend trees + full FX `AnimatorController`s, VRC expression parameters/menus, and the composite **toggle bundle**, as Unity YAML (deterministic fileIDs/GUIDs) — wired to `avatar anim-gen` / `avatar toggle` |
| [`avatar-migrate`](crates/migrate/README.md) | **SDK2 → SDK3 migration** of an avatar project: descriptor/PipelineManager retyped in place, DynamicBone → PhysBone (the SDK's own rules), Cloth → PhysBone skirt, clutter stripped, gesture overrides → FX layer, rig-derived eye look, and a VCC-openable project tree around the rewritten prefab — wired to `avatar migrate sdk3`; plus **PhysBone tuning** on any SDK3 prefab (`avatar physbone list|set|split|stretch|flare|nudge`: values + per-chain curves, split chains, lengthen a skirt/tail, re-angle chains) |
| [`avatar-render`](crates/render/README.md) | **GPU preview** via wgpu: render an avatar (and a Unity world scene, with the avatar dropped at the world's spawn point) to a PNG, headless — plus an optional interactive **winit viewer** (orbit/zoom/walk). Wired to `avatar render` / `avatar view` |
| [`avatar-osc`](crates/osc/README.md) | VRChat **OSC runtime**: `/avatar/parameters`, `/input`, `/avatar/change` codec + UDP client and OSCQuery avatar-config parsing — the M5 runtime foundation, wired to `avatar osc` |
| [`avatar-osc-gestures`](crates/osc-gestures/README.md) | The **analog-gesture daemon** ("Vive advanced controls on any hardware"): controller trigger → `Gesture*`/`Gesture*Weight` over OSC, with deadzone + change detection. Wired to `avatar osc gestures` |
| [`avatar-mcp`](crates/mcp/README.md) | A domain-agnostic **MCP server** (stdio JSON-RPC): exposes the read/diagnose surface plus text-returning generation tools an agent host can discover + call. Wired to `avatar mcp serve` |
| [`avatar-cli`](crates/cli/README.md) | The `avatar` binary tying the above together |
| [`avatar-testkit`](crates/testkit/README.md) | Test-only (`publish = false`): the golden-snapshot harness + in-code synthetic-FBX builders behind the workspace's fixture corpus |

The asset-generation (`avatar-anim-gen`), OSC-runtime (`avatar-osc`), and analog-gesture daemon
(`avatar-osc-gestures`) crates are driven by the `avatar anim-gen` / `avatar osc` subcommands. The
daemon's production input backend (OpenXR) is the remaining on-device M5 piece; its mapping and OSC
send are done and run headless via a demo source. See [`PLAN.md`](PLAN.md) and
[`docs/tutorial.md`](docs/tutorial.md).

## Quick start

```sh
cargo build --workspace
cargo run -p avatar-cli -- describe path/to/model.fbx                  # one-shot snapshot (FBX or project)
cargo run -p avatar-cli -- describe path/to/model.fbx --json           # machine-readable, for agents
cargo run -p avatar-cli -- fbx inspect path/to/model.fbx
cargo run -p avatar-cli -- armature check path/to/model.fbx
cargo run -p avatar-cli -- armature fix path/to/model.fbx              # dry run: print the repair plan
cargo run -p avatar-cli -- armature fix path/to/model.fbx -o fixed.fbx # write a repaired FBX
cargo run -p avatar-cli -- armature fix path/to/model.fbx --blender-script fix.py  # full repair incl. geometry, via headless Blender
cargo run -p avatar-cli -- lint path/to/UnityProject                   # SDK3 compliance report
cargo run -p avatar-cli -- lint path/to/UnityProject --deny-warnings   # also fail CI on warnings
cargo run -p avatar-cli -- stats path/to/model.fbx                     # performance rank (geometry)
cargo run -p avatar-cli -- stats path/to/UnityProject                  # performance rank (components)
cargo run -p avatar-cli -- anim-gen clip --name Smile --blendshape Body:Smile:100 -o Smile.anim
cargo run -p avatar-cli -- anim-gen controller --name FX --clip <guid>@0.0 --clip <guid>@1.0 -o FX.controller  # full FX controller
cargo run -p avatar-cli -- anim-gen params --param Hat:bool --param Dim:float:0.5:local -o Params.asset  # VRCExpressionParameters
cargo run -p avatar-cli -- anim-gen menu --toggle Hat:Hat --radial Dim:Dim -o Menu.asset      # VRCExpressionsMenu
cargo run -p avatar-cli -- toggle --name Hat --toggle Armature/Head/Hat -o HatBundle/  # full toggle bundle: clips+FX+params+menu (+.metas)
cargo run -p avatar-cli -- migrate sdk3 <extracted-sdk2-project> -o out/ --name MyAvatar --drop-cloth --eyes Eye_L,Eye_R  # SDK2 avatar -> SDK3 project
cargo run -p avatar-cli -- physbone list Avatar.prefab                                     # every VRCPhysBone: chains + tuning
cargo run -p avatar-cli -- physbone set Avatar.prefab Hair --pull 0.3 --spring-curve "0:1,1:0.5" -o Avatar.prefab --force
cargo run -p avatar-cli -- physbone stretch Avatar.prefab SkirtRoot --factor 1.5 -o Avatar.prefab --force  # longer skirt
cargo run -p avatar-cli -- asset set Parameters.asset --path m_Name --value Params2   # surgical edit (round-trip-safe)
cargo run -p avatar-cli -- schema describe                            # JSON Schema for a --json report type
cargo run -p avatar-cli -- osc send VRCEmote 3                         # drive a running VRChat over OSC
cargo run -p avatar-cli -- osc query path/to/avatar-osc-config.json    # list an avatar's OSC parameters
cargo run -p avatar-cli -- osc gestures --seconds 10                   # analog-gesture daemon (demo sweep)
cargo run -p avatar-cli -- unitypackage info avatar.unitypackage       # summarize a .unitypackage (SDK, avatar/world)
cargo run -p avatar-cli -- unitypackage extract avatar.unitypackage -o proj   # unpack into a Unity Assets/ tree
cargo run -p avatar-cli -- unitypackage testbed avatar.unitypackage world.unitypackage  # co-import conflict check
cargo run -p avatar-cli -- render --avatar avatar.fbx -o preview.png    # offscreen GPU render of the avatar
cargo run -p avatar-cli -- render --avatar avatar.fbx --world world/Assets/Scene.unity -o in-world.png  # at the world's spawn
cargo run -p avatar-cli -- view   --avatar avatar.fbx --world world/Assets/Scene.unity  # interactive window (orbit/zoom/walk)
```

`avatar lint` exits non-zero when the report contains errors (or, with `--deny-warnings`, any
warnings), so it can gate CI directly.

`armature fix` rewrites a non-standard rig (e.g. a raw Mixamo export) so Unity auto-configures it
as a humanoid: it canonicalizes bone names (`mixamorig:LeftArm` → `LeftUpperArm`), which is what the
humanoid auto-mapper keys on. It is a **dry run by default** — pass `-o` to write. Mis-wired parent
topology and scale/orientation problems are *reported* but not auto-applied — they need a geometry
transform, not a metadata relabel (see
[`docs/reference/armature-repair.md`](docs/reference/armature-repair.md)).

> FBX support is **binary only** (FBX 7.x, the Autodesk/Unity/Blender default).
> ASCII FBX is not supported — re-export as binary.

**For agents / scripting.** `avatar describe <path> [--json]` is a one-call snapshot of an asset
(FBX structure + armature + geometry rank, or project lint + per-avatar rank). Every read and
generate command takes `--json`; `avatar schema [name|all]` publishes the JSON Schema of those
report types so the output shape is a contract, not a guess. The generators (`anim-gen …`) are
write-safe: `--dry-run` previews without touching disk and an existing output file is never
overwritten without `--force`.

## Documentation

| Doc | What it covers |
|-----|----------------|
| [`PLAN.md`](PLAN.md) | Architecture + roadmap of record: layers, crates, milestones (M0–M5), decisions, risks. |
| [`CLAUDE.md`](CLAUDE.md) | Repo-internal guidance: conventions, commands, status, gotchas (also mirrors this map). |
| [`docs/README.md`](docs/README.md) | Index of the `docs/` directory. |
| [`docs/overview.md`](docs/overview.md) | The layered architecture in brief, with external references. |
| [`docs/tutorial.md`](docs/tutorial.md) | End-to-end walkthrough of the `avatar` CLI from FBX to lint to stats. |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Build/test/lint, conventions, and how to add a lint rule or crate. |
| [`docs/reference/humanoid-bones.md`](docs/reference/humanoid-bones.md) | Unity humanoid bones + VRChat rig requirements. |
| [`docs/reference/sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md) | Every `avatar lint` rule (`VRC001`–`VRC061`) + encodings. |
| [`docs/reference/unity-asset.md`](docs/reference/unity-asset.md) | `avatar-unity-asset`: the typed AnimatorController (`.controller`) reader the controller lint rules consume. |
| [`docs/reference/armature-repair.md`](docs/reference/armature-repair.md) | What `avatar armature fix` repairs, and the FBX writer. |
| [`docs/reference/performance-stats.md`](docs/reference/performance-stats.md) | `avatar stats`: metrics (incl. particles & constraints), component recognition, and PC/Android threshold tables. |
| [`docs/reference/rig-runtime.md`](docs/reference/rig-runtime.md) | Runtime rig layer: skin/bind extraction, posing, IK, tracker input. |
| [`docs/reference/anim-gen.md`](docs/reference/anim-gen.md) | `avatar-anim-gen`: `.anim` clip + analog-gesture blend-tree generation. |
| [`docs/reference/unity-yaml-edit.md`](docs/reference/unity-yaml-edit.md) | `EditableUnityFile` / `avatar asset set`: surgical, round-trip-safe value + structural edits to an existing Unity asset. |
| [`docs/reference/migrate.md`](docs/reference/migrate.md) | `avatar migrate sdk3`: SDK2 → SDK3 migration — what is converted and how (SDK's PhysBone rules, script references, FX from overrides, eye look), output layout, limits. |
| [`docs/reference/physbone.md`](docs/reference/physbone.md) | `avatar physbone list|set|split|stretch|flare|nudge`: inspect + retune a prefab's PhysBones in place (curves, split, stretch, re-angle) and preview the prefab's pose with `avatar render --pose`. |
| [`docs/reference/osc-runtime.md`](docs/reference/osc-runtime.md) | `avatar-osc`: the OSC address space, codec, and OSCQuery config parsing. |
| [`docs/reference/unitypackage.md`](docs/reference/unitypackage.md) | `avatar-unitypackage`: the `.unitypackage` format, extraction, and the avatar-in-world testbed. |
| [`docs/reference/render.md`](docs/reference/render.md) | `avatar-render` / `avatar render` + `avatar view`: the wgpu preview, avatar rest-pose render, world rendering, avatar-at-spawn-in-world, and the interactive viewer. |

**Per-crate READMEs** (purpose · key API · status):
[`fbx`](crates/fbx/README.md) ·
[`armature`](crates/armature/README.md) ·
[`mesh`](crates/mesh/README.md) ·
[`gltf`](crates/gltf/README.md) ·
[`pose`](crates/pose/README.md) ·
[`input`](crates/input/README.md) ·
[`unity-yaml`](crates/unity-yaml/README.md) ·
[`unity-asset`](crates/unity-asset/README.md) ·
[`unitypackage`](crates/unitypackage/README.md) ·
[`render`](crates/render/README.md) ·
[`vpm`](crates/vpm/README.md) ·
[`vrc-descriptor`](crates/vrc-descriptor/README.md) ·
[`lint`](crates/lint/README.md) ·
[`stats`](crates/stats/README.md) ·
[`anim-gen`](crates/anim-gen/README.md) ·
[`migrate`](crates/migrate/README.md) ·
[`osc`](crates/osc/README.md) ·
[`osc-gestures`](crates/osc-gestures/README.md) ·
[`cli`](crates/cli/README.md).

## Contributing

This repository does **not** accept external contributions, feature requests, or support requests.
It is a single-maintainer project authored by the maintainer and AI agents under human direction.
You are free to fork, study, and use it under the licenses below — see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## License

Dual-licensed under [MIT](LICENSE-MIT) OR [Unlicense](UNLICENSE), at your option.
