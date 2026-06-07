# avatar-cli

The `avatar` command-line binary. Package `avatar-cli` · binary `avatar`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Ties the library crates together behind a [`clap`](https://crates.io/crates/clap) v4 subcommand
interface. Each subcommand has a `--json` flag for machine-readable output.

## Subcommands

| Command | What it does |
|---------|--------------|
| `avatar fbx inspect <path>` | Dump an FBX's structure (objects, hierarchy, global settings); flag unit/orientation issues. |
| `avatar armature check <path>` | Validate the skeleton against VRChat humanoid rig requirements. Exits non-zero when a Unity-required bone is missing (not humanoid-ready), to gate CI. |
| `avatar armature fix <path> [-o out.fbx]` | Plan repairs; with `-o`, write a repaired FBX (canonical bone renames are applied; mis-wired topology and scale/orientation are flagged). Dry-run by default; `--force` to overwrite input. |
| `avatar lint <project> [--deny-warnings]` | SDK3-compliance report over a Unity/VRChat project. Exits non-zero on errors (or, with `--deny-warnings`, any warnings) to gate CI. |
| `avatar stats <path>` | VRChat performance ranking (Excellent→Very Poor) of an FBX (geometry) or every avatar in a Unity project (components), against PC + Android limits. |
| `avatar anim-gen blendtree …` | Generate a 1D analog-gesture FX blend tree (Unity YAML) blending `GestureLeftWeight`/`…Right` across child clips. `--tree-only` for just the `BlendTree` document; otherwise a self-contained state-machine fragment. |
| `avatar anim-gen clip …` | Generate a `.anim` clip from `--blendshape PATH:SHAPE:VALUE` and/or `--toggle PATH` curves. |
| `avatar osc send <name> <value>` | Set one `/avatar/parameters/<name>` on a running VRChat (auto-detects bool/int/float; `--type` to force; `--host`/`--port` to retarget). |
| `avatar osc input <name> <value>` | Send a VRChat `/input/<axis|button>` (axis float `-1..1`, button `true`/`false`). |
| `avatar osc monitor [--seconds N]` | Listen for the avatar parameters VRChat broadcasts and print each update. |
| `avatar osc change <avtr-id>` | Ask VRChat to load a different avatar. |
| `avatar osc query <config.json>` | Parse an avatar's OSCQuery config JSON and list its parameters (offline; `--json` available). |
| `avatar osc gestures` | Run the analog-gesture daemon ("Vive advanced controls on any hardware"): controller trigger → `Gesture*`/`Gesture*Weight`. No on-device input backend headless yet, so it drives a synthetic demo sweep (`--hz`/`--period`/`--seconds`). |
| `avatar unitypackage info\|list\|extract\|testbed <pkg>` | Inspect a `.unitypackage` (contents, detected SDK, avatar/world), list its assets, extract it into a Unity `Assets/` tree, or cross-check an avatar package against a world package for co-import conflicts. |
| `avatar render [--avatar X] [--world Y] -o out.png` | Offscreen GPU preview → PNG. With both, the avatar is dropped at the world's player-spawn point at human scale; `--frame avatar\|world`, `--width/--height/--yaw/--pitch`. |
| `avatar view [--avatar X] [--world Y]` | Open an interactive window onto the same scene: drag = orbit, wheel = zoom, WASD/Space/Shift = walk, R = reset, Esc = quit. Needs a display (cli `viewer` feature, on by default). |

```sh
cargo run -p avatar-cli -- armature fix model.fbx            # dry run
cargo run -p avatar-cli -- armature fix model.fbx -o fixed.fbx
cargo run -p avatar-cli -- anim-gen clip --name Smile --blendshape Body:Smile:100 -o Smile.anim
cargo run -p avatar-cli -- anim-gen blendtree --parameter GestureLeftWeight \
    --clip <relaxed-guid>@0.0 --clip <fist-guid>@1.0 -o FistBlend.asset
cargo run -p avatar-cli -- osc send VRCEmote 3                # wave, on a running VRChat
cargo run -p avatar-cli -- osc query ~/.../OSC/usr_…/Avatars/avtr_….json
cargo run -p avatar-cli -- osc gestures --seconds 10         # analog-gesture demo sweep
cargo run -p avatar-cli -- unitypackage extract avatar.unitypackage -o avatar-proj
cargo run -p avatar-cli -- render --avatar avatar.fbx --world world/Assets/Scene.unity -o in-world.png
cargo run -p avatar-cli -- view   --avatar avatar.fbx --world world/Assets/Scene.unity  # interactive
```

## Exit codes

Commands that validate exit non-zero on failure, so they gate CI directly:

- `armature check` — fails when the rig is not humanoid-ready (a Unity-required bone is missing).
- `lint` — fails on any error; with `--deny-warnings`, also on any warning.

Other commands (`fbx inspect`, `armature fix`) exit zero unless they hit a hard error (e.g. an
unreadable file).

## Status

`fbx inspect` / `armature check`: **M1**. `lint`: **M2**. `armature fix`: **M3**. `stats`: built.
`anim-gen`: **M4** (library + CLI). `osc` / `osc gestures`: **M5** (library + CLI; the daemon's
on-device OpenXR input backend is the remaining runtime piece).

## See also

- [`README.md`](../../README.md) — quick start.
- [`docs/tutorial.md`](../../docs/tutorial.md) — end-to-end CLI walkthrough.
- [`docs/reference/armature-repair.md`](../../docs/reference/armature-repair.md) — `armature fix`
  behaviour.
- [`docs/reference/anim-gen.md`](../../docs/reference/anim-gen.md) — `anim-gen` generation.
- [`docs/reference/osc-runtime.md`](../../docs/reference/osc-runtime.md) — `osc` address space + codec.
