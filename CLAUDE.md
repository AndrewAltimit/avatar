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
| [`docs/reference/sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md) | Every `avatar lint` rule (`VRC001`–`VRC052`) + encodings. |
| [`docs/reference/unity-asset.md`](docs/reference/unity-asset.md) | `avatar-unity-asset`: the typed AnimatorController (`.controller`) reader the controller lint rules consume. |
| [`docs/reference/armature-repair.md`](docs/reference/armature-repair.md) | What `avatar armature fix` repairs, and the FBX writer. |
| [`docs/reference/rig-runtime.md`](docs/reference/rig-runtime.md) | Runtime rig layer: skin/bind extraction, posing + bone-matrix palette, two-bone IK, tracker input. |
| [`docs/reference/performance-stats.md`](docs/reference/performance-stats.md) | `avatar stats`: performance-rank metrics (incl. particles & constraints), component recognition, PC/Android threshold tables. |
| [`docs/reference/anim-gen.md`](docs/reference/anim-gen.md) | `avatar-anim-gen`: `.anim` clip + analog-gesture blend-tree generation (Unity-YAML emitter, deterministic fileIDs). |
| [`docs/reference/osc-runtime.md`](docs/reference/osc-runtime.md) | `avatar-osc`: VRChat OSC address space, codec, UDP client, OSCQuery avatar-config parsing. |
| [`docs/reference/unitypackage.md`](docs/reference/unitypackage.md) | `avatar-unitypackage`: reading the `.unitypackage` format, extracting to a Unity project tree, the avatar-in-world co-import testbed. |
| [`docs/reference/render.md`](docs/reference/render.md) | `avatar-render` / `avatar render` + `avatar view`: offscreen wgpu preview pipeline, avatar rest-pose render (auto-upright), world-scene render, avatar-dropped-at-spawn-in-world, interactive winit viewer (orbit/zoom/walk) + limits. |
| [`docs/tutorial.md`](docs/tutorial.md) | End-to-end CLI walkthrough (FBX → armature → lint → stats). |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | No-external-contributions policy, then the internal dev reference: build/test/lint, conventions, adding a lint rule or crate. |

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
[`vpm`](crates/vpm/README.md) ·
[`vrc-descriptor`](crates/vrc-descriptor/README.md) ·
[`lint`](crates/lint/README.md) ·
[`stats`](crates/stats/README.md) ·
[`anim-gen`](crates/anim-gen/README.md) ·
[`render`](crates/render/README.md) ·
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

Install commit hooks with `scripts/install-hooks.sh` (uses the [pre-commit](https://pre-commit.com)
framework — `.pre-commit-config.yaml`: file hygiene + actionlint + shellcheck + local fmt/clippy;
falls back to the native fmt+clippy hook in `scripts/git-hooks/` if `pre-commit` isn't installed).
Sweep manually with `pre-commit run --all-files`.

## Status

M0 (scaffold), M1 (armature diagnosis, hierarchy-aware humanoid mapping), M2 (project
SDK3 linting: `avatar lint <project>`), and M3 (armature repair: `avatar armature fix`) are built
and green. M2 lints Expression Parameters/Menus, the VRC Avatar Descriptor parsed from
prefabs/scenes (expression + playable-layer references resolved via a guid→path `.meta` index,
viseme lip-sync), animator-controller contents (`.controller`, via the `avatar-unity-asset` crate:
parameter references, default states, Write Defaults consistency, duplicate params), and
project/VPM info. Seven rules landed on top of the M2 set: `VRC012` (Info: expression param
referenced by no menu/animator anywhere), `VRC022` (Warn: empty menu control), `VRC038` (Warn:
duplicate/empty viseme blendshape entry), `VRC045` (Warn: Write Defaults inconsistent across the
avatar's playable-layer controllers), and a new `VRC05x` PhysBones/Avatar-Dynamics group — `VRC050`
(unresolvable PhysBone root), `VRC051` (PhysBone moves zero transforms), `VRC052` (PhysBone has
collider slots but none wired). Lint rule codes (`VRC001`–`VRC052`): `docs/reference/sdk3-lint-rules.md`.
Roadmap and crate plan: `PLAN.md`.

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
constraint→source chain via a cycle-safe walk, reported only when every source resolves). The three
former `not_evaluated` particle sub-metrics now landed too — **Mesh Particle Polygons** (a mesh-mode
`ParticleSystemRenderer`'s mesh triangles × the system's particle count, resolved through the same
renderer→mesh→FBX chain as geometry — approximate), **Particle Trails**, and **Particle Collision**
(count of systems with `TrailModule`/`CollisionModule` enabled) — so `not_evaluated` is now empty by
default. Overall = worst measured metric. PC+Android limit tables are data on `Metric::limits`.
Behaviour: `docs/reference/performance-stats.md`.

**Runtime rig layer (built, green):** the "load and drive a rig at runtime" foundation for the
Legaia VR spectator (PRD §9), renderer-agnostic. New crates: `avatar-mesh` (POD `RawMesh`/skin
interchange, no `glam`), `avatar-fbx::meshes()` + `avatar-gltf` (FBX/glTF → `RawMesh` + skin/bind),
`avatar-pose` (`PosedSkeleton` → world matrices, GPU bone-matrix palette, CPU skinning, `pose::ik`
two-bone IK), and `avatar-input` (`TrackerState`/`TrackerSource`; `MockSource` + `osc` feature
backend; OpenXR planned). `glam` (f32) lives in pose/input/gltf/**render** — and, since the
`avatar render` command is built on the renderer, in the cli's `render_scene`/`world` modules too
(the older "never the cli graph" rule is relaxed for the renderer; it stays out of lint/descriptor).
Bind comes from `TransformLink`/inverse-bind (never recomposed from `Lcl`+`PreRotation`); the
load→pose→skin pipeline is validated by a renderer-free **rest-pose reproduction** invariant.
Behaviour: `docs/reference/rig-runtime.md`.

**In-engine preview — `avatar-render` + `avatar render` (built; textured avatar + world):**
`avatar-render` (lib `avatar_render`; `wgpu` 29 + `glam`/`bytemuck`/`png`, GPU-only — knows nothing of
FBX/Unity) is a headless **offscreen** pipeline: a `Scene` of world-space `RenderMesh`es + a texture
pool + `Camera` + `Light` → RGBA8 → PNG, no window (render-to-texture + readback; works over SSH/CI on
any GPU adapter — validated on NVIDIA GB10 via Vulkan). One merged vertex buffer, **per-texture index
batches** (one draw per texture), 4× MSAA + depth, two-sided Lambert (imported winding is
inconsistent), RH `0..1` camera, auto-framing. **Textured:** each `RenderMesh` carries UVs + an
optional index into `Scene.textures`; shader = texture × vertex tint with a **0.5 alpha cutout**
(foliage/hair/decal cards) + V-flip; untextured meshes bind a 1×1 white texel. Image decode
(PNG/JPEG/TGA/BMP/GIF via the `image` crate) lives in the **cli** (`texture.rs`: `TextureSet`
interns/dedups by key; `split_by_material` makes each material slot its own single-texture mesh) — the
renderer only uploads RGBA8. CLI `avatar render --avatar <fbx|gltf|glb> [--world <scene|project>] -o
out.png` with `--width/--height/--yaw/--pitch`; importer glue in
`crates/cli/src/{render_scene,world,texture}.rs`. **Avatar render (validated, correct):** uses **raw
control points** as bind geometry — deliberately NOT the FBX skin-bind matrices, since ripped/MMD→FBX
avatars ship inconsistent per-cluster `Transform`s that make LBS collapse into spikes; **auto-uprights**
by aligning the hips→head axis (measured from cluster centroids in control-point space — reliable
weights, not the broken bind matrices) to +Y; **textured** from FBX-embedded materials (per-slot
diffuse texture/colour). Renders the real SDK2 avatar standing/undistorted/textured. **World render
(built; geometrically faithful + textured):** parses a `.unity` scene (lossy Unity-YAML parse added to
`avatar-unity-yaml` — `UnityFile::parse_lossy` skips MonoBehaviour bodies yaml-rust2 rejects) and
emulates enough of Unity's **FBX import pipeline** to place geometry correctly: reads
`Transform`/`MeshFilter`/`MeshRenderer`/`PrefabInstance`, composes each scene transform up `m_Father`,
composes **FBX node-world transforms** (each mesh by its `Model` node's `Lcl` T/R/S up the `OO` parent
chain — assembles multi-mesh FBXs), applies the **import scale**
(`useFileScale`/`globalScale`×`UnitScaleFactor`/100, baked by Unity into the imported mesh — fixes
cm-unit props rendering 100× big), and **expands prefab instances** (`m_SourcePrefab`→FBX, every mesh
at `world(m_TransformParent)·root_local·import_scale·node_world`; root scale = import scale, a prefab
default never serialized — this is how the cabin shell renders). **Materials/textures:** direct props
resolve each `m_Materials` slot → `.mat` `_Color` + `_MainTex` (decoded); prefab models (no scene-side
material) resolve through the model importer's **material remap** (`.fbx.meta` `externalObjects`: FBX
material *name* → `.mat`), falling back to the **FBX-embedded** material's texture/colour. Validated vs
the Cozy Cabin world: cabin assembles at ~6 m with its trees (alpha-cut pine foliage), cabin remapped
materials, and props (clocks/iPad/pens) textured at correct real-world sizes inside it. **Limits:**
flat-lit (base-colour texture × tint, one Lambert light — no normal/metallic/emission maps, no blend
transparency beyond the 0.5 cutout, no lightmaps/custom shaders), `image`-decodable formats only (no
DDS/PSD/EXR → flat colour), no FBX pivots/pre-rotation/geometric-transform/`InheritType`, no prefab
nesting. Headless smoke test gated on adapter availability. **Avatar-in-world (built):** with both
`--avatar` and `--world`, the avatar is dropped at the world's **player-spawn point** (resolved from
the `VRC_SceneDescriptor`/`VRCWorld` transform), normalised to **human height** (1.6 m, any source
units) with its feet on the spawn, and the camera frames on it (`--frame avatar|world`). **Interactive
viewer (built; `avatar view`):** the same assembled scene in a native **winit** window — orbit (drag) /
zoom (wheel) / walk (WASD+Space/Shift) / reset (R), reusing the offscreen geometry+shader pipeline but
drawing to a live surface (`avatar-render`'s off-by-default `viewer` feature, on by default in the
cli; winit builds headlessly, needs a display only at runtime). Behaviour: `docs/reference/render.md`.

**Asset generation — M4 (built; library + CLI):** `avatar-anim-gen` emits Unity-YAML `.anim` clips
(`AnimationClip`, class 74 — blendshape-weight and GameObject-active curves) and FX-layer analog-
gesture blend trees (`BlendTree`, class 206, blending `GestureLeft/RightWeight`). A faithful
hand-written YAML emitter (`yaml_emit`) handles Unity's exact field names, block indentation, and
flow maps; `IdGen` hands out **deterministic** FNV-1a-seeded fileIDs so generated assets are
diffable/reproducible, and every generated clip round-trips through the `avatar-unity-yaml` reader in
tests. A new `controller.rs` module now emits a **full FX `AnimatorController`** (Unity class 91)
wrapping a blend-tree layer — `m_AnimatorParameters` + `m_AnimatorLayers` + the state machine — via
`AnimatorController`/`AnimatorLayer`/`AnimatorParameter`/`ParamType` and the `fx_blend_tree`
convenience, with deterministic fileIDs and a round-trip validated through `avatar-unity-asset`'s
reader (in-repo only — not yet a real Unity import). This closes the M4 "full FX animator-layer
assembly" gap. Driven by `avatar anim-gen blendtree` / `avatar anim-gen clip` (write to stdout or
`-o`). Behaviour: `docs/reference/anim-gen.md`.

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

**`.unitypackage` tooling + avatar-in-world testbed (built; library + CLI):** `avatar-unitypackage`
reads Unity's `.unitypackage` distribution format (a gzip+tar of `<guid>/{pathname,asset,asset.meta}`
members; just `flate2` + `tar`, deliberately out of the lint/cli dep graph's heavier crates). It
**summarizes** a package (counts, size-by-extension, and traits: `vrc_sdk` — read from the plugin
DLLs `VRCSDK2.dll`/`VRCSDK3*.dll` and VPM package paths, *not* the date-based `VRCSDK/version.txt` —
plus `looks_like_avatar`/`looks_like_world`), **extracts** it into a normal Unity `Assets/` tree
(asset bytes + `.meta` sidecars) so the existing `avatar lint`/`stats` and FBX/armature tools consume
it unchanged, and **cross-checks** two packages for co-import conflicts (`overlap`: GUID collisions —
flagged `identical` when bytes match — and path collisions). Extraction refuses non-project paths
(absolute POSIX, Windows drive `C:/…`, UNC, `..`); old SDK exports leak absolute editor-DLL paths.
Driven by `avatar unitypackage info|list|extract|testbed` (testbed has `--strict` for gating).
Validated against a real SDK2 avatar package and the Cozy Cabin world (PC/Quest) exports. Behaviour:
`docs/reference/unitypackage.md`.

**Robustness hardening (built):** the importers were hardened against malformed/hostile inputs.
`avatar-unitypackage` extraction now caps decompressed bytes per entry (512 MiB) and in total
(2 GiB) — a decompression-bomb guard. `avatar-gltf` rejects primitives with more than `u32::MAX`
vertices. `avatar-fbx` reads `UpAxis`/`FrontAxis` via checked `i32::try_from` instead of an `as`
cast. `avatar-unity-yaml`'s `parse_lossy` was refactored to be infallible by construction, removing
a latent panic path.

**Agent ergonomics (built):** four changes make the toolchain easier for agents to drive and trust.
(1) **`avatar describe <path>`** (crate `avatar-cli`, `cmd/describe.rs`) is a one-shot consolidated
snapshot — for an FBX it runs inspect + armature + geometry stats; for a project it runs lint +
per-avatar stats — so one call yields a full mental model instead of stitching four commands. Gates
non-zero on a non-humanoid-ready rig or lint errors. (2) **Dry-run-safe writes**: a shared
`WriteGuard` (`--dry-run`/`--force`) in `cmd/mod.rs` (`write_out_guarded`) backs the generators so
they preview without writing and never silently clobber; `armature fix` refuses to overwrite any
pre-existing output (not just the input) without `--force`, and `unitypackage extract` refuses a
non-empty destination without `--force`. (3) **`--json` is now uniform** across the read/generate
surface — added to `anim-gen clip|blendtree|controller` (the report carries allocated fileIDs + the
wiring note + the YAML, so an agent needn't parse YAML to wire the asset). (4) **`avatar schema
[name|all]`** emits a JSON Schema for each `--json` report type (`describe`/`lint`/`stats`/
`armature`/`fbx-inspect`) so an agent can introspect the output contract and we can catch our own
breaking changes; built on `schemars`, derived via a `schema` cargo feature on `avatar-lint`/
`-stats`/`-armature` (optional for library consumers, on by default in the cli). A new **`avatar
anim-gen controller`** subcommand exposes the crate's `fx_blend_tree` — a complete, Unity-importable
FX `AnimatorController` (class 91) — which the M4 generator previously couldn't emit from the CLI.
The headless Unity-acceptance workflow gained a second gate (`GeneratedAssetAcceptance.cs`): it
imports CLI-generated `.anim`/`.controller` assets in a real editor and asserts they parse into the
expected object types with no import errors — the "last mile" the in-repo round-trip tests can't
cover for M4.

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
