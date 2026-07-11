# Project Plan — VRChat Avatar Tools

A Rust monorepo of tools for VRChat avatars (Unity, SDK3 / Avatars 3.0). This document is the
architecture + roadmap of record. Decisions marked **[decided]** are locked; **[open]** items are
revisited as we learn.

## 1. The core architectural reality

There are two layers, and only one is fully ours to own in Rust.

| Layer | What lives here | Rust ownership |
|---|---|---|
| **3D asset — `.fbx`** | Armature/skeleton, bone names, bind poses, skinning weights, blendshapes | **Full** read (`fbxcel`); write is the open risk (§8). This is where "armature not set up right" lives. |
| **Unity/VRChat project — UnityYAML** (`.anim`, `.controller`, `.asset`, `.prefab`, scene, `.meta`) | Avatar Descriptor, humanoid bone mapping, animator layers, expression menus/params, blend trees, gesture keyframes | **Read/lint** low-risk; **generate** requires correct fileIDs/GUID refs. |
| **Upload/build** | VRCSDK control panel → VRChat servers | **None.** Interactive VRChat-account login; Unity license painful in containers; effectively Windows. Not ours. |

So this repo is **file transformation / analysis / validation / generation** plus an **OSC runtime
daemon** — not a Unity replacement. We generate assets you drop into Unity; you build & upload there.

References: [Unity YAML](https://blog.unity.com/engine-platform/understanding-unitys-serialization-language-yaml),
[VRChat Rig Requirements](https://creators.vrchat.com/avatars/rig-requirements/),
[fbxcel](https://github.com/lo48576/fbxcel),
[GameCI / Unity license](https://game.ci/docs/docker/docker-images/),
[VPM CLI](https://vcc.docs.vrchat.com/vpm/cli/),
[OSC Avatar Parameters](https://docs.vrchat.com/docs/osc-avatar-parameters),
[Animator Parameters](https://creators.vrchat.com/avatars/animator-parameters/).

## 2. Crates

Crate dirs are unprefixed; package names `avatar-<slug>`; lib names `avatar_<slug>`. Binary: `avatar`.

**Foundation**
- `avatar-fbx` — load binary FBX (`fbxcel` tree), extract `Model`/`Geometry` objects + `Connections`
  into a typed scene graph; plus `FbxDocument` for editing + write-back. **[built: read M0/M1, write M3]**
- `avatar-unity-yaml` — read UnityYAML (multi-doc `--- !u!<classID> &<fileID>`, GUID refs in
  `.meta`). Reader only for now; write/round-trip is deferred (see §8). **[built: M2]**

**Models**
- `avatar-armature` — renderer-agnostic skeleton + Unity Humanoid bone enum; infer mapping from bone
  names; validate against VRChat rig requirements; propose repairs. **[building: M1]**
- `avatar-unity-asset` — typed Unity asset graphs over `avatar-unity-yaml`. `AnimatorController`
  (parameters, conditions, blend trees, state machines, write-defaults) is built for M2 lint rules;
  `AnimationClip` / material / scene typing still to come. **[built: AnimatorController (M2); rest M4]**
- `avatar-fbx` also **writes**: `FbxDocument` retains `fbxcel`'s mutable tree, edits objects by id
  (rename / reparent / global-settings / scale), and serializes back to binary FBX. **[built: M3]**
- `avatar-vrc-descriptor` — expression menu/params (256-bit budget, 8-control menu) and the
  **VRC Avatar Descriptor** (view position, viseme lip-sync, expression refs, playable layers)
  extracted structurally; SDK3 rule constants. **[built: M2]**
- `avatar-vpm` — parse `vpm-manifest.json`; discover project root; report editor version +
  installed SDK / Modular Avatar / VRCFury versions. **[built: M2]**

**Logic**
- `avatar-lint` — diagnostics engine; rules over project/params/menus/descriptor/controllers/
  PhysBones → structured report (`serde_json` + pretty print, error/warn/info). Rule codes
  (`VRC001`–`VRC052`) in `docs/reference/sdk3-lint-rules.md`. **[built: M2 + extensions]**
- `avatar-stats` — offline VRChat **performance ranking** (Excellent→Very Poor): geometry metrics
  from an FBX (`analyze_fbx`) and component metrics from a project's avatars (`analyze_project`),
  ranked against the PC + Android limit tables (encoded as data). **[built]**
- `avatar-anim-gen` — generate `.anim` clips + FX-layer blend trees (wrapped in a full FX
  `AnimatorController`) for analog gestures and general expression authoring. Unity-YAML emitter,
  deterministic fileIDs, reader-validated round-trip + a real-editor import gate in CI.
  **[built: M4 — library + `avatar anim-gen clip|blendtree|controller` CLI; full FX animator-layer
  assembly done; CLI-generated assets gated against a real Unity editor]**

**Runtime**
- `avatar-osc` — `rosc`-backed VRChat OSC parameter protocol: `/avatar/parameters/*`, `/input/*` axes
  & buttons (reset-to-zero), `/avatar/change`, split pure-codec + UDP `ParamClient`, plus offline
  OSCQuery avatar-config (`AvatarConfig`) parsing. **[built: M5 — library + `avatar osc` CLI; live
  OSCQuery discovery next]**
- `avatar-osc-gestures` — daemon mapping controller trigger analog → fractional gesture weights →
  OSC. The "Vive advanced controls" feature. Glam-free `AnalogSource` + pure `HandMapping` (deadzone)
  + change-detecting `GestureDaemon`; demo trigger sweep headless, OpenXR input backend on-device.
  **[built: M5 — library + `avatar osc gestures` CLI; OpenXR input adapter next]**
- `avatar-mcp` — a domain-agnostic Model Context Protocol server (stdio JSON-RPC 2.0): pure
  `Server::handle` dispatch core + thin `serve_stdio` loop, no async, mirroring the `avatar-osc`
  codec/transport split. The cli wires the read/diagnose surface in as MCP tools so an agent host can
  discover + call capabilities structurally. **[built — library + `avatar mcp serve` CLI; read-only
  tools, generation tools next]**

**Entry point**
- `avatar-cli` — `avatar` binary, clap v4 subcommands. **[building: M1]**

**Test support**
- `avatar-testkit` — test-only (`publish = false`): the golden-snapshot harness (`golden::assert_json`
  / `redact_roots`), the shared fixture-corpus resolver (`corpus`/`workspace_root`), and in-code
  synthetic-FBX builders (`fbx::humanoid_skeleton`, feature `fbx`). Pulled in as a dev-dependency by
  the crates that golden-test their report surfaces. See `docs/reference/testing.md`. **[built]**

## 3. "Fix the armature" workflow

1. `avatar fbx inspect model.fbx` — node hierarchy, skeleton roots, bone count, scale/orientation,
   units; flags unapplied transforms, wrong axis/units, extra root, unskinned mesh, missing T-pose.
2. `avatar armature check model.fbx` — map detected bones → Unity Humanoid; report missing/mis-mapped
   bones against the required spine chain (Hips→Spine→Chest→Neck→Head + shoulders + hands + feet).
3. `avatar armature fix model.fbx -o fixed.fbx` — opt-in safe repairs (rename to humanoid-friendly
   convention, reparent stray bones, normalize scale/orientation), write FBX *or* emit a headless
   Blender script (see §8). **[M3]**
4. For a Unity project: `avatar lint <projectDir>` adds Avatar-definition + Descriptor + SDK3 checks.

## 4. Analog-gesture / OSC feature

VRChat already exposes the analog signal: `GestureLeftWeight`/`GestureRightWeight` are floats 0→1
from trigger pull; a blend tree in the Fist slot blends on them. The feature splits:
- **Authoring (`avatar-anim-gen`)**: generate FX-layer blend trees + keyframed `.anim` so *any*
  gesture reaches *any fraction* (conceptually what ComboGestureExpressions does, as a generator).
- **Runtime (`avatar-osc-gestures`)**: daemon reads controller analog inputs, sends
  `/avatar/parameters/<Gesture>Weight` — mimicking Vive advanced-controls mapping on any hardware.

## 5. Docker & CI

- Rust → Docker yes: `docker/rust.Dockerfile`, UID/GID-matched like the Legaia Ghidra image. CI =
  GitHub Actions: `fmt --check` + `clippy --all-targets -D warnings` + `test --workspace`.
- Unity → mostly no: GameCI can run Unity headless for *validation*, but **upload** needs
  interactive auth and Unity licensing is painful in containers. Documented as optional; not a
  dependency. Upload stays a manual Unity step.

## 6. Conventions

Locked to the `legend-of-legaia-re` style — see `CLAUDE.md`. Addition vs. Legaia: a
`rust-toolchain.toml` pinning the toolchain (fbxcel + edition 2024 want a recent compiler).

## 7. Decisions

- **[decided]** Crate prefix `avatar-` (libs `avatar_*`, binary `avatar`).
- **[decided]** FBX writes: prototype native (`fbxcel` writer) first in M1/M3; fall back to emitting
  headless Blender Python scripts if Unity/Blender reject the output.
- **[decided]** First build target: M0 scaffold + M1 armature diagnosis on a real FBX.
- **[open]** Whether `avatar-lint` should *emit* Modular Avatar components as an output format.
- **[open]** ASCII-FBX support (currently out — binary only).

## 8. Risks

1. **FBX write-back** (was the biggest). **Resolved in M3.** `fbxcel` 0.9's `writer` feature gives a
   mutable tree + binary serializer (`Writer::write_tree`/`finalize`); `avatar-fbx`'s `FbxDocument`
   loads, edits by object id, and writes back. Proven by round-trip tests, including an
   `AVATAR_SAMPLE_FBX`-gated apply-on-a-real-Mixamo-FBX test. Residual: final acceptance is a manual
   Unity import. Note `write_tree` re-emits arrays uncompressed (larger files, semantically fine).
   The Blender-headless fallback is reserved for mutations that *require* re-transforming geometry
   (scale/orientation normalization), which M3 deliberately flags rather than applies.
2. **UnityYAML round-trip + `.meta` GUIDs.** Read/lint low-risk; *generating whole assets* needs
   correct fileIDs/GUIDs (done in M4 for `.anim`/`.controller`). **Surgical edits are now safe too**:
   `avatar_unity_yaml::EditableUnityFile` (and `avatar asset set`) edit an *existing* asset by
   span-splicing the raw text — it locates the target value's byte range and replaces only that,
   leaving every `&fileID`, `{fileID, guid}` reference, key order, and byte of formatting untouched
   by construction, then re-parses to validate. This sidesteps the original risk (a parse→re-emit
   that reorders keys / drops anchors breaks references): unchanged bytes are never rewritten.
   Scope is value edits (scalars, reference re-targets, flow-map subfields); *structural* edits
   (adding/removing keys or sequence elements) still prefer the generators. See
   [`docs/reference/unity-yaml-edit.md`](docs/reference/unity-yaml-edit.md).
3. **VRChat SDK churn.** Encode rules as data/config, not hardcoded constants.
4. **Ecosystem overlap.** Modular Avatar / VRCFury / NDMF already cover non-destructive Unity-side
   building. Complement them (own the FBX/armature + headless-validation layer; consider emitting MA
   components) rather than duplicate.
5. **Malformed / hostile inputs.** Hardened: `.unitypackage` extraction caps decompressed bytes per
   entry (512 MiB) and in total (2 GiB) — a decompression-bomb guard; `avatar-gltf` rejects
   primitives with > `u32::MAX` vertices; `avatar-fbx` reads `UpAxis`/`FrontAxis` via checked
   `i32::try_from`; and `avatar-unity-yaml`'s `parse_lossy` is now infallible by construction (no
   latent panic path).

## 9. Milestones

- **M0 — Scaffold.** Workspace, CI, hooks, docs, `avatar-fbx` reading a real FBX node tree. *(this pass)*
- **M1 — Armature diagnosis.** `avatar fbx inspect` / `armature check`; resolve FBX-write question. *(this pass)*
- **M2 — Project linting.** ✅ `avatar-unity-yaml`, `avatar-unity-asset`, `avatar-vpm`,
  `avatar-vrc-descriptor`, `avatar-lint` → `avatar lint <project>` SDK3-compliance report. Covers
  params budget, menus, dangling refs, project/SDK info, the VRC Avatar Descriptor parsed from
  prefabs/scenes (expression-parameters/menu reference resolution via a guid→path `.meta` index,
  playable-layer animator-controller existence, viseme lip-sync, eye-look config, and an
  expression-parameter↔animator wiring cross-check by name+type: `VRC002`/`030`–`037`), **and**
  animator-controller contents (parameter references, default states, Write Defaults consistency,
  duplicate params: `VRC040`–`044`). Since extended with seven more rules: `VRC012` (param used by no
  menu/animator), `VRC022` (empty menu control), `VRC038` (duplicate/empty viseme blendshape),
  `VRC045` (Write Defaults inconsistent across the avatar's playable-layer controllers), and a
  PhysBones/Avatar-Dynamics group `VRC050`–`052` (unresolvable root, moves zero transforms, collider
  slots but none wired). M2 gaps closed. Since extended again: **animation-clip rules** (`VRC046`–`VRC049`: missing/absent
  state motions, FX-layer transform/muscle curves, empty clips), the **viseme↔source-FBX morph-channel
  cross-check** (`VRC039`, the first cross-layer rule), and a **hygiene/Android group** (`VRC060`
  missing scripts, `VRC061` non-mobile shaders). Next: contacts.
- **M3 — Armature repair.** ✅ `avatar armature fix <model.fbx>` plans repairs and (with `-o`) writes
  a corrected binary FBX via `avatar-fbx`'s `FbxDocument`. The one native repair is canonical
  humanoid bone **renames** (id-safe; what makes Unity auto-map). Mis-wired parent **topology** and
  scale/orientation normalization are **flagged, not applied**: each needs a geometry transform
  (→ Blender territory), not a metadata relabel — re-pointing a bone's `OO` connection without
  recomposing its local transform would move its rest/bind pose. Dry-run by default. The
  native-FBX-write risk (§8) is resolved. Rules/behaviour: `docs/reference/armature-repair.md`.
  The headless-Blender fallback is now *emitted*: `--blender-script` renders the whole plan
  (renames + rest-pose-preserving reparents + transform baking) as a Blender Python script
  (`avatar_armature::blender_script`); running it under CI is the remaining step.
- **M4 — Asset generation.** 🟡 *Library + CLI landed; expression assets + the toggle composite since.* `avatar-anim-gen` generates Unity-YAML
  `.anim` clips (`AnimationClip`: blendshape-weight + GameObject-active curves) and FX-layer
  analog-gesture blend trees (`BlendTree`), with a faithful YAML emitter, deterministic FNV-seeded
  fileIDs, and a reader-validated round-trip — driven by `avatar anim-gen blendtree` / `… clip` /
  `… controller`. A `controller.rs` module also emits a **full FX `AnimatorController`** (class 91)
  wrapping a blend-tree layer (`m_AnimatorParameters` + `m_AnimatorLayers` + state machine), now
  exposed by the `controller` subcommand. The reader round-trip is in-repo, **and** the
  Unity-acceptance workflow now imports CLI-generated `.anim`/`.controller` assets into a real editor
  (`GeneratedAssetAcceptance.cs`) and asserts they parse into the expected object types with no
  import errors — closing the "live Unity import" gap for M4 (gated on a `UNITY_LICENSE` secret).
  Since extended with **expression-asset generation** (`VRCExpressionParameters` /
  `VRCExpressionsMenu`, `avatar anim-gen params|menu`) and the composite **`avatar toggle`** bundle
  (On/Off clips + two-state FX controller + params + menu + guid-pinning `.meta` sidecars — the
  end-to-end authoring loop), all also exposed as non-writing MCP tools (`avatar_gen_*`). The typed
  `AnimationClip` reader landed in `avatar-unity-asset` (feeding lint's clip rules `VRC046`–`VRC049`).
  Remaining: typed material/scene in `avatar-unity-asset`. Behaviour: `docs/reference/anim-gen.md`.
- **M5 — OSC runtime.** 🟡 *Library + CLI landed.* `avatar-osc` implements VRChat's OSC parameter
  protocol (`/avatar/parameters`, `/input`, `/avatar/change`) as a pure codec + non-blocking UDP
  `ParamClient`, plus offline OSCQuery avatar-config parsing — driven by `avatar osc
  send|input|monitor|change|query`. The **analog-gesture daemon** `avatar-osc-gestures` (the "Vive
  advanced controls on any hardware" feature) maps a controller trigger → gesture + weight with a
  glam-free `AnalogSource`, a pure deadzoned `HandMapping`, and change-detected sends — driven by
  `avatar osc gestures` (a synthetic demo sweep headless). Remaining: live OSCQuery (HTTP/mDNS)
  discovery and the daemon's on-device OpenXR input backend. Behaviour:
  `docs/reference/osc-runtime.md`.

### Performance stats (built) — `avatar stats`

A cost-side complement to `lint`'s correctness-side checks: `avatar-stats` reproduces VRChat's
performance ranking (Excellent→Very Poor) offline. `avatar stats <model.fbx>` ranks the geometry
metrics (triangles, skinned/basic meshes, material slots, bones); `avatar stats <project>` ranks the
component metrics per avatar (PhysBone components/colliders, contacts, particle systems, lights,
renderers, …), recognizing VRChat dynamics `MonoBehaviour`s structurally and everything else by Unity
class id. Overall rank = the worst measured metric; metrics a given source can't measure (total
particles, constraints) are surfaced in `not_evaluated` rather than silently assumed clean. The
project path resolves renderers' `m_Mesh` guids to their source FBX for **triangles** (distinct files
counted once; unresolved meshes flagged), reads **bones** from `m_Bones`, estimates **texture memory
for both PC and Android** through the renderer → material → texture chain (per-texture VRAM from image
dimensions + per-platform import format — DXT/BC vs ASTC/ETC2; an estimate, since the imported GPU
format isn't knowable offline), and computes **PhysBone affected-transform & collision-check counts**
by walking the transform hierarchy under each PhysBone's `rootTransform` (descendants minus
`ignoreTransforms`, plus an endpoint per chain tip when `endpointPosition` is set, × assigned
colliders — an estimate of VRChat's Avatar-Dynamics cost; PhysBones with an unresolvable root are
flagged), so a project gets a unified geometry+component rank on both platforms. Limit tables (PC +
Android) are encoded as data (decision §7, risk 3). **Total particle count** (per `ParticleSystem`:
`min(maxParticles, ceil(rate × lifetime))`, summed; unparseable systems flagged) and **constraint
count + depth** (Unity constraint class ids 320–325 + structural VRChat constraints; depth = longest
constraint→source chain via a cycle-safe walk) are now measured rather than deferred. **Mesh-particle
polygon cost** (mesh-mode renderer's mesh triangles × particle count, resolved through the
renderer→mesh→FBX chain — approximate) and **particle trail/collision flags** (systems with
`TrailModule`/`CollisionModule` enabled) have now landed too, emptying the `not_evaluated` list by
default. Behaviour: `docs/reference/performance-stats.md`.

### Runtime rig layer (built, green) — the "drive a rig at runtime" band

A parallel band requested by the Legaia VR spectator PRD (§9): load and **render/pose** a rig,
renderer-agnostic (the consuming wgpu viewport owns the draw). Independent of the VRChat path; it
generalizes the layer *beneath* VRChat (rig + pose + input). Crates:

- **`avatar-mesh`** — POD `RawMesh`/`SkinData` interchange (no `glam`, no format dep).
- **`avatar-fbx::meshes()` + `avatar-gltf`** — FBX and glTF → `RawMesh` + skin/bind matrices.
- **`avatar-pose`** — `PosedSkeleton` → world matrices, GPU bone-matrix palette, CPU skinning, and
  `pose::ik` analytic two-bone IK. The only `glam` (f32) in the runtime tier.
- **`avatar-input`** — backend-agnostic `TrackerState`/`TrackerSource` (HMD + controllers +
  trackers); `MockSource` + an `osc`-feature backend; OpenXR is the planned on-device backend.

Bind transforms come straight from the file (`TransformLink` / inverse-bind), never recomposed from
`Lcl` + `PreRotation`; correctness is pinned by a renderer-free **rest-pose reproduction** test on
both importers. Behaviour: `docs/reference/rig-runtime.md`. Next: GPU skinning demo in the viewport
(consumer side), OpenXR backend, optional IK refinements (twist, foot-lock).
