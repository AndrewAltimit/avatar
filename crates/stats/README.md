# avatar-stats

Offline VRChat **avatar performance ranking**. Package `avatar-stats` · library `avatar_stats`.
Part of the [avatar](../../README.md) monorepo.

## What it does

Reproduces VRChat's Excellent → Very Poor performance grade from the files an avatar is made of —
no Unity, no upload. VRChat ranks each *metric* (triangles, material slots, PhysBone components, …)
against a fixed table of per-tier limits, and the avatar's overall rank is the **worst** single
metric; this crate computes both.

- **`analyze_fbx(path)`** — the geometry side, from an FBX: Triangles, Skinned / Basic Meshes,
  Material Slots, Bones. The "is my mesh too heavy before I even open Unity?" check.
- **`analyze_project(path)`** — the full picture, per avatar (per VRC Avatar Descriptor found):
  components (PhysBone components & colliders, contacts, particle systems, lights, audio sources,
  renderers, material slots, cloth/physics/animators), plus **triangles** and **bones** resolved
  through the project's mesh references, **texture memory (PC + Android)** estimated through the
  material → texture chain, and **PhysBone affected-transform & collision-check counts** from
  walking the bone hierarchy under each PhysBone root.

Each source measures only *part* of the full rank, so every report lists the rank-affecting metrics
it could **not** evaluate (`not_evaluated`) — a clean rank from a bare FBX doesn't account for the
component side, and vice versa. Overall rank is the worst of the *measured* metrics.

VRChat dynamics components (PhysBone / collider / contact) are `MonoBehaviour`s, so — like
[`avatar-vrc-descriptor`](../vrc-descriptor/README.md) — they're recognized **structurally** by
their distinctive serialized fields (`collisionTags` ⇒ contact, `bonesAsSpheres`/`insideBounds` ⇒
collider, `endpointPosition`/`multiChildType` ⇒ PhysBone), not by a hardcoded script GUID, so it
keeps working across SDK versions. Everything else is counted by stable Unity class id.

## Key API

- `analyze_fbx(&Path) -> Result<PerfReport>` and `analyze_project(&Path) -> Result<Vec<PerfReport>>`.
- `PerfReport` — `stats: Vec<MetricStat>`, `not_evaluated`, and `overall(Platform)`.
- `MetricStat` — `metric`, `name`, `value`, and per-platform `pc` / `android` `Rank`.
- `Metric` — the ranked metrics; `Metric::limits(Platform)` is the threshold table as data.
- `Rank` (Excellent → VeryPoor, ordered by severity) and `Platform` (Pc / Android).

Thresholds are encoded as data on `Metric`, not scattered constants, so a VRChat limit change is a
one-line edit (PLAN §7, risk 3). Numbers from VRChat's
[performance ranking system](https://creators.vrchat.com/avatars/avatar-performance-ranking-system/).

## Status

Built. FBX geometry side and project component side both land; the rank engine + PC/Android
threshold tables are complete. The project view resolves **triangles** (each renderer's `m_Mesh`
guid → source FBX, summed; distinct files counted once, unresolved meshes flagged), **bones**
(distinct `m_Bones` transforms), and **texture memory for both PC and Android** (renderer → material
→ texture chain; per-texture VRAM estimated from image dimensions + per-platform import format —
DXT/BC for PC, ASTC/ETC2 for Android — an estimate, documented as such), and **PhysBone
affected-transform / collision-check counts** (walking the transform hierarchy under each PhysBone's
`rootTransform` — descendants minus `ignoreTransforms`, plus endpoints, × assigned colliders; an
estimate of VRChat's Avatar-Dynamics cost), for a unified geometry+component rank. **Total particles**,
**constraint count + depth**, **mesh-particle polygon cost** (mesh-mode renderer triangles × particle
count, resolved through the renderer→mesh→FBX chain — approximate), and **particle trail/collision
flags** are now measured too, so `not_evaluated` is empty by default.

## Features

- `schema` (off by default) — derive [`schemars::JsonSchema`](https://crates.io/crates/schemars) on
  `PerfReport` (and `MetricStat`/`Rank`) so a consumer can emit a JSON Schema for the report. The
  `avatar` CLI enables it (on by default) to back `avatar schema stats`.

## See also

- [`docs/reference/performance-stats.md`](../../docs/reference/performance-stats.md) — the metrics,
  the threshold tables, and how each value is measured.
