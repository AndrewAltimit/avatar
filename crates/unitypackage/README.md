# avatar-unitypackage

Reader for Unity's `.unitypackage` distribution format. Package `avatar-unitypackage` ·
lib `avatar_unitypackage`. Part of the [avatar](../../README.md) monorepo.

## What it does

A `.unitypackage` is a gzip-compressed tar where every asset is a directory named by its Unity
**GUID**, holding `pathname` (the project-relative path), `asset` (the raw bytes), `asset.meta`
(the `.meta` sidecar), and an optional `preview.png`. This crate parses that archive and turns the
distribution format into something the rest of the toolchain can use:

- **Read & summarize** a package without unpacking it: counts, size-by-extension, and heuristic
  traits (which VRChat SDK it bundles, whether it looks like an avatar or a world/map).
- **Extract** it into a normal Unity `Assets/` tree — asset bytes at their pathname, each `.meta`
  written alongside — so `avatar lint`, `avatar stats`, and the FBX/armature tools (which only need
  files on disk plus their `.meta` GUID index) run against it unchanged.
- **Cross-check** two packages for co-import conflicts: the file-level "can I drop this avatar into
  this world's project to preview it?" question.

It deliberately stays out of the lint/cli type graph's heavier deps: just `flate2` (already in the
graph via fbxcel) + `tar`.

## Key API

- `UnityPackage::open(path)` / `read(reader)` — parse into a GUID-indexed [`Entry`] set. Each entry
  carries `guid`, `pathname`, `asset` (`None` for folders), and `meta`.
- `UnityPackage::summary()` → `PackageSummary` — entry/file/folder counts, total bytes,
  `by_extension`, and `PackageTraits` (`vrc_sdk`, `looks_like_avatar`, `looks_like_world`, …). SDK2
  vs SDK3 is read from the plugin DLLs (`VRCSDK2.dll` vs `VRCSDK3*.dll`) and VPM package paths, not
  the date-based `VRCSDK/version.txt`.
- `UnityPackage::extract(dest)` → `ExtractReport` — reconstruct the project tree. Refuses paths that
  are not cleanly project-relative (absolute POSIX, Windows drive `C:/…`, UNC, or `..`); old SDK
  exports leak absolute paths to bundled editor DLLs that are not project assets.
- `UnityPackage::overlap(&other)` → `OverlapReport` — GUID collisions (flagged `identical` when the
  bytes match, a harmless duplicate; otherwise one asset is silently overwritten on import) and
  path collisions (same path under different GUIDs).

```rust
use avatar_unitypackage::UnityPackage;
let avatar = UnityPackage::open("avatar.unitypackage".as_ref())?;
let world  = UnityPackage::open("CozyCabin.unitypackage".as_ref())?;
println!("avatar SDK: {:?}", avatar.summary().traits.vrc_sdk);
let report = avatar.overlap(&world);
if report.is_clean() { println!("safe to import the avatar into the world"); }
# anyhow::Ok(())
```

## CLI

Driven by `avatar unitypackage <subcommand>`:

- `info <pkg>` — summary (contents, SDK, avatar/world detection).
- `list <pkg> [--filter S] [--folders]` — list assets (guid, size, path).
- `extract <pkg> -o <dir>` — unpack into a Unity project tree.
- `testbed <avatar> <world> [--strict]` — report co-import GUID/path conflicts.

## Status

Built and green. Unit tests run on an in-memory synthesized package; an env-gated integration test
(`AVATAR_SAMPLE_UNITYPACKAGE`, plus `AVATAR_SAMPLE_UNITYPACKAGE_WORLD` for the overlap path) runs
the full pipeline against a real package. Behaviour:
[`docs/reference/unitypackage.md`](../../docs/reference/unitypackage.md).
