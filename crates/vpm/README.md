# avatar-vpm

Discovery and parsing of a VRChat/Unity project. Package `avatar-vpm` · library `avatar_vpm`. Part
of the [avatar](../../README.md) monorepo.

## What it does

Locates a project root and reads the metadata that matters for linting: the VPM manifest
(`Packages/vpm-manifest.json`), the editor version (`ProjectSettings/ProjectVersion.txt`), and the
asset locations.

## Key API

- `UnityProject::discover(start) -> Result<UnityProject>` — walk up from any path to the project root
  and parse its manifest + editor version.
- `assets_dir()`, `package_version(name)`, `has_avatar_sdk()` — project queries used by the linter.
- `Package` — one installed VPM package (`name`, `version`, `locked`).
- `AVATAR_SDK` / `BASE_SDK` — the `com.vrchat.avatars` / `com.vrchat.base` package ids.

## Usage

```rust,no_run
use avatar_vpm::UnityProject;
use std::path::Path;

let project = UnityProject::discover(Path::new("./MyAvatarProject"))?;
println!("Unity {:?}", project.unity_version);
if project.has_avatar_sdk() {
    println!("avatars SDK {:?}", project.package_version(avatar_vpm::AVATAR_SDK));
}
let assets = project.assets_dir();
# let _ = assets;
# anyhow::Ok(())
```

## Status

Built: **M2**.

## See also

- [VPM / Creator Companion docs](https://vcc.docs.vrchat.com/vpm/).
