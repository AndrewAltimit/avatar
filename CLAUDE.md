# CLAUDE.md — repo-internal guidance

This file is the **map of the repo's documentation**: a brief overview, a table of contents pointing
at every other markdown file, the working conventions, and the build commands. Keep it slim — when a
subsystem grows, the detail goes in its `docs/reference/*.md` and per-crate README, and this file
just points at them. User-facing docs live in `README.md`, `PLAN.md`, and `docs/`.

**TL;DR:** a Rust monorepo (edition 2024, workspace, `anyhow` + `clap`) of tools that operate on the
*files* a VRChat SDK3 avatar is made of — FBX and Unity YAML — to diagnose, fix, lint, generate, and
preview them, plus an OSC runtime layer. Two layers (3D `.fbx` and Unity/VRChat YAML) are ours; the
VRChat upload step is not. For what's built and the roadmap, see [Status](#status) and
[`PLAN.md`](PLAN.md).

## Documentation map

| Doc | What it covers |
|-----|----------------|
| [`README.md`](README.md) | User-facing overview, crate table, quick-start commands. |
| [`PLAN.md`](PLAN.md) | Architecture + roadmap of record: layers, crates, milestones (M0–M5), decisions, risks. |
| `CLAUDE.md` *(this file)* | Repo-internal guidance: doc map, conventions, commands, status, gotchas. |
| [`docs/README.md`](docs/README.md) | Index of the `docs/` directory. |
| [`docs/overview.md`](docs/overview.md) | The layered architecture in brief, with external references. |
| [`docs/reference/humanoid-bones.md`](docs/reference/humanoid-bones.md) | Unity humanoid bones + VRChat rig requirements. |
| [`docs/reference/sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md) | Every `avatar lint` rule (`VRC001`–`VRC061`) + encodings. |
| [`docs/reference/unity-asset.md`](docs/reference/unity-asset.md) | `avatar-unity-asset`: the typed AnimatorController (`.controller`) + AnimationClip (`.anim`) readers the controller/clip lint rules consume. |
| [`docs/reference/armature-repair.md`](docs/reference/armature-repair.md) | What `avatar armature fix` repairs, and the FBX writer. |
| [`docs/reference/rig-runtime.md`](docs/reference/rig-runtime.md) | Runtime rig layer: skin/bind extraction, posing + bone-matrix palette, two-bone IK, tracker input. |
| [`docs/reference/performance-stats.md`](docs/reference/performance-stats.md) | `avatar stats`: performance-rank metrics (incl. particles & constraints), component recognition, PC/Android threshold tables. |
| [`docs/reference/anim-gen.md`](docs/reference/anim-gen.md) | `avatar-anim-gen`: `.anim` clip + analog-gesture blend-tree + FX `AnimatorController` generation (Unity-YAML emitter, deterministic fileIDs). |
| [`docs/reference/unity-yaml-edit.md`](docs/reference/unity-yaml-edit.md) | `EditableUnityFile` / `avatar asset set`: surgical, round-trip-safe value **and structural** edits to an *existing* Unity asset by span-splicing raw text (fileIDs/refs/key-order/formatting preserved). |
| [`docs/reference/migrate.md`](docs/reference/migrate.md) | `avatar-migrate` / `avatar migrate sdk3`: SDK2 → SDK3 migration — descriptor/PipelineManager retyped in place, DynamicBone → PhysBone by the SDK's own rules, Cloth → PhysBone skirt, subtree stripping, gesture overrides → FX layer, rig-derived eye look, output project layout, script references, limits. |
| [`docs/reference/physbone.md`](docs/reference/physbone.md) | `avatar physbone list|set|split|stretch|flare|nudge`: inspect + retune a prefab's `VRCPhysBone`s in place (values, per-chain curves, split chains onto own components, stretch chain offsets for a longer skirt/tail, re-angle chains toward vertical) + the `avatar render --pose <prefab>` / `--stretch` previews. |
| [`docs/reference/osc-runtime.md`](docs/reference/osc-runtime.md) | `avatar-osc`: VRChat OSC address space, codec, UDP client, OSCQuery avatar-config parsing; the analog-gesture daemon. |
| [`docs/reference/unitypackage.md`](docs/reference/unitypackage.md) | `avatar-unitypackage`: reading the `.unitypackage` format, extracting to a Unity project tree, the avatar-in-world co-import testbed. |
| [`docs/reference/render.md`](docs/reference/render.md) | `avatar-render` / `avatar render` + `avatar view`: offscreen wgpu preview pipeline, avatar rest-pose render (auto-upright), world-scene render, avatar-dropped-at-spawn-in-world, interactive winit viewer (orbit/zoom/walk) + limits. |
| [`docs/reference/mcp.md`](docs/reference/mcp.md) | `avatar-mcp` / `avatar mcp serve`: the stdio MCP server exposing the read/diagnose tools to an agent host. |
| [`docs/reference/testing.md`](docs/reference/testing.md) | The fixture corpus (`fixtures/`) + the `avatar-testkit` golden-snapshot harness: corpus layers, `golden::assert_json`/`redact_roots`, the `UPDATE_GOLDEN` workflow. |
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
[`migrate`](crates/migrate/README.md) ·
[`render`](crates/render/README.md) ·
[`osc`](crates/osc/README.md) ·
[`osc-gestures`](crates/osc-gestures/README.md) ·
[`mcp`](crates/mcp/README.md) ·
[`cli`](crates/cli/README.md) ·
[`testkit`](crates/testkit/README.md).

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
   Read/lint is low-risk; *generating* whole assets Unity accepts requires correct fileIDs/GUIDs
   (M4). *Editing* an existing asset is round-trip-safe via `EditableUnityFile` (span-splice; see
   [`docs/reference/unity-yaml-edit.md`](docs/reference/unity-yaml-edit.md)).

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
- **`glam` confinement (an invariant, not a preference):** `glam` (f32) lives only in the runtime-rig
  + render crates (`pose`/`input`/`gltf`/`render`) and the cli's render modules
  (`render_scene`/`world`/`texture`). Keep it out of the diagnose/generate/OSC graph
  (`lint`/`stats`/`vrc-descriptor`/`unity-*`/`anim-gen`/`osc`/`osc-gestures`/`mcp`) — `osc-gestures`
  is deliberately glam-free so the cli's OSC path stays clean (`cargo tree` verifies).
- **Tests:** unit tests in-crate; integration tests in `crates/<name>/tests/`, **gated by an env var**
  (`AVATAR_SAMPLE_FBX`, `AVATAR_SAMPLE_UNITYPACKAGE`, `AVATAR_SAMPLE_UNITYPACKAGE_WORLD`) that
  self-skips when unset so CI without fixtures stays green. The committed corpus is in top-level
  `fixtures/` (synthetic Unity projects; FBX is synthesized in-code); report surfaces are pinned by
  **golden snapshots** via the `avatar-testkit` dev crate (regenerate with `UPDATE_GOLDEN=1 cargo
  test`). Never commit user FBX/Unity projects (see `.gitignore`). Full detail:
  [`docs/reference/testing.md`](docs/reference/testing.md), [`CONTRIBUTING.md`](CONTRIBUTING.md).

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

Built and green through **M5**, plus the runtime-rig and render layers and the agent-facing surface.
The roadmap of record (what each milestone covers, decisions, risks) is [`PLAN.md`](PLAN.md);
per-subsystem behaviour is in the [documentation map](#documentation-map) above and the per-crate
READMEs. What exists today, with its doc:

- **Diagnose / lint** — `avatar lint <project>` (SDK3 rules `VRC001`–`VRC061`, incl. animation-clip
  contents, the viseme↔source-FBX cross-check, and Android/hygiene checks;
  [`sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md)); `avatar stats` offline VRChat
  performance ranking ([`performance-stats.md`](docs/reference/performance-stats.md)); `avatar
  describe` one-shot consolidated snapshot.
- **FBX** — read (incl. blendshape channels) + native binary write-back (`avatar_fbx::FbxDocument`);
  `avatar armature check|fix` — canonical humanoid renames applied natively; `avatar fbx reslot` —
  per-polygon material-slot reassignment by region/brightness (a glowing strand → the black slot),
  or its `--uv-mask` footprint for a texture-side fix — **the writer's output is not yet proven to
  import in Unity for a full skinned avatar** (see armature-repair.md); `avatar fbx blendshapes` —
  per-channel material slots (which material an emote overlay shape renders with);
  topology/scale/orientation flagged **and** emitted as a headless-Blender repair script
  (`--blender-script`, [`armature-repair.md`](docs/reference/armature-repair.md)).
- **Generate (M4)** — `avatar anim-gen clip|blendtree|controller|params|menu`: Unity-YAML `.anim`,
  a full FX `AnimatorController`, and VRC expression parameters/menu assets, deterministic fileIDs;
  plus the composites **`avatar toggle`** — the full ten-file toggle bundle (clips + two-state FX
  controller + params + menu + guid-pinning `.meta`s) — and **`avatar anim-gen puppet`** — graft a
  radial-puppet dial (gated blend-tree layer + float param + menu control) into an existing
  controller/params/menu by span-splice ([`anim-gen.md`](docs/reference/anim-gen.md)).
- **Edit** — `avatar asset set` / `avatar_unity_yaml::EditableUnityFile`: surgical, round-trip-safe
  value edits to an *existing* Unity asset (scalars, reference re-targets, flow-map subfields) **and
  structural edits** (remove/replace/append documents, block-sequence items) by span-splicing raw
  text — fileIDs/refs/key-order/formatting preserved
  ([`unity-yaml-edit.md`](docs/reference/unity-yaml-edit.md)).
- **Migrate (SDK2 → SDK3)** — `avatar migrate sdk3 <extracted-project> -o <out> --name N …`: rewrites
  the SDK2 avatar prefab in place (descriptor + PipelineManager retyped at their fileIDs, root motion
  off, DynamicBone → PhysBone with the SDK's own conversion rules, optional Cloth → PhysBone skirt,
  `--strip` subtrees, gesture overrides → an either-hand FX layer with analog trigger-depth blend trees
  (`GestureLeftWeight`/`GestureRightWeight`; `--no-analog-gestures` for discrete), rig-derived eye
  look + blink) and
  assembles a VCC-openable project around it (`--vpm-package` bundles e.g. a shader package,
  `--relink-locked-shaders` re-points locked materials at their original shader); `--dry-run` / `--json`
  ([`migrate.md`](docs/reference/migrate.md)). Post-migration **PhysBone tuning** — `avatar physbone
  list|set|split|stretch|flare|nudge` (typed `PhysBoneSpec` read-back + re-render, per-chain curves, split
  chains onto own components, chain stretch for a longer skirt, chain re-angling so a skirt hugs the
  legs; `avatar render --pose <prefab>` draws the FBX in a prefab's pose to preview it)
  ([`physbone.md`](docs/reference/physbone.md)).
- **OSC runtime (M5)** — `avatar osc send|input|monitor|capture|replay|change|query` + the
  analog-gesture daemon `avatar osc gestures`; `capture` reduces VRChat's parameter stream to a
  gesture/weight cross-tab (also a standalone Windows-cross-compilable `avatar-gesture-capture`
  bin, OSCQuery-advertised); `replay` simulates a `.controller` against a captured log — the
  state timeline the FX actually went through ([`osc-runtime.md`](docs/reference/osc-runtime.md)).
- **Packaging / preview** — `avatar unitypackage info|list|extract|testbed`
  ([`unitypackage.md`](docs/reference/unitypackage.md)); `avatar render` / `avatar view` wgpu preview
  ([`render.md`](docs/reference/render.md)).
- **Agent surface** — `--json` across the read/generate commands, `avatar schema`, and `avatar mcp
  serve` (non-writing MCP server incl. `avatar_physbone_list` and text-returning `avatar_gen_*` generation tools,
  [`mcp.md`](docs/reference/mcp.md)). Disk writes/repairs stay on the CLI behind a dry-run-safe
  `WriteGuard`.
- **Runtime rig** — `mesh`/`pose`/`input` + `gltf`, the renderer-agnostic VR-spectator foundation
  ([`rig-runtime.md`](docs/reference/rig-runtime.md)).

In flight: the OpenXR on-device input backend for the gesture daemon; OSCQuery **advertisement**
(mDNS responder + HOST_INFO/tree HTTP) landed in `avatar_osc::oscquery` for the capture tools —
full *browse/resolve* discovery is still open; running the generated Blender repair script under
CI. See [`PLAN.md`](PLAN.md).

## Gotchas

- **Unity GUIDs in test fixtures must contain letters.** A GUID is 32 hex chars; an all-digit
  "guid" is parsed by yaml-rust2 as a *number*, so `as_str()` returns `None` and guid resolution
  silently breaks. Use a guid with letters (e.g. `aaaa…`).
- **FBX bind never recomposed from `Lcl`+`PreRotation`** — it comes from `TransformLink`/inverse-bind
  (see [`rig-runtime.md`](docs/reference/rig-runtime.md)); the avatar renderer deliberately uses raw
  control points, not the per-cluster bind matrices, which ripped/MMD→FBX avatars ship broken
  ([`render.md`](docs/reference/render.md)).
- **`fbxcel`'s `write_tree` re-emits arrays uncompressed** — a written FBX is larger than the input
  and re-loads identically here, but a rewritten full skinned avatar imported *invisible* in Unity
  2022.3 (open problem; [`armature-repair.md`](docs/reference/armature-repair.md)) — don't ship a
  written FBX to Unity without checking it there.
- **SDK3 script references are DLL class hashes, not `11500000`.** Every SDK3 runtime class lives in
  a DLL, so `m_Script` is `{fileID: <MD4 class hash>, guid: <dll guid>, type: 3}` —
  `avatar_unity_yaml::script_file_id(namespace, class)` derives the hash (test-pinned against the
  SDK's own assets); the pinned refs are in `avatar_migrate::sdk3` / `avatar_anim_gen::expressions`.
  A `.cs`-style `{fileID: 11500000, guid: …}` to a guessed GUID resolves to nothing in Unity.
