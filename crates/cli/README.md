# avatar-cli

The `avatar` command-line binary. Package `avatar-cli` · binary `avatar`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Ties the library crates together behind a [`clap`](https://crates.io/crates/clap) v4 subcommand
interface. The read and generate commands have a `--json` flag for machine-readable output, and
`avatar schema` publishes the JSON Schema of those report types so the output is a stable contract.
The generators (`anim-gen …`) are write-safe: `--dry-run` previews without writing, and an existing
output file is never overwritten without `--force`.

## Subcommands

| Command | What it does |
|---------|--------------|
| `avatar describe <path>` | One-shot consolidated snapshot: for an FBX, structure + humanoid-armature analysis + geometry rank; for a project, lint + per-avatar rank. Gates non-zero on a non-humanoid-ready rig or lint errors. Built for "one call → full mental model." |
| `avatar fbx inspect <path>` | Dump an FBX's structure (objects, hierarchy, global settings); flag unit/orientation issues. |
| `avatar armature check <path>` | Validate the skeleton against VRChat humanoid rig requirements. Exits non-zero when a Unity-required bone is missing (not humanoid-ready), to gate CI. |
| `avatar armature fix <path> [-o out.fbx]` | Plan repairs; with `-o`, write a repaired FBX (canonical bone renames are applied; mis-wired topology and scale/orientation are flagged). Dry-run by default; `--force` to overwrite input. |
| `avatar lint <project> [--deny-warnings]` | SDK3-compliance report over a Unity/VRChat project. Exits non-zero on errors (or, with `--deny-warnings`, any warnings) to gate CI. |
| `avatar stats <path>` | VRChat performance ranking (Excellent→Very Poor) of an FBX (geometry) or every avatar in a Unity project (components), against PC + Android limits. |
| `avatar anim-gen blendtree …` | Generate a 1D analog-gesture FX blend tree (Unity YAML) blending `GestureLeftWeight`/`…Right` across child clips. `--tree-only` for just the `BlendTree` document; otherwise a self-contained state-machine fragment. |
| `avatar anim-gen clip …` | Generate a `.anim` clip from `--blendshape PATH:SHAPE:VALUE` and/or `--toggle PATH` curves. |
| `avatar anim-gen controller …` | Generate a complete, Unity-importable FX `AnimatorController` (class 91) wrapping an analog-gesture blend tree in one layer — the full M4 asset, not the splice-in fragment. |
| `avatar asset set <file> --path P --value V\|--ref …` | Surgically edit one value in an *existing* Unity YAML asset (scalar, reference re-target, or flow-map subfield), preserving fileIDs/refs/key-order/formatting byte-for-byte. `--doc <fileID>` selects the document in a multi-doc file; stdout preview by default, `-o`+`--force` to write in place, `--dry-run`/`--json` supported. |
| `avatar physbone list\|set\|split\|stretch\|flare\|nudge <prefab> …` | Inspect and retune an SDK3 prefab's `VRCPhysBone`s in place: `list` (root, chains + lengths, colliders, tuning + curves; `--json`), `set TARGET --pull/--spring/--stiffness/--gravity/--immobile/--limit-type/--max-angle… --*-curve "0:0.7,1:1" --ignore/--collider…`, `split TARGET --chain A --chain B` (chains onto their own components, tuned apart), `stretch TARGET --factor 1.5|--by 0.077` (longer skirt/tail by scaling bone offsets below the root; `--by` adds equal length per chain so an even hem stays even; `--chain NAME=METERS` overrides one chain), `flare TARGET --angle 10\|--scale 0.5` (re-angle chains toward straight down: a funnel skirt hugs the legs), `nudge TARGET --out 0.008` (shift the hinge ring radially: lift a skirt off a waistband). Same write policy as `asset set` (`-o`+`--force` in place, `--dry-run`, `--json`). |
| `avatar schema [name\|all]` | Print the JSON Schema for a `--json` report type (`describe`/`lint`/`stats`/`armature`/`fbx-inspect`/`migrate`/`physbone`) so an agent can introspect the output contract. Built on the default-on `schema` feature. |
| `avatar osc send <name> <value>` | Set one `/avatar/parameters/<name>` on a running VRChat (auto-detects bool/int/float; `--type` to force; `--host`/`--port` to retarget). |
| `avatar osc input <name> <value>` | Send a VRChat `/input/<axis|button>` (axis float `-1..1`, button `true`/`false`). |
| `avatar osc monitor [--seconds N]` | Listen for the avatar parameters VRChat broadcasts and print each update. |
| `avatar osc change <avtr-id>` | Ask VRChat to load a different avatar. |
| `avatar osc query <config.json>` | Parse an avatar's OSCQuery config JSON and list its parameters (offline; `--json` available). |
| `avatar osc gestures` | Run the analog-gesture daemon ("Vive advanced controls on any hardware"): controller trigger → `Gesture*`/`Gesture*Weight`. No on-device input backend headless yet, so it drives a synthetic demo sweep (`--hz`/`--period`/`--seconds`). |
| `avatar unitypackage info\|list\|extract\|testbed <pkg>` | Inspect a `.unitypackage` (contents, detected SDK, avatar/world), list its assets, extract it into a Unity `Assets/` tree, or cross-check an avatar package against a world package for co-import conflicts. |
| `avatar render [--avatar X] [--world Y] -o out.png` | Offscreen GPU preview → PNG. With both, the avatar is dropped at the world's player-spawn point at human scale; `--frame avatar\|world`, `--width/--height/--yaw/--pitch`; `--pose X.prefab` draws the FBX in that prefab's pose (what Unity will show), `--stretch 'Skirt_0_*:1.5'` previews a `physbone stretch` alone. |
| `avatar view [--avatar X] [--world Y]` | Open an interactive window onto the same scene: drag = orbit, wheel = zoom, WASD/Space/Shift = walk, R = reset, Esc = quit. Needs a display (cli `viewer` feature, on by default). |
| `avatar mcp serve` | Run a Model Context Protocol server over stdio (JSON-RPC), exposing the **read-only** diagnose surface (`describe`/`lint`/`stats`/`armature`/`fbx-inspect`/`physbone-list`/`unitypackage-info`/`schema`) as tools an agent host can discover + call. Each returns the same JSON as the `--json` flags. |

```sh
cargo run -p avatar-cli -- describe model.fbx --json         # one-call snapshot, machine-readable
cargo run -p avatar-cli -- schema describe                   # the shape of that --json output
cargo run -p avatar-cli -- armature fix model.fbx            # dry run
cargo run -p avatar-cli -- armature fix model.fbx -o fixed.fbx
cargo run -p avatar-cli -- anim-gen clip --name Smile --blendshape Body:Smile:100 -o Smile.anim
cargo run -p avatar-cli -- anim-gen controller --name FX --clip <relaxed>@0.0 --clip <fist>@1.0 -o FX.controller
cargo run -p avatar-cli -- anim-gen blendtree --parameter GestureLeftWeight \
    --clip <relaxed-guid>@0.0 --clip <fist-guid>@1.0 -o FistBlend.asset
cargo run -p avatar-cli -- asset set Parameters.asset --path m_Name --value Params2  # preview to stdout
cargo run -p avatar-cli -- asset set Hands.controller --doc 110600000 \
    --path m_BlendParameter --value GestureLeftWeight -o Hands.controller --force      # fix in place
cargo run -p avatar-cli -- osc send VRCEmote 3                # wave, on a running VRChat
cargo run -p avatar-cli -- osc query ~/.../OSC/usr_…/Avatars/avtr_….json
cargo run -p avatar-cli -- osc gestures --seconds 10         # analog-gesture demo sweep
cargo run -p avatar-cli -- unitypackage extract avatar.unitypackage -o avatar-proj
cargo run -p avatar-cli -- render --avatar avatar.fbx --world world/Assets/Scene.unity -o in-world.png
cargo run -p avatar-cli -- view   --avatar avatar.fbx --world world/Assets/Scene.unity  # interactive
cargo run -p avatar-cli -- mcp serve                         # stdio MCP server for an agent host
```

## Exit codes

Commands that validate exit non-zero on failure, so they gate CI directly:

- `armature check` — fails when the rig is not humanoid-ready (a Unity-required bone is missing).
- `lint` — fails on any error; with `--deny-warnings`, also on any warning.
- `describe` — fails on a non-humanoid-ready FBX or a project with lint errors (it shares the gating
  logic of the commands it aggregates).

Other commands (`fbx inspect`, `armature fix`, `anim-gen …`, `asset set`, `schema`) exit zero unless
they hit a hard error (e.g. an unreadable file, or a refused overwrite without `--force`).

## Status

`fbx inspect` / `armature check`: **M1**. `lint`: **M2**. `armature fix`: **M3**. `stats`: built.
`anim-gen` (now incl. `controller`): **M4** (library + CLI; CLI-generated `.anim`/`.controller`
assets are gated against a real editor by the Unity-acceptance workflow). `osc` / `osc gestures`:
**M5** (library + CLI; the daemon's on-device OpenXR input backend is the remaining runtime piece).
`asset set`: built (surgical, round-trip-safe value edits to an existing Unity asset, behind the same
`WriteGuard`). `describe` / `schema` and the generators' `--dry-run`/`--force` write-safety: built
(agent ergonomics). `mcp serve`: built (read-only tool surface over MCP; generation tools next).

## See also

- [`README.md`](../../README.md) — quick start.
- [`docs/tutorial.md`](../../docs/tutorial.md) — end-to-end CLI walkthrough.
- [`docs/reference/armature-repair.md`](../../docs/reference/armature-repair.md) — `armature fix`
  behaviour.
- [`docs/reference/anim-gen.md`](../../docs/reference/anim-gen.md) — `anim-gen` generation.
- [`docs/reference/unity-yaml-edit.md`](../../docs/reference/unity-yaml-edit.md) — `asset set` /
  `EditableUnityFile` surgical-edit model.
- [`docs/reference/osc-runtime.md`](../../docs/reference/osc-runtime.md) — `osc` address space + codec.
- [`docs/reference/mcp.md`](../../docs/reference/mcp.md) — `mcp serve` tools, handshake, error model.
