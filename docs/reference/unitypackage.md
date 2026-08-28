# `.unitypackage` reading + the avatar/world testbed

`avatar-unitypackage` (lib `avatar_unitypackage`, CLI `avatar unitypackage`) reads Unity's
`.unitypackage` distribution format and bridges it to the rest of the toolchain, plus a file-level
"test an avatar inside a world/map" cross-check.

## The format

A `.unitypackage` is a **gzip-compressed tar**. There is no manifest; instead every asset is a
top-level directory named by its Unity **GUID** (32 hex chars), containing:

| Member | Contents |
|--------|----------|
| `pathname` | The project-relative path the asset had on export, e.g. `Assets/Avatar/final.fbx`. First non-empty line is the path; Unity occasionally appends trailing lines, which are ignored. |
| `asset` | The raw asset bytes. **Absent** for folder entries (a directory has a `pathname` and a `.meta` but no `asset`). |
| `asset.meta` | The Unity `.meta` sidecar (YAML) — same `guid`, plus import settings. |
| `preview.png` | Optional thumbnail. Ignored. |

The reader loads the whole archive into memory and indexes it by GUID. (A large world export can be
~1 GB uncompressed; peak RSS for the `testbed` cross-check of a ~90 MB avatar against a ~700 MB world
is ~1.1 GB.)

## Reading & summarizing — `avatar unitypackage info`

`UnityPackage::summary()` reports entry/file/folder counts, total asset bytes, a size breakdown by
extension, and heuristic `PackageTraits`:

- **`vrc_sdk`** — `Sdk2`, `Sdk3Avatars`, `Sdk3Worlds`, `Unknown`, or none. Determined from the
  runtime plugin DLLs (`VRCSDK2.dll` vs `VRCSDK3*.dll`) and the VPM package paths
  (`com.vrchat.avatars` / `com.vrchat.worlds`) — **not** from `VRCSDK/version.txt`, which is
  date-based (e.g. `2021.04.21...`) and whose 2021-era bundle shipped both SDK2 and SDK3 editor
  scripts in one folder, making the source tree alone ambiguous.
- **`looks_like_avatar`** — a VRChat avatar prefab (`prefab-id-v1_avtr_*.prefab`) is present.
- **`looks_like_world`** — scenes present and no avatar prefab.

## Listing — `avatar unitypackage list`

`list` prints every asset's path, GUID, and size without extracting anything — the quick way to
find the prefab or FBX inside a package before committing to a full `extract`. `--filter STR`
keeps only paths containing the substring (case-insensitive), `--folders` includes folder entries
(default: files only), `--json` emits the rows as a machine-readable report.

## Extracting — `avatar unitypackage extract -o <dir>`

`extract` reconstructs a normal Unity project tree under the destination: each asset at
`<dest>/<pathname>` with its `.meta` written as `<dest>/<pathname>.meta`. Because the lint/stats and
FBX/armature tools only need files on disk plus the `.meta` GUID index, an extracted package is
immediately consumable:

```sh
avatar unitypackage extract avatar.unitypackage -o /tmp/proj
avatar lint  /tmp/proj
avatar stats /tmp/proj/Assets/Avatar/.../final.fbx
avatar armature check /tmp/proj/Assets/Avatar/.../final.fbx
```

**Non-project paths are refused.** Legitimate assets are always project-relative (`Assets/`,
`Packages/`, `ProjectSettings/`, …). Old SDK exports sometimes leak absolute paths to bundled editor
DLLs (e.g. `C:/Program Files/Unity/Hub/Editor/.../UnityEngine.UI.dll`). The extractor rejects
absolute POSIX paths, Windows drive letters (`C:/…` — important on non-Windows hosts, where the std
path parser treats `C:` as an ordinary segment), UNC roots, and `..` traversal; these are counted in
`ExtractReport::skipped_unsafe` and not written.

## The testbed — `avatar unitypackage testbed <avatar> <world>`

Testing an avatar "in a map" offline is, at the file level, the question: *if I import both packages
into one Unity project, what breaks?* `UnityPackage::overlap` answers it:

- **GUID collisions** — the same GUID in both packages. Flagged `identical` when the asset bytes
  match (a harmless duplicate); otherwise Unity keeps whichever was imported last, **silently
  changing** one package's asset (e.g. two shader or SDK versions fighting).
- **Path collisions** — different GUIDs claiming the same `Assets/...` path. The second import
  overwrites the first file on disk while keeping a different GUID, so references can dangle.

`--strict` exits non-zero when any conflicting (different-bytes) GUID collision or path collision
exists, for gating. `--json` emits both package summaries and the full overlap.

A clean result means the avatar can be dropped into the world's project to preview scale, lighting,
and placement without clobbering the world's assets. Cross-checking two *platform variants* of one
world (e.g. a world's PC vs Quest export) is expected to show large overlap with many
content-conflicting GUIDs (platform-recompressed textures/materials and serialized Udon programs).

## Notes

- SDK2 avatars surface their age through the rest of the toolchain naturally: `avatar lint` reports
  `VRC001` (no `com.vrchat.avatars` package) and finds no SDK3 descriptor, and `avatar stats` on the
  project finds no avatar because the prefab carries the SDK2 `VRC_AvatarDescriptor`, not the SDK3
  `VRCAvatarDescriptor`. That is no longer a dead end: `avatar migrate sdk3` consumes exactly this
  extractor's output and rewrites the prefab to SDK3 ([`migrate.md`](migrate.md)). Run
  `avatar stats <fbx>` directly for the geometry rank regardless.
- The reader holds asset bytes in memory; it is built for one-shot CLI runs, not streaming.
