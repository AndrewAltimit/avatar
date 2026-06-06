# CLAUDE.md — repo-internal guidance

Internal guidance for working in this repo. This file is the **map of the repo's documentation**:
a brief overview plus a table of contents pointing at every other markdown file. User-facing docs
live in `README.md`, `PLAN.md`, and `docs/`.

**TL;DR:** a Rust monorepo (edition 2024, workspace, `anyhow` + `clap`) of tools that operate on the
*files* a VRChat SDK3 avatar is made of — FBX and Unity YAML — to diagnose, fix, lint, and (later)
generate them, plus an OSC runtime layer. Two layers (3D `.fbx` and Unity/VRChat YAML) are ours; the
VRChat upload step is not. Built and green through **M3** (FBX read/write + armature fix, project
linting), with the **M4** asset-generation (`avatar-anim-gen`) and **M5** OSC-runtime (`avatar-osc`)
layers now landed and wired to the `avatar anim-gen` / `avatar osc` subcommands — including the
analog-gesture daemon (`avatar-osc-gestures`, `avatar osc gestures`); only its on-device OpenXR input
backend remains. See [`PLAN.md`](PLAN.md) for the roadmap and the [Status](#status) section below.

## Documentation map

| Doc | What it covers |
|-----|----------------|
| [`README.md`](README.md) | User-facing overview, crate table, quick-start commands. |
| [`PLAN.md`](PLAN.md) | Architecture + roadmap of record: layers, crates, milestones (M0–M5), decisions, risks. |
| `CLAUDE.md` *(this file)* | Repo-internal guidance: conventions, commands, status, gotchas. |
| [`docs/README.md`](docs/README.md) | Index of the `docs/` directory. |
| [`docs/overview.md`](docs/overview.md) | The layered architecture in brief, with external references. |
| [`docs/reference/humanoid-bones.md`](docs/reference/humanoid-bones.md) | Unity humanoid bones + VRChat rig requirements. |
| [`docs/reference/sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md) | Every `avatar lint` rule (`VRC001`–`VRC044`) + encodings. |
| [`docs/reference/armature-repair.md`](docs/reference/armature-repair.md) | What `avatar armature fix` repairs, and the FBX writer. |
| [`docs/reference/rig-runtime.md`](docs/reference/rig-runtime.md) | Runtime rig layer: skin/bind extraction, posing + bone-matrix palette, two-bone IK, tracker input. |
| [`docs/reference/performance-stats.md`](docs/reference/performance-stats.md) | `avatar stats`: performance-rank metrics (incl. particles & constraints), component recognition, PC/Android threshold tables. |
| [`docs/reference/anim-gen.md`](docs/reference/anim-gen.md) | `avatar-anim-gen`: `.anim` clip + analog-gesture blend-tree generation (Unity-YAML emitter, deterministic fileIDs). |
| [`docs/reference/osc-runtime.md`](docs/reference/osc-runtime.md) | `avatar-osc`: VRChat OSC address space, codec, UDP client, OSCQuery avatar-config parsing. |
| [`docs/tutorial.md`](docs/tutorial.md) | End-to-end CLI walkthrough (FBX → armature → lint → stats). |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | Contributor guide: build/test/lint, conventions, adding a lint rule or crate. |

**Per-crate READMEs** (purpose · key API · status):
[`fbx`](crates/fbx/README.md) ·
[`armature`](crates/armature/README.md) ·
[`mesh`](crates/mesh/README.md) ·
[`gltf`](crates/gltf/README.md) ·
[`pose`](crates/pose/README.md) ·
[`input`](crates/input/README.md) ·
[`unity-yaml`](crates/unity-yaml/README.md) ·
[`unity-asset`](crates/unity-asset/README.md) ·
[`vpm`](crates/vpm/README.md) ·
[`vrc-descriptor`](crates/vrc-descriptor/README.md) ·
[`lint`](crates/lint/README.md) ·
[`stats`](crates/stats/README.md) ·
[`anim-gen`](crates/anim-gen/README.md) ·
[`osc`](crates/osc/README.md) ·
[`osc-gestures`](crates/osc-gestures/README.md) ·
[`cli`](crates/cli/README.md).

## What this repo is

A Rust monorepo of tools for VRChat avatars (Unity, SDK3 / Avatars 3.0). It operates on the
*files* an avatar is made of — FBX, Unity YAML assets, VRChat descriptor/menu/parameter assets —
to diagnose, validate, fix, and generate them, plus an OSC runtime layer. It does **not** replace
Unity or the VRChat SDK upload step (interactive VRChat-account login, effectively Windows-only).

## The two-layer model (read this first)

1. **3D asset layer — `.fbx`**: armature/skeleton, bones, bind poses, skinning, blendshapes.
   Fully ours in Rust (`fbxcel`) — read *and* write: native binary write-back landed in M3
   (`avatar_fbx::FbxDocument`), resolving the long-standing PLAN §8 risk.
2. **Unity/VRChat project layer — UnityYAML** (`.anim`, `.controller`, `.asset`, `.prefab`,
   scene, `.meta`): humanoid mapping, Avatar Descriptor, animator layers, expression menus/params.
   Read/lint is low-risk; *generating* assets Unity accepts requires correct fileIDs/GUIDs.

## Conventions (mirrors legend-of-legaia-re)

- Workspace: `resolver = "3"`, edition 2024, `version`/`edition`/`license` inherited from
  `[workspace.package]`. License: `MIT OR Unlicense`.
- Crate dirs are unprefixed (`crates/fbx`); package names are `avatar-<slug>`; lib names are
  `avatar_<slug>`. Binary is `avatar` (in `crates/cli`).
- Shared deps go in `[workspace.dependencies]` and are used via `.workspace = true`. Other deps
  are pinned to a major version, declared per-crate.
- Error handling: `anyhow` everywhere (`Result`, `Context`, `bail`). No `thiserror`/`eyre`.
- CLI: `clap` v4 derive. Logging (binaries only): `log` + `env_logger`. Reports: `serde`/`serde_json`.
- Manual, transparent parsing over heavy combinator frameworks — match the format byte/field for
  field and validate with `bail`, like the Legaia format crates.
- Integration tests live in `crates/<name>/tests/` and are **gated by an env var** pointing at a
  sample asset (e.g. `AVATAR_SAMPLE_FBX`); if unset, the test prints a skip notice and returns OK
  so CI without fixtures stays green. Never commit user FBX/Unity projects (see `.gitignore`).

## Commands

```sh
cargo build --workspace
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
cargo run -p avatar-cli -- <subcommand>
```

Install the pre-commit hook (fmt + clippy) with `scripts/install-hooks.sh`.

## Status

M0 (scaffold), M1 (armature diagnosis, hierarchy-aware humanoid mapping), M2 (project
SDK3 linting: `avatar lint <project>`), and M3 (armature repair: `avatar armature fix`) are built
and green. M2 lints Expression Parameters/Menus, the VRC Avatar Descriptor parsed from
prefabs/scenes (expression + playable-layer references resolved via a guid→path `.meta` index,
viseme lip-sync), animator-controller contents (`.controller`, via the `avatar-unity-asset` crate:
parameter references, default states, Write Defaults consistency, duplicate params), and
project/VPM info. Lint rule codes: `docs/reference/sdk3-lint-rules.md`. Roadmap and crate plan:
`PLAN.md`.

**Performance stats (built):** `avatar stats <path>` (crate `avatar-stats`) computes VRChat's
performance ranking offline — `analyze_fbx` for geometry (triangles/meshes/material-slots/bones),
`analyze_project` for per-avatar components (PhysBones/colliders/contacts/particles/lights/renderers,
VRChat dynamics recognized structurally, the rest by Unity class id). The project path also resolves
**triangles** (renderer `m_Mesh` guid → source FBX via the `.meta` index, distinct files summed once,
unresolved meshes flagged), **bones** (distinct `m_Bones` transforms), and **texture memory for
both PC and Android** (renderer → material → texture chain; per-texture VRAM estimated from
image-header dimensions + the `.meta` `TextureImporter` per-platform format — DXT/BC vs ASTC/ETC2 —
an estimate, since the imported GPU format isn't knowable offline; `MetricStat` carries a per-platform
value for this), and **PhysBone affected-transform & collision-check counts** (walk the transform
hierarchy under each PhysBone's `rootTransform` — descendants minus `ignoreTransforms`, plus an
endpoint per chain tip when `endpointPosition` is set, × assigned colliders; an estimate, PhysBones
with an unresolvable root flagged), for a unified geometry+component rank. It also estimates **total
particle count** (per `ParticleSystem`: `min(maxParticles, ceil(rate × lifetime))`, summed;
unparseable systems flagged) and **constraint count + depth** (Unity built-in constraint class ids
320–325 + VRChat constraint `MonoBehaviour`s recognized structurally; depth = longest
constraint→source chain via a cycle-safe walk, reported only when every source resolves). Overall =
worst measured metric; what remains in `not_evaluated` is now only the particle sub-metrics (mesh-
particle polygons, trail/collision flags). PC+Android limit tables are data on `Metric::limits`.
Behaviour: `docs/reference/performance-stats.md`.

**Runtime rig layer (built, green):** the "load and drive a rig at runtime" foundation for the
Legaia VR spectator (PRD §9), renderer-agnostic. New crates: `avatar-mesh` (POD `RawMesh`/skin
interchange, no `glam`), `avatar-fbx::meshes()` + `avatar-gltf` (FBX/glTF → `RawMesh` + skin/bind),
`avatar-pose` (`PosedSkeleton` → world matrices, GPU bone-matrix palette, CPU skinning, `pose::ik`
two-bone IK), and `avatar-input` (`TrackerState`/`TrackerSource`; `MockSource` + `osc` feature
backend; OpenXR planned). `glam` (f32) is confined to pose/input/gltf — never the lint/cli graph.
Bind comes from `TransformLink`/inverse-bind (never recomposed from `Lcl`+`PreRotation`); the
load→pose→skin pipeline is validated by a renderer-free **rest-pose reproduction** invariant.
Behaviour: `docs/reference/rig-runtime.md`.

**Asset generation — M4 (built; library + CLI):** `avatar-anim-gen` emits Unity-YAML `.anim` clips
(`AnimationClip`, class 74 — blendshape-weight and GameObject-active curves) and FX-layer analog-
gesture blend trees (`BlendTree`, class 206, blending `GestureLeft/RightWeight`). A faithful
hand-written YAML emitter (`yaml_emit`) handles Unity's exact field names, block indentation, and
flow maps; `IdGen` hands out **deterministic** FNV-1a-seeded fileIDs so generated assets are
diffable/reproducible, and every generated clip round-trips through the `avatar-unity-yaml` reader in
tests. Driven by `avatar anim-gen blendtree` / `avatar anim-gen clip` (write to stdout or `-o`).
Behaviour: `docs/reference/anim-gen.md`.

**OSC runtime — M5 (built; library + CLI):** `avatar-osc` speaks VRChat's OSC *parameter* protocol —
`/avatar/parameters/*`, `/input/*` (axes/buttons, reset-to-zero), `/avatar/change` — split into a
**pure** `codec` (encode/decode `rosc::OscMessage`, fully unit-tested) and a thin non-blocking
`ParamClient` UDP transport (send to VRChat :9000, poll :9001). `query` parses an avatar's OSCQuery
config JSON (`AvatarConfig`: parameter names, OSC type tags, read/write access) offline. Distinct
from the VMC *tracker* OSC in `avatar_input::osc` (transforms that drive a local rig, not avatar
parameters). Driven by `avatar osc send|input|monitor|change|query`. Behaviour:
`docs/reference/osc-runtime.md`.

**Analog-gesture daemon — M5 (built; library + CLI):** `avatar-osc-gestures` is the "Vive advanced
controls on any hardware" feature: read a controller's analog trigger/grip → map to a VRChat gesture
(`GestureLeft/Right` int 0–7) + analog `Gesture*Weight` → send via `avatar-osc`'s `ParamClient`.
Deliberately **glam-free** — it defines its own minimal `AnalogSource` (per-hand trigger/grip floats)
instead of depending on `avatar-input`, so the cli graph stays out of `glam` (verified: `cargo tree`
shows no glam under cli/osc-gestures). Pure `HandMapping` (deadzone rescale `(dz,1]→(0,1]`, Fist by
default, optional grip gesture) + `GestureFrame::updates_since` change detection (only changed
params re-sent) + `GestureDaemon` loop (`tick`/`run`/`run_for`) over a `ParamSink` (impl for
`ParamClient`). On-device input is OpenXR (an `AnalogSource` adapter, pending); headless uses a
deterministic `DemoSource` triangle sweep. Driven by `avatar osc gestures` (`--hz`/`--period`/
`--seconds`). Behaviour: `docs/reference/osc-runtime.md`.

M3 resolves the project's biggest risk — native binary FBX **write-back**. `avatar-fbx`'s
`FbxDocument` retains `fbxcel` 0.9's mutable tree (enable the `writer` feature) and serializes via
`Writer::write_tree`/`finalize`; it edits objects by FBX **object id** (skin/anim refs are by id,
not name, so renames are safe). `avatar-armature::repair` applies canonical humanoid **renames**
(the only native repair), and **flags** mis-wired parent topology + scale/orientation problems
(not applied — those need a geometry transform, i.e. Blender, not a metadata relabel: re-pointing a
bone's `OO` connection without recomposing its local transform would move its rest/bind pose). The
low-level `reparent_object` primitive is retained but not wired into `apply_plan`. `fbxcel`'s
`write_tree` re-emits arrays uncompressed (written FBX is larger; semantically identical).
Behaviour: `docs/reference/armature-repair.md`.

Note when authoring Unity-YAML test fixtures: a Unity GUID is 32 hex chars and must contain
letters (e.g. `aaaa…`); an all-digit "guid" is parsed by yaml-rust2 as a *number*, so `as_str()`
returns `None` and guid resolution silently breaks.
