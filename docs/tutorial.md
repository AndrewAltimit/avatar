# Tutorial: I have an avatar FBX and a Unity project — what can this do?

This is an end-to-end walkthrough of the `avatar` CLI. The premise: you have a character `.fbx`
(maybe a raw Mixamo or VRoid export) and a Unity/VRChat SDK3 project, and you want to find and fix
the common problems before you import and upload. The tools operate on the *files* — they never
replace Unity or the VRChat SDK upload step (see [`../README.md`](../README.md) for the scope).

Build the binary once:

```sh
cargo build --workspace
```

Then run subcommands with `cargo run -p avatar-cli -- <subcommand>`, or run the built `avatar`
binary directly from `target/`. Every read and generate command takes `--json` to emit a
machine-readable report instead of the human-readable text shown here, and `avatar schema [name]`
prints the JSON Schema of those reports so the output is a stable contract (handy when driving the
CLI from a script or agent); `avatar lint` exits non-zero when it finds errors, so it can gate CI.

> FBX support is **binary only** (FBX 7.x — the Autodesk/Unity/Blender default). ASCII FBX is not
> supported; re-export as binary.

## 0. The one-shot overview — `avatar describe`

If you just want the whole picture of an asset in one call, start with `describe`. For an FBX it
runs the inspect + armature + geometry-rank steps below in one shot; for a Unity project it runs lint
+ a per-avatar performance rank:

```sh
cargo run -p avatar-cli -- describe model.fbx              # human-readable summary
cargo run -p avatar-cli -- describe model.fbx --json       # one consolidated machine-readable report
cargo run -p avatar-cli -- describe path/to/UnityProject   # lint + per-avatar rank for a project
```

It exits non-zero on a non-humanoid-ready rig or a project with lint errors, so it gates CI too. The
sections below cover the same checks individually when you want the detail.

## 1. Inspect the FBX — `avatar fbx inspect`

Start by looking at the model's structure, units, and orientation:

```sh
cargo run -p avatar-cli -- fbx inspect model.fbx
```

It prints the FBX format version, the unit scale and up-axis (flagged with `[!]` when they don't
match Unity's expectation — Unity imports centimetres, i.e. a unit scale of `100.0`, and Y-up), the
object counts (models / geometries / materials / bone-like nodes), and the model hierarchy as a
tree. This is the first thing to check when an imported avatar comes in at the wrong size or rotated
ninety degrees — those are almost always a unit-scale or up-axis mismatch you can see here.

## 2. Check the armature — `avatar armature check`

Validate the skeleton against VRChat's humanoid rig requirements:

```sh
cargo run -p avatar-cli -- armature check model.fbx
```

It infers which bones map to Unity's humanoid bones (hierarchy-aware, not just by name), then
reports:

- **Mapped humanoid bones** — each slot and the source bone(s) feeding it.
- **Missing REQUIRED bones** (`[X]`) — the avatar will not import as humanoid until these exist. This
  is what fails the command (non-zero exit).
- **Missing recommended bones** (`[!]`) — VRChat expects a full spine + shoulders; surfaced but not
  fatal.
- **Duplicate mappings**, **unmapped bone-like nodes**, and a count of ignored finger / leaf `_End`
  bones.
- Multiple armature roots are flagged (`[!]`) — VRChat expects a single root.

The bone table it maps and validates against is documented in
[`reference/humanoid-bones.md`](reference/humanoid-bones.md). The command exits non-zero only when a
Unity-*required* bone is missing, so you can gate an import pipeline on it.

## 3. Fix the armature — `avatar armature fix`

If `check` reports a rig that isn't humanoid-ready (the classic case: a raw Mixamo export with
`mixamorig:` bone prefixes), plan and apply the safe repairs:

```sh
# Dry run — print the repair plan, write nothing (the default):
cargo run -p avatar-cli -- armature fix model.fbx

# Write a repaired FBX:
cargo run -p avatar-cli -- armature fix model.fbx -o fixed.fbx
```

Flags (from `crates/cli/src/main.rs`): `-o, --output <FILE>` writes the repaired FBX (omit it for a
dry run); `--force` allows `--output` to overwrite the input file (refused by default); `--json`
emits the plan as JSON.

The one **native repair that is applied** is canonical humanoid bone **renames**
(`mixamorig:LeftArm` → `LeftUpperArm`) — that is what Unity's humanoid auto-mapper keys on, and it is
id-safe (skin/anim references are by FBX object id, not name). Mis-wired parent **topology** and
**scale/orientation** problems are **flagged, not applied**: each needs a geometry transform (Blender
territory), not a metadata relabel — re-pointing a bone's connection without recomposing its local
transform would move its rest/bind pose. The full rationale and the FBX writer's characteristics are
in [`reference/armature-repair.md`](reference/armature-repair.md).

## 4. Lint the Unity project — `avatar lint`

Once the FBX is set up and you have a Unity/VRChat SDK3 project, check it for SDK3 compliance:

```sh
cargo run -p avatar-cli -- lint path/to/UnityProject

# Also fail on warnings (useful for gating CI):
cargo run -p avatar-cli -- lint path/to/UnityProject --deny-warnings
```

You can point it at the project root or any path inside it — it discovers the project upward. It
reports the Unity version and avatar SDK version, asset counts (Expression Parameters, menus,
descriptors, animator controllers, packages), then the diagnostics: each is tagged `[X]` error,
`[!]` warn, or `[i]` info, with the `VRCNNN` rule code, the offending file, and a fix hint. The
command exits non-zero when there are errors (or, with `--deny-warnings`, any warnings).

What it covers: the Expression Parameters sync budget and duplicates, menu size and dangling
parameter references, the VRC Avatar Descriptor parsed from prefabs/scenes (expression / playable-
layer references resolved via a guid→path `.meta` index, viseme lip-sync, eye-look), animator-
controller contents (parameter references, default states, Write Defaults consistency), and
project/VPM info. Every rule code (`VRC001`–`VRC062`) and its encoding is in
[`reference/sdk3-lint-rules.md`](reference/sdk3-lint-rules.md).

## 5. Estimate performance — `avatar stats`

Reproduce VRChat's performance ranking (Excellent → Very Poor) offline, for **both PC and Android**.
The argument can be an FBX (geometry side) or a Unity project / any path inside one (component side):

```sh
cargo run -p avatar-cli -- stats model.fbx          # geometry: triangles, meshes, material slots, bones
cargo run -p avatar-cli -- stats path/to/UnityProject  # per-avatar components: PhysBones, contacts, ...
cargo run -p avatar-cli -- stats model.fbx --json   # machine-readable report
```

It prints a per-metric table with the PC and Android rank for each metric and an **Overall** rank
(the worst measured metric). Texture Memory is shown as `PC/Android` when the two differ — textures
recompress per platform. Metrics a given source can't measure (e.g. an FBX can't see texture memory
or PhysBones) are listed under "Not evaluated for this source" rather than silently assumed clean, so
you know the real rank could still be lower. Which metrics each source measures, how components are
recognized, and the PC/Android threshold tables are in
[`reference/performance-stats.md`](reference/performance-stats.md).

## Putting it together

A typical first pass on a fresh avatar:

```sh
avatar fbx inspect model.fbx           # units / orientation / structure sane?
avatar armature check model.fbx        # humanoid-ready?
avatar armature fix model.fbx -o fixed.fbx   # if not, canonicalize bone names
avatar stats fixed.fbx                 # geometry within performance budget?
# ... import fixed.fbx into Unity, build the avatar prefab ...
avatar lint  path/to/UnityProject      # SDK3 compliance
avatar stats path/to/UnityProject      # full component-side performance rank
# ... build & upload in Unity via the VRChat SDK (not part of these tools) ...
```

## Generate animation assets — `avatar anim-gen`

`avatar-anim-gen` emits Unity assets as text (Unity YAML, in the exact shape Unity's own serializer
writes), with **deterministic** fileIDs so output is diffable and reproducible. Six subcommands —
`clip`, `blendtree`, `controller`, `params`, `menu`, `puppet`:

```sh
# A static expression clip: hold a blendshape (and/or toggle a GameObject on).
avatar anim-gen clip --name Smile --blendshape Body:Smile:100 -o Smile.anim
avatar anim-gen clip --name HatOn --toggle Armature/Head/Hat -o HatOn.anim

# A 1D analog-gesture blend tree: VRChat blends across your clips by trigger pull.
# By default emits a self-contained AnimatorStateMachine + State + BlendTree fragment;
# --tree-only emits just the BlendTree document to graft onto an existing Fist state.
avatar anim-gen blendtree --name FistBlend --parameter GestureLeftWeight \
    --clip <relaxed-hand-guid>@0.0 --clip <fist-guid>@1.0 -o FistBlend.asset

# A complete, Unity-importable FX AnimatorController wrapping that blend tree in one layer.
avatar anim-gen controller --name FX --layer "Base Layer" \
    --clip <relaxed-hand-guid>@0.0 --clip <fist-guid>@1.0 -o FX.controller

# The VRChat expression assets themselves:
avatar anim-gen params --param Hat:bool --param Dim:float:0.5:local -o Params.asset
avatar anim-gen menu --toggle Hat:Hat --radial Dim:Dim -o Menu.asset

# Graft a radial-puppet dial into an EXISTING avatar's controller/params/menu (span-splice):
avatar anim-gen puppet --controller FX.controller --param Blink \
    --clip <neutral-guid>@0.0 --clip <closed-guid>@1.0 …
```

Output goes to stdout (pipe/redirect) or a file with `-o`. A wiring note is printed to stderr
explaining how to splice a fragment into your FX `.controller` (the `controller` subcommand emits the
whole thing, so it needs none). These generators are **write-safe**: `--dry-run` previews without
touching disk, and an existing output file is never overwritten without `--force`. Add `--json` to
get a structured report (allocated fileIDs, the wiring note, the YAML) instead of raw YAML. See
[`reference/anim-gen.md`](reference/anim-gen.md).

The most common composite has its own top-level command — a complete **toggle** in one call:

```sh
avatar toggle --name Hat --toggle Armature/Head/Hat -o HatBundle/
```

emits the full ten-file bundle (On/Off clips, a two-state FX controller, expression params + menu,
and guid-pinning `.meta`s) ready to drop into `Assets/`.

## Edit an existing asset — `avatar asset set`

To change one value inside an asset you already have — without Unity rewriting the whole file —
`asset set` span-splices the raw text, so fileIDs, references, key order, and formatting all
survive:

```sh
avatar asset set Parameters.asset --path m_Name --value Params2
avatar asset set Avatar.prefab --doc 123456 --path m_LocalPosition/y --value 0.1 -o Avatar.prefab --force
```

See [`reference/unity-yaml-edit.md`](reference/unity-yaml-edit.md) for the path syntax and the
structural-edit surface underneath it.

## Migrate an SDK2 avatar — `avatar migrate sdk3`

If what you have is an old SDK2-era avatar `.unitypackage`, the whole modernization is one pass:

```sh
avatar unitypackage extract avatar2018.unitypackage -o proj    # unpack the old package
avatar migrate sdk3 proj -o out/ --name MyAvatar --drop-cloth --eyes Eye_L,Eye_R
```

The migration retypes the descriptor + PipelineManager **in place** (same fileIDs), converts
DynamicBone → PhysBone using the SDK's own conversion rules, turns gesture overrides into an
either-hand FX layer with analog trigger-depth blend trees, derives eye look + blink from the rig,
and assembles a VCC-openable project around the rewritten prefab. `--dry-run` / `--json` preview
everything. See [`reference/migrate.md`](reference/migrate.md).

Then tune the converted physics — inspect and rewrite `VRCPhysBone` components inside the prefab:

```sh
avatar physbone list out/Assets/MyAvatar.prefab                # every PhysBone: chains + tuning
avatar physbone set out/Assets/MyAvatar.prefab Hair --pull 0.3 --spring-curve "0:1,1:0.5" \
    -o out/Assets/MyAvatar.prefab --force
avatar physbone stretch out/Assets/MyAvatar.prefab SkirtRoot --factor 1.5 -o … --force
```

`split`, `flare`, and `nudge` cover the rest (per-strand tuning, skirt re-angling, hinge rings);
see [`reference/physbone.md`](reference/physbone.md).

## Preview without Unity — `avatar render` / `avatar view`

```sh
avatar render --avatar model.fbx -o preview.png                       # rest pose, auto-upright
avatar render --avatar model.fbx --world world/Assets/Scene.unity -o in-world.png  # at the spawn
avatar render --avatar model.fbx --pose out/Assets/MyAvatar.prefab -o posed.png    # in a prefab's pose
avatar view   --avatar model.fbx --world world/Assets/Scene.unity     # interactive orbit/zoom/walk
```

Headless GPU rendering via wgpu — no Unity, no display needed for `render`. `--pose` / `--stretch`
make it the preview loop for the PhysBone commands above. See [`reference/render.md`](reference/render.md).

## Drive and inspect a running avatar — `avatar osc`

`avatar-osc` speaks VRChat's OSC parameter protocol. The send/monitor commands need a running VRChat
with OSC enabled; `osc query` is fully offline.

```sh
avatar osc send VRCEmote 3                 # set /avatar/parameters/VRCEmote (auto bool/int/float)
avatar osc input Vertical 0.5              # /input axis, -1..1
avatar osc input Jump true                 # /input button
avatar osc monitor --seconds 5            # print the parameters VRChat broadcasts
avatar osc capture --seconds 30 -o params.log  # record the stream → gesture/weight cross-tab
avatar osc replay params.log FX.controller     # simulate the FX against the capture, offline
avatar osc query path/to/avtr_….json      # list an avatar's parameters from its OSCQuery config
avatar osc gestures --seconds 10          # analog-gesture daemon (demo trigger sweep)
```

`capture` + `replay` are the two halves of proof when debugging gestures: capture shows what VRChat
actually delivered; replay shows what your `.controller` did with it.

`osc query --json` emits the parameter list (name, OSC type, read/write access) for tooling.

`osc gestures` runs the **analog-gesture daemon** (`avatar-osc-gestures`, "Vive advanced controls on
any hardware"): it maps a controller's analog trigger to a VRChat gesture + analog weight and sends
them over OSC, so the Fist gesture blends by trigger pull on any headset. Headless there is no
on-device input yet (OpenXR is the production backend), so the CLI drives a synthetic trigger sweep
you can watch against a running VRChat. See [`reference/osc-runtime.md`](reference/osc-runtime.md).

There is also a renderer-agnostic **runtime rig layer** (`avatar-mesh` / `avatar-gltf` /
`avatar-pose` / `avatar-input`) for loading and posing a rig at runtime — see
[`reference/rig-runtime.md`](reference/rig-runtime.md).

## Hand it to an agent — `avatar mcp serve`

Everything read-shaped above is also exposed over the Model Context Protocol: `avatar mcp serve`
runs a stdio MCP server any agent host can connect to (describe/lint/stats/physbone-list plus
text-returning generation tools). It never writes to disk — generated YAML comes back as text, and
writes stay on the CLI behind its dry-run-safe guard. See [`reference/mcp.md`](reference/mcp.md).

## See also

- The rendered docs site, including the in-browser WebAssembly FBX analyzer:
  [andrewaltimit.github.io/avatar](https://andrewaltimit.github.io/avatar/).
- Library usage examples you can run: `crates/cli/examples/` (`cargo run -p avatar-cli --example
  lint_report -- <project>`, `--example perf_stats -- <path>`).
- [`../CONTRIBUTING.md`](../CONTRIBUTING.md) — build / test / lint and how to add a rule or a crate.
</content>
