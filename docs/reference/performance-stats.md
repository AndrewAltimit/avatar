# Avatar performance stats (`avatar stats`)

`avatar stats <path>` estimates an avatar's VRChat **performance ranking** (Excellent → Very Poor)
from the files it's made of, with no Unity. It is implemented by the `avatar-stats` crate.

```sh
avatar stats model.fbx                # geometry-side rank of an FBX
avatar stats path/to/UnityProject     # component-side rank of every avatar in a project
avatar stats model.fbx --json         # machine-readable report
```

The command is **informational** — it always exits 0 on success (unlike `lint`/`armature check`,
which gate CI).

## How the rank works

VRChat ranks each *metric* against a table of per-tier limits and takes the **worst** single metric
as the avatar's overall rank. A measured value earns the first tier whose bound it does not exceed
(bounds are inclusive, checked Excellent → Poor); anything past the Poor bound is **Very Poor**.

Because an FBX and a Unity project each only carry part of the picture, a report covers a *subset* of
the metrics and lists the rest under **"Not evaluated"** — a clean rank there isn't the whole story.
The overall rank is the worst of the metrics actually measured.

## What each source measures

| Metric | From FBX (`model.fbx`) | From project (avatar in a prefab/scene) | How |
|---|---|---|---|
| Triangles | ✅ | ✅ | FBX: sum of triangles across extracted meshes. Project: resolve each renderer's `m_Mesh` guid to its source FBX (via the `.meta` index) and sum; distinct source files counted once. |
| Texture Memory | — | ✅ (PC + Android) | Project: renderer → material → texture chain; estimate per-texture VRAM from image dimensions + import settings, per platform (DXT/BC for PC, ASTC/ETC2 for Android — see below). |
| Skinned Meshes | ✅ | ✅ | FBX: skinned `RawMesh`es. Project: `SkinnedMeshRenderer` (class 137). |
| Basic Meshes | ✅ | ✅ | FBX: non-skinned meshes. Project: `MeshRenderer` (class 23). |
| Material Slots | ✅ | ✅ | FBX: `Material → Model` connections. Project: Σ `m_Materials` over renderers. |
| Bones | ✅ | ✅ | FBX: distinct bones driving skin clusters (fallback: bone-like model count). Project: distinct `m_Bones` transforms across skinned mesh renderers. |
| PhysBone Components | — | ✅ | `MonoBehaviour` with `endpointPosition`/`multiChildType`. |
| PhysBone Colliders | — | ✅ | `MonoBehaviour` with `bonesAsSpheres`/`insideBounds`. |
| PhysBone Affected Transforms | — | ✅ | Σ over PhysBones of the moving transforms under each root (see below). |
| PhysBone Collision Check Count | — | ✅ | Σ over PhysBones of `affected transforms × assigned colliders`. |
| Contacts | — | ✅ | `MonoBehaviour` with `collisionTags` (sender + receiver). |
| Particle Systems | — | ✅ | `ParticleSystem` (class 198). |
| Total Particles | — | ✅ | Σ over `ParticleSystem`s of `min(maxParticles, ceil(rate × lifetime))` (see below). |
| Mesh Particle Polygons | — | ✅ | Σ over mesh-mode particle renderers of `mesh triangles × live-particle count` (see below). |
| Particle Trails | — | ✅ | Particle systems with a `TrailModule.enabled: 1`. |
| Particle Collision | — | ✅ | Particle systems with a `CollisionModule.enabled: 1`. |
| Constraints | — | ✅ | Unity built-in constraints (classes 320–325) + VRChat constraint `MonoBehaviour`s (structural). |
| Constraint Depth | — | ✅ | Longest chain of constraints driving one another (see below). |
| Lights | — | ✅ | `Light` (class 108). |
| Audio Sources | — | ✅ | `AudioSource` (class 82). |
| Trail / Line Renderers | — | ✅ | classes 96 / 120. |
| Cloths | — | ✅ | `Cloth` (class 183). |
| Physics Colliders | — | ✅ | Box / Mesh / Sphere / Capsule (classes 65 / 64 / 135 / 136). |
| Physics Rigidbodies | — | ✅ | `Rigidbody` (class 54). |
| Animators | — | ✅ | `Animator` (class 95). |

**Everything either source can reproduce from the files is now measured** — the project view's
`not_evaluated` list is empty by default. (Total particle count, mesh-particle polygons, the
particle trail/collision flags, and constraint count/depth all used to live here; they are now
measured — see the dedicated sections below.) An individual measured metric can still *add* a
`not_evaluated` note when its inputs were incomplete (e.g. a particle system whose modules can't be
parsed, a mesh-particle renderer whose mesh doesn't resolve to an FBX, or a constraint chain whose
sources don't resolve), marking that the value is a lower bound / the depth unknown.

### How components are recognized

Renderers, particles, lights, etc. are matched by **stable Unity class id**. The VRChat dynamics
components (PhysBone / collider / contact) are `MonoBehaviour`s whose script GUID changes across SDK
versions, so — mirroring `avatar-vrc-descriptor` — they're recognized **structurally** by their
distinctive serialized fields: a `collisionTags` list ⇒ a Contact, `bonesAsSpheres`/`insideBounds` ⇒
a PhysBone collider, `endpointPosition`/`multiChildType` ⇒ a PhysBone.

### How project triangles & bones are resolved

A `SkinnedMeshRenderer`/`MeshFilter`'s `m_Mesh` is a `{fileID, guid}` reference into an imported
model. `avatar stats` builds a `guid → path` index from the project's `.meta` files (as `lint`
does), resolves each renderer's mesh guid to its **source FBX**, loads it once (cached), and sums
its triangles. **Distinct source files are counted once** — an avatar split into several renderers
over a single body FBX counts that FBX's geometry one time (its triangle total already covers all
its sub-meshes). A mesh that resolves to a non-FBX source (a baked/built-in `.asset` mesh) or a
missing guid is reported as **unresolved**: its triangles are omitted (so the count is a lower
bound) and the gap is noted in `not_evaluated` rather than silently dropped.

Bones come straight from the prefab: the union of distinct `m_Bones` transforms across the skinned
mesh renderers — what VRChat counts — so no FBX load is needed.

### How project texture memory is estimated (PC + Android)

VRChat's Texture Memory is the GPU VRAM the avatar's textures occupy — and it **differs by
platform**, because the same source is recompressed to a different GPU format (DXT/BC on PC,
ASTC/ETC2 on Android). We can't query the imported format offline, so `avatar stats` estimates it
from what the files *do* reveal, following the renderer → `m_Materials` guid → `.mat` → `m_TexEnvs`
texture guid → texture file chain (every link resolved through the `.meta` index, results cached).
For each **distinct** texture (deduped across materials — VRChat counts unique texture objects):

1. **Dimensions + alpha** from the image header (PNG / PSD / TGA / JPEG); an unreadable/unsupported
   format (e.g. EXR) is reported as unresolved rather than guessed.
2. **Import settings** from the sibling `.meta` (`TextureImporter`): `maxTextureSize`, mipmaps,
   `textureCompression`, and any explicit `textureFormat`, reading the platform's own override
   (`Standalone` / `Android`) and falling back to the default platform.
3. **Bytes-per-pixel** from the explicit `textureFormat` when one is set (a table covering DXT/BC,
   ETC2, and ASTC block sizes), else the platform's Automatic-compression default: PC → DXT1 (0.5,
   opaque) / DXT5·BC7 (1.0, alpha); Android → ASTC, block size by quality (HQ 4×4 = 1.0, normal
   6×6 ≈ 0.44, LQ 8×8 = 0.25). Uncompressed is 4 bpp either way.

`bytes = effective_w · effective_h · bpp · mip_factor`, where the effective size scales the source
down so neither side exceeds `maxTextureSize` (aspect preserved) and `mip_factor` is 4/3 when
mipmaps are on. Both platform totals are computed; the CLI shows them as `PC/Android MB`, ranked
against each platform's own thresholds. This is an **estimate** — for textures left on Automatic we
apply Unity's default format choice, which can differ from the actual build — so treat it as a close
ballpark, not an exact byte count. Textures that don't resolve are flagged in `not_evaluated` (both
totals are then a lower bound).

**Project scope:** each avatar is counted over the whole file that declares it (carries a VRC Avatar
Descriptor) — accurate for the common "one avatar per prefab" case; a scene packing several avatars
into one file would over-count.

### How PhysBone affected-transform & collision-check counts are resolved

VRChat's two Avatar-Dynamics *cost* metrics depend on the bone hierarchy each PhysBone drives, so
`avatar stats` walks the prefab/scene's transform graph. It builds a `Transform fileID → children`
map (and `GameObject → Transform`) from the file's `Transform` documents (class 4 / 224), then for
each PhysBone (recognized structurally, as for the component count) it:

1. Finds the **root** — `rootTransform` when set, else the transform on the PhysBone's own
   GameObject.
2. Walks the root's subtree, **skipping any transform in `ignoreTransforms`** and its descendants.
3. Counts **affected transforms** = the root's descendants (the root itself is the immovable anchor
   and isn't simulated), **plus one endpoint** per chain tip when `endpointPosition` is non-zero
   (VRChat appends an endpoint bone to each leaf).
4. **Collision checks** for that PhysBone = its affected transforms × the number of colliders
   assigned in its `colliders` list; the report sums both quantities over all PhysBones.

This is an **estimate** following VRChat's documented model; it doesn't reproduce every
`multiChildType` nuance, and a PhysBone whose root transform is stripped out of a nested-prefab
override can't be resolved — such components are flagged in `not_evaluated` and both totals are then
a lower bound.

### How total particle count is estimated

VRChat ranks the **total live particles** an avatar can have on screen at once, on top of the
particle-*system* count. We can't run the simulation offline, so for each `ParticleSystem` (class
198) `avatar stats` reproduces VRChat's own ceiling, `min(maxParticles, ceil(rate × lifetime))`,
and sums it across systems:

- **`maxParticles`** = the system's `InitialModule.maxNumParticles` — Unity's hard cap on
  simultaneously-live particles (defaulting to 1000 if the field is absent).
- **`lifetime`** = the constant scalar of `InitialModule.startLifetime`.
- **`rate`** = the constant scalar of `EmissionModule.rateOverTime` **plus** `rateOverDistance`.

Each of `startLifetime` / `rateOverTime` is a Unity *MinMaxCurve*; we read its constant `scalar`
term (or a bare number for the simple form). This is an **estimate**: animated curves, emission
**bursts**, and **sub-emitters** (which spawn additional systems) are approximated by their constant
term — a close ballpark, not an exact ceiling, and on the low side for burst-heavy or sub-emitting
effects. A system that has no readable `InitialModule` (so its ceiling can't be bounded) is
**flagged** in `not_evaluated` and contributes nothing, making the total a lower bound — never
silently assumed to emit zero.

### How mesh-particle polygons & trail/collision flags are estimated

A particle system's renderer can draw each particle as a **mesh** rather than a billboard, which
multiplies that mesh's geometry by however many particles are alive — VRChat budgets the resulting
**active polygons**. The system (`ParticleSystem`, class 198) and its renderer
(`ParticleSystemRenderer`, class 199) are two documents on the same GameObject; `avatar stats` pairs
them by GameObject fileID. For each renderer in **Mesh** render mode (`m_RenderMode: 4`) it:

1. Resolves the renderer's `m_Mesh` (falling back to the first non-null `m_Meshes` entry) to its
   **source FBX** through the same `guid → path` `.meta` index and triangle cache used for the
   geometry triangle count.
2. Multiplies that FBX's triangles by the **sibling system's live-particle count** (the same
   `min(maxParticles, ceil(rate × lifetime))` estimate as Total Particles) and sums across renderers.

This shares `resolve_geometry`'s **lower-fidelity caveat**: triangles are summed per *distinct source
FBX* (so the count covers all the FBX's sub-meshes), which means a particle mesh that points at a
single sub-mesh of a multi-mesh FBX **over-counts** (it's charged the whole file's geometry). A
mesh-mode renderer whose mesh resolves to a non-FBX source (a baked/built-in `.asset` mesh) or a
missing guid is reported as **unresolved**: its polygons are omitted (so the count is a lower bound)
and the gap is noted in `not_evaluated`, never silently dropped.

The **trail** and **collision** flags are read straight off the system body: a
`TrailModule.enabled: 1` increments the Particle Trails count, a `CollisionModule.enabled: 1` the
Particle Collision count. Both are ranked like Lights (`0/0/0/1`): any system with the module
enabled costs a tier.

### How constraint count & depth are computed

`avatar stats` counts two kinds of constraint:

1. **Unity built-in constraints** — `PositionConstraint` (320), `RotationConstraint` (321),
   `ScaleConstraint` (322), `AimConstraint` (323), `ParentConstraint` (324), `LookAtConstraint`
   (325) — by stable class id.
2. **VRChat constraints** (`VRCConstraint` / `VRCParentConstraint` / …) — `MonoBehaviour`s whose
   script GUID changes across SDK versions, so they're recognized **structurally** (like the other
   VRC dynamics) by VRCConstraintBase's distinctive fields: a `Sources` list together with a
   `TargetTransform`.

**Depth** is the longest chain of constraints that drive one another — a constraint whose *source*
GameObject also carries a constraint, transitively. We build a `Transform fileID → GameObject` map
(the reverse of the PhysBone hierarchy map), resolve each constraint's source transforms
(`m_Sources[].sourceTransform` for built-ins, `Sources[].SourceTransform` for VRChat constraints) to
their GameObjects, and walk constraint → source-GameObject's constraints with a memoized, cycle-safe
DFS; a standalone constraint has depth 1.

This is **conservative**: depth is reported only when every source slot resolves within the file. An
empty `Sources` list is a legitimate leaf (depth 1), but a source slot that's unfilled (`fileID: 0`)
or points at a transform stripped out of a nested-prefab override means the chain is incomplete — in
that case the **count is still ranked** but the **depth is reported as unknown** (omitted from the
ranked metrics and noted in `not_evaluated`).

## The threshold tables

Encoded as data on `Metric::limits(Platform)` (PLAN §7, risk 3: rules as data, not constants).
Source: VRChat's [Avatar Performance Ranking System][src]. Each cell is the inclusive upper bound for
that tier; the next worse tier starts one above it, and anything over **Poor** is **Very Poor**.

### PC (Windows)

| Metric | Excellent | Good | Medium | Poor |
|---|---|---|---|---|
| Triangles | 32,000 | 70,000 | 70,000 | 70,000 |
| Skinned Meshes | 1 | 2 | 8 | 16 |
| Basic Meshes | 4 | 8 | 16 | 24 |
| Material Slots | 4 | 8 | 16 | 32 |
| Bones | 75 | 150 | 256 | 400 |
| PhysBone Components | 4 | 8 | 16 | 32 |
| PhysBone Colliders | 4 | 8 | 16 | 32 |
| PhysBone Affected Transforms | 16 | 32 | 64 | 128 |
| PhysBone Collision Check Count | 8 | 16 | 32 | 64 |
| Contacts | 8 | 16 | 24 | 32 |
| Particle Systems | 0 | 4 | 8 | 16 |
| Total Particles | 0 | 300 | 1,000 | 2,500 |
| Mesh Particle Polygons † | 0 | 2,000 | 20,000 | 50,000 |
| Particle Trails | 0 | 0 | 0 | 1 |
| Particle Collision | 0 | 0 | 0 | 1 |
| Constraints | 100 | 250 | 300 | 350 |
| Constraint Depth | 20 | 50 | 80 | 100 |
| Lights | 0 | 0 | 0 | 1 |
| Audio Sources | 1 | 4 | 8 | 8 |
| Trail Renderers | 1 | 2 | 4 | 8 |
| Line Renderers | 1 | 2 | 4 | 8 |
| Cloths | 0 | 1 | 1 | 1 |
| Physics Colliders | 0 | 1 | 8 | 8 |
| Physics Rigidbodies | 0 | 1 | 8 | 8 |
| Animators | 1 | 4 | 16 | 32 |

### Android / Quest (stricter; some metrics not ranked)

| Metric | Excellent | Good | Medium | Poor |
|---|---|---|---|---|
| Triangles | 7,500 | 10,000 | 15,000 | 20,000 |
| Skinned Meshes | 1 | 1 | 2 | 2 |
| Basic Meshes | 1 | 1 | 2 | 2 |
| Material Slots | 1 | 1 | 2 | 4 |
| Bones | 75 | 90 | 150 | 150 |
| PhysBone Components | 0 | 4 | 6 | 8 |
| PhysBone Colliders | 0 | 4 | 8 | 16 |
| PhysBone Affected Transforms | 0 | 16 | 32 | 64 |
| PhysBone Collision Check Count | 0 | 8 | 16 | 32 |
| Contacts | 2 | 4 | 8 | 16 |
| Particle Systems | 0 | 0 | 0 | 2 |
| Total Particles | 0 | 0 | 0 | 200 |
| Mesh Particle Polygons † | 0 | 0 | 2,000 | 20,000 |
| Particle Trails | 0 | 0 | 0 | 1 |
| Particle Collision | 0 | 0 | 0 | 1 |
| Constraints | 30 | 60 | 120 | 150 |
| Constraint Depth | 5 | 15 | 35 | 50 |
| Trail Renderers | 0 | 0 | 0 | 1 |
| Line Renderers | 0 | 0 | 0 | 1 |
| Animators | 1 | 1 | 1 | 2 |

Lights, Audio Sources, Cloths, and the physics colliders/rigidbodies are **not ranked on Android**
(those component types are stripped on mobile); `avatar stats` shows `-` for them in the Android
column.

† **Mesh Particle Polygons** thresholds are **approximate**. VRChat folds mesh-particle polygons
into its particle budget rather than publishing a standalone per-tier table, so these bounds are our
own triangle-style ramp — confirm against VRChat's published budget when one is available (PLAN
risk 3: rules as data, so a correction is a one-line edit to `Metric::defs`).

[src]: https://creators.vrchat.com/avatars/avatar-performance-ranking-system/
