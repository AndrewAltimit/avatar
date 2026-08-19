# avatar-migrate

**SDK2 → SDK3 (Avatars 3.0) migration** of a VRChat avatar project. Package `avatar-migrate` ·
library `avatar_migrate` · CLI `avatar migrate sdk3`. Part of the [avatar](../../README.md)
monorepo. Full behaviour: [`docs/reference/migrate.md`](../../docs/reference/migrate.md).

## What it does

Takes an extracted SDK2 avatar project (what `avatar unitypackage extract` produces), rewrites the
avatar prefab into an SDK3 one **in place** (byte-preserving where untouched — via
[`avatar-unity-yaml`](../unity-yaml/README.md)'s `EditableUnityFile`), and assembles a fresh Unity
project around it that the VRChat Creator Companion can open:

- `VRC_AvatarDescriptor` (SDK2) → `VRCAvatarDescriptor` at the same fileID (view position, IPD,
  visemes, portrait offsets carried; SDK-default playable layers + a generated FX layer; eye look
  derived from the rig geometry; blink blendshape wired). `PipelineManager` re-pointed at the SDK3
  DLL (blueprint cleared — an SDK3 upload is a new avatar).
- Root `Animator`: `applyRootMotion` off, controller cleared.
- DynamicBone / DynamicBoneCollider → `VRCPhysBone` / `VRCPhysBoneCollider` in place (same
  fileIDs), using the SDK's **own** conversion rules (disassembled from
  `PhysBoneMigration.Convert`, SDK 3.10.4). Optionally Unity `Cloth` dropped, its `CapsuleCollider`s
  retyped as PhysBone colliders, and new PhysBone chains added on named roots (Cloth skirt →
  PhysBone skirt).
- Named subtrees stripped (haptics vests, stray cameras).
- SDK2 gesture overrides → clean blendshape clips + an either-hand gesture FX layer
  ([`avatar-anim-gen`](../anim-gen/README.md)); empty expression menu/params generated.
- `Assets/` copied minus exclusions (SDK2 `VRCSDK`, examples, DynamicBone scripts), plus
  `Packages/vpm-manifest.json` (`com.vrchat.avatars`), a VCC-template `manifest.json` (incl.
  `com.unity.test-framework`, without which the SDK doesn't compile) and `ProjectVersion.txt`.
- `--vpm-package` bundles VPM packages (e.g. a shader package's release zip) into `Packages/`;
  `--relink-locked-shaders` re-points materials from a locker's generated `Hidden/…` shader copy
  to their `OriginalShader` (found in the project or bundled packages) and drops the copies.
- A report (`--json`; schema `avatar schema migrate`) of every conversion, warning (locomotion
  overrides not migrated, shaders with missing includes, …) and the remaining Unity-side steps.

## Key API

- `MigrateOptions::new(source_project, output, avatar_name)` + fields (`strip`, `drop_cloth`,
  `capsules_to_physbone_colliders`, `physbone_roots: Vec<PhysBoneRootSpec>`, `eye_bones`,
  `blink_shape`, `fx_from_overrides`, `exclude`, `sdk_version`, `unity_version`, `dry_run`) →
  `migrate(&opts) -> Result<MigrationReport>`.
- `sdk3`: the SDK3 script references (`VRC_AVATAR_DESCRIPTOR`, `VRC_PHYS_BONE`, …, all
  `{fileID: <MD4 class hash>, guid: <dll guid>}` from SDK 3.10.4) and body emitters
  (`DescriptorSpec`, `PhysBoneSpec::{new, from_dynamic_bone, from_yaml, to_body}` — a full
  read/modify/write spec incl. per-chain `Curve`s, `PhysBoneColliderSpec`, `pipeline_manager_body`).
- `physbone`: post-migration tuning on any SDK3 prefab — `list`/`info`/`find` (`PhysBoneInfo`:
  root, chains, colliders, tuning), `set(&mut rw, id, &Tuning, ignore±, colliders±)`,
  `split(&mut rw, id, chains, &Tuning)`, `stretch(&mut rw, id, factor, from_depth)`,
  `flare(&mut rw, id, FlareTarget, hinge_depth)` — wired to
  `avatar physbone list|set|split|stretch|flare|nudge` ([`docs/reference/physbone.md`](../../docs/reference/physbone.md)).
- `sdk2::Sdk2Avatar::read(&Scene)` — structural recognition of the SDK2 descriptor, PipelineManager,
  DynamicBone(+Collider), Cloth, CapsuleCollider, root Animator.
- `scene::Scene` — the prefab graph (transform tree, components, world-space composition);
  `rewrite::PrefabRewriter` — strip subtree / remove / retype / add component over `EditableUnityFile`.
- `fx::build_fx_from_overrides` — override controller → FX bundle; `eyelook::derive_eye_look`.
- `packages::{VpmPackage, ShaderIndex, relink_locked_materials}` — VPM bundling + locked-shader relink.

## Status

Built and green: unit tests per module + an end-to-end golden test over the synthetic
`fixtures/projects/Sdk2Project` (report, migrated prefab, FX controller pinned). Validated on a
real 2021 SDK2 avatar export (`avatar lint` on the output: 0 errors/warnings, `avatar stats`
computes the PhysBone metrics), and that output opened in Unity 2022.3 via VCC, built and uploaded
to VRChat unchanged. The PhysBone tuning pass (`physbone`) is golden-tested on the same fixture
(list → split → set → stretch pinned) and was applied to that avatar.
