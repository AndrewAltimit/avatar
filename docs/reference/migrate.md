# SDK2 → SDK3 migration — `avatar-migrate` / `avatar migrate sdk3`

The migration takes an **extracted SDK2 avatar project** (as [`avatar unitypackage
extract`](unitypackage.md) produces) and turns it into an **SDK3 / Avatars 3.0** project the
VRChat Creator Companion can open, with the avatar prefab rewritten in place. It exists because
the last generation of avatars — Cloth skirts, DynamicBones, gesture override controllers, root
motion on the Animator — cannot be uploaded with today's SDK, and the manual conversion is a long,
error-prone Unity session. Everything except the final Unity/VCC open + SDK upload is done here.

```sh
avatar migrate sdk3 <extracted-project> -o <out-dir> --name MyAvatar \
  --strip BhapticsVRC_Vest --drop-cloth --capsules-to-physbone-colliders \
  --physbone "Hips|Spine,Left leg,Right leg,ButtTail1|L cap,R cap|SkirtRoot" \
  --eyes "Eye_L,Eye_R" --exclude Assets/Bhaptics --exclude Assets/Avatar/DynamicBone \
  --vpm-package com.poiyomi.toon-9.3.64.zip --relink-locked-shaders [--dry-run] [--json]
```

## What is migrated

| SDK2 | SDK3 | How |
|------|------|-----|
| `VRC_AvatarDescriptor` (MonoBehaviour, `VRCSDK2.dll`) | `VRCAvatarDescriptor` (`VRCSDK3A.dll`) | Retyped **at the same fileID** so its slot in the root's `m_Component` list is untouched. Carried: `ViewPosition`, `ScaleIPD`, `lipSync`, `VisemeSkinnedMesh`, `VisemeBlendShapes`, `MouthOpenBlendShapeName`, portrait offsets. Set: `customizeAnimationLayers` with all playable layers on SDK defaults except **FX** (generated, below); `expressionsMenu`/`expressionParameters` (generated); eye look (below); contact colliders `state: 0` (Automatic — the SDK computes them). |
| `PipelineManager` (`VRCCore-Editor.dll`) | `PipelineManager` (`VRCCore-Standalone.dll`) | Retyped in place; `blueprintId` cleared (an SDK3 upload is a new avatar). Added if missing. |
| root `Animator` | same | `m_ApplyRootMotion: 0`, `m_Controller: {fileID: 0}` — SDK3's playable layers drive the animator; root motion left on is the classic "avatar drifts / spins" bug. |
| `DynamicBone` | `VRCPhysBone` | Retyped in place with the SDK's own conversion (next section). |
| `DynamicBoneCollider` | `VRCPhysBoneCollider` | Retyped in place: capsule if `height > 2·radius` else sphere; `m_Direction` X → 90° about local Z, Z → 90° about local X; `insideBounds` from `m_Bound`; `bonesAsSpheres: 1`. Because fileIDs are kept, converted PhysBones' `colliders` lists resolve without rewriting. |
| Unity `Cloth` (`--drop-cloth`) | — | Removed; the mesh falls back to its skinning. |
| Unity `CapsuleCollider` (`--capsules-to-physbone-colliders`) | `VRCPhysBoneCollider` (capsule) | Retyped in place (class 136 → 114). One that shares a GameObject with a DynamicBoneCollider is removed instead (the DBC conversion covers it). |
| — (`--physbone ROOT|IGNORE…|COLLIDERS…|GROUP`) | new `VRCPhysBone` | The chain roots are `ROOT`'s remaining **bone-only** children (children carrying components — collider holders, props — are never simulated: left in place when grouping, auto-ignored otherwise). With `GROUP`, a new empty child of `ROOT` named so is created, the chain roots are re-parented under it (identity local pose, so nothing moves; skinning references bones by fileID, so it is unaffected) and the PhysBone is rooted **there** with no ignore list. Without it, the PhysBone sits on `ROOT` (`multiChildType: Ignore`) with `ignoreTransforms` = the ignored + component-bearing children — but if any collider lives under `ROOT` (leg capsules under `Hips`), VRChat's PhysBone scheduler reports a **cyclic dependency**, so the tool warns and you should use `GROUP`. `colliders` = the named objects' converted colliders (default: every converted capsule). Skirt-ish defaults: pull 0.25, spring 0.5, stiffness 0.15, gravity 0.03 (falloff 0.5), immobile 0.3 (world), radius 0.03, Angle limit 55°, no grab/pose. |
| `--strip NAME` subtrees | — | Every GameObject/Transform/component under it removed and the parent's `m_Children` entry dropped (a haptics vest with cameras and its own Animator, say). |
| `CustomStandingAnims` gesture slots | FX layer | See *FX*, below. |
| `CustomStandingAnims` locomotion / emote slots, `CustomSittingAnims` | — | **Reported, not migrated.** SDK3 Base/Action/Sitting layers are a different design (blend trees, root-motion-free locomotion) and an SDK2 idle/walk/prone clip dropped into them is the source of the drift bugs the migration exists to remove. |

Anything else in the prefab (renderers, meshes, materials, non-VRChat scripts, transforms) is
untouched — byte for byte — because the rewrite is span-splicing over the raw text
([`unity-yaml-edit.md`](unity-yaml-edit.md)). The FBX and its `.meta` (humanoid map, T-pose) are
copied unchanged.

## DynamicBone → PhysBone: the SDK's rules

`PhysBoneSpec::from_dynamic_bone` reproduces
`VRC.SDK3.Dynamics.PhysBone.PhysBoneMigration.Convert` (SDK 3.10.4, disassembled from
`VRC.SDK3.Dynamics.PhysBone.dll` — the conversion the SDK's "Auto Fix" runs on import):

| PhysBone field | From |
|---|---|
| `version` | 0 (Version_1_0), `integrationType` 1 (Advanced), `isAnimated` 1 |
| `rootTransform`, `ignoreTransforms` | `m_Root`, `m_Exclusions` |
| `multiChildType` | 0 (Ignore); `immobileType` 1 (World) |
| `pull` | `m_Elasticity` |
| `spring` | `1 − m_Damping` |
| `stiffness` | 0 |
| `immobile` | `m_Inert` |
| `radius` | `m_Radius × |lossyScale.x(component)| / |lossyScale.x(root)|` |
| `limitType` / `maxAngleX` | `m_FreezeAxis` set → Hinge on that axis; else **Angle** with `maxAngleX = StiffToMaxAngle(m_Stiffness)` from the SDK's table `0→180, 0.1→129, 0.2→106, 0.3→89, 0.4→74, 0.5→60, 0.6→47, 0.7→35, 0.8→23, 0.9→11, 1→0` (linearly interpolated here; the SDK smooths tangents through the same points) |
| `gravity`, `gravityFalloff` | whichever of `m_Gravity.y` / `m_Force.y` is larger in magnitude, as `−g · |lossyScale.x| / max(1e−5, average world bone length of the chain)`; falloff 1 if it came from `m_Gravity` |

Distribution curves (`m_*Distrib`) are not carried (rare on avatars); `m_EndLength` is reported.

## Script references

Every SDK3 component is a class inside a DLL, so `m_Script` is
`{fileID: <class hash>, guid: <dll guid>, type: 3}`. The hash is Unity's
`script_file_id(namespace, class)` = first four bytes of MD4 over `"s\0\0\0" + namespace + class`
(`avatar_unity_yaml::script_file_id`, test-pinned against the SDK's own serialized assets); the
DLL GUIDs were read off `com.vrchat.avatars` / `com.vrchat.base` **3.10.4**:

| Class | fileID | DLL guid |
|---|---|---|
| `VRC.SDK3.Avatars.Components.VRCAvatarDescriptor` | `542108242` | `67cc4cb7839cd3741b63733d5adf0442` (`VRCSDK3A.dll`) |
| `…ScriptableObjects.VRCExpressionParameters` / `VRCExpressionsMenu` | `-1506855854` / `-340790334` | same |
| `VRC.SDK3.Dynamics.PhysBone.Components.VRCPhysBone` / `VRCPhysBoneCollider` | `1661641543` / `-1631200402` | `2a2c05204084d904aa4945ccff20d8e5` |
| `VRC.SDK3.Dynamics.Contact.Components.VRCContactReceiver` | `-1450912254` | `80f1b8067b0760e4bb45023bc2e9de66` |
| `VRC.Core.PipelineManager` | `-1427037861` | `b0e1c0f72d838fe49bfe88b987a471bd` (`VRCCore-Standalone.dll`) |

Field layouts come from the same packages' sample scenes/assets, so a body is what Unity would
write itself (missing newer fields default on load).

## FX from gesture overrides

SDK2's `CustomStandingAnims` is an `AnimatorOverrideController` swapping clips of the SDK's
`Male_Standing_Pose.fbx` template. Slot names are resolved through the template's `.meta`
`fileIDToRecycleName` when the SDK examples were exported with the avatar, else the fixed
`SDK2_TEMPLATE_SLOTS` table (the template's fileIDs never changed). Gesture slots map to SDK3
`GestureLeft`/`GestureRight` values (`FIST` 1, `HANDOPEN` 2, `FINGERPOINT` 3, `VICTORY` 4,
`ROCKNROLL` 5, `HANDGUN` 6, `THUMBSUP` 7).

For each gesture the override `.anim` is read and **only its blendshape curves are lifted** — SDK2
gesture clips also carried finger-muscle curves, and in SDK3 hand poses are the Gesture layer's
job (muscle curves in FX would fight it) — into a clean `Gesture_<Name>.anim` holding each shape at
the source clip's final value. A `Gesture_Neutral.anim` writes every touched shape back to 0. The
controller (`FX.controller`) has **one either-hand layer** (`avatar_anim_gen::gesture`): one
Any-State `Equals` transition per gesture *per parameter* and a Neutral requiring both parameters
to be 0 — exactly SDK2's semantics (an override fired for whichever hand made the gesture) and
immune to the two-layer Write-Defaults-off clobber. Empty `Parameters.asset` / `Menu.asset` are
generated alongside so toggles can be added later (`avatar toggle`).

By default the layer is **analog** (`GestureLayer::analog()`, [anim-gen.md](anim-gen.md)): each
gesture gets a per-hand state (`Fist L` / `Fist R`) whose motion is a 1D BlendTree on that hand's
`GestureLeftWeight`/`GestureRightWeight`, blending `Gesture_Neutral` (0) → the gesture clip (1) —
SDK2's Vive "advanced controls", where trigger depth is expression depth. Transitions are
mutually exclusive and weight-gated (see anim-gen.md): the actively-squeezing hand owns the face,
the right hand wins when both squeeze **different** gestures, and a wand thumb resting on the
touchpad (Fist at weight 0) can never mask or oscillate against the other hand. Both hands on
the **same** gesture route to its `LR` capped-sum state (2D tree over both weights,
`min(left + right, 1)`); the migration emits a half-strength companion clip per gesture
(`Gesture_<Name>_Half.anim`, every shape at 50 %) as the tree's midpoint samples. `--no-analog-gestures` emits discrete
states instead (one static state per gesture, right-priority). Note the platform trade: on
Index-style controllers only Fist's weight tracks an analog axis, so with analog gestures the
other expressions need the trigger held — exactly how the SDK2 avatar behaved on wands.

## Eye look and blink

SDK3 eye look stores the **local rotation of each eye bone** per look state. It is derived
geometrically: `local_state = (R_parent⁻¹ · R_delta · R_parent) · local_rest`, `R_delta` a fixed
turn about the *avatar-space* X (up/down) or Y (left/right) axis — so it is right even when the
eye bones' own axes are wildly rolled (ripped/MMD rigs). Default angles 10° up/down, 12° left/right
(`--eye-angles U,D,L,R`). The blink blendshape is found on the viseme mesh's FBX by name (`Blink`,
`blink`, `Blink_Both`, `vrc.blink`, `まばたき`, or `--blink NAME`) and its **import-order index**
written into `eyelidsBlendshapes` (`eyelidType: 2`).

## Bundled packages and locked-shader relink

`--vpm-package PATH` (repeatable) bundles a VPM package — a directory with `package.json`, or a
`.zip` of one, e.g. a shader package's GitHub release — into `<out>/Packages/<name>/` (Unity treats
it as an embedded package) and records it in `vpm-manifest.json`. Its `legacyFolders` (what VCC
deletes on install, e.g. `Assets/_PoiyomiShaders`) are excluded from the asset copy, and any source
asset whose GUID the package already provides is skipped (`assets_deduped` in the report), so the
project opens without duplicate-GUID reassignments.

`--relink-locked-shaders` handles the export-time reality that shader lockers (Poiyomi/Thry's
optimizer, Kaj's) leave behind: each material points at a generated, per-material `Hidden/…`
shader copy whose `#include`s were never exported, and remembers the real one in
`stringTagMap.OriginalShader`. The relink finds a shader whose `Shader "<name>"` matches that tag
(exactly, else ignoring bullets/whitespace — `.poiyomi/• Poiyomi Toon •` ↔ `.poiyomi/Poiyomi
Toon`) among the source assets **and bundled packages**, re-points `m_Shader`, sets the locker's
`_ShaderOptimizerEnabled` float to 0, and excludes the generated `OptimizedShaders/<folder>` copy
from the copy. Property values are kept (lockers keep property names; the shader's own upgrade
pass runs on first inspection). Materials whose original shader can't be found are reported and
left alone; materials still on a shader with unresolvable includes are warned about.

## Output layout

```
<out>/Assets/<copied source assets, minus --exclude and the SDK2 VRCSDK/, VRChat Examples/>
<out>/Assets/<Name>_SDK3/<Name>.prefab (+ .meta)         # the migrated prefab
<out>/Assets/<Name>_SDK3/FX/FX.controller, Gesture_*.anim (+ .meta)
<out>/Assets/<Name>_SDK3/Parameters.asset, Menu.asset (+ .meta)
<out>/Packages/<bundled package>/…                        # each --vpm-package
<out>/Packages/vpm-manifest.json                          # com.vrchat.avatars/base <sdk_version> + bundled
<out>/Packages/manifest.json                              # VCC-template Unity packages incl. com.unity.test-framework
<out>/ProjectSettings/ProjectVersion.txt (<unity_version>)
```

The output must not already contain `Assets/` (refuse-before-write); `--dry-run` plans and reports
without touching the filesystem. Generated GUIDs/fileIDs are deterministic (seeded from names).

## What the report tells you to do in Unity

- Open `<out>` in the Creator Companion (*Add Existing Project*) so it resolves the SDK, then in
  Unity drag the prefab into a scene, check the descriptor (view position, FX layer), *Build &
  Publish*.
- Materials whose shader source has an unresolvable `#include` (locked/optimized shaders exported
  without their `.cginc`s) are flagged: they render pink until the shader package is installed and
  the materials re-pointed at it — which `--vpm-package` + `--relink-locked-shaders` does offline.
- `Packages/manifest.json` must carry `com.unity.test-framework`: the SDK's editor assembly ships
  NUnit tests and a project without it fails to compile (`NUnit could not be found`, then a Burst
  `VRC.ExampleCentral.Editor` resolution cascade).
- Play-test PhysBones, confirm the eye-look preview, test each gesture. Retuning is a one-line
  edit on the migrated prefab — `avatar physbone set|split|stretch` ([`physbone.md`](physbone.md)):
  values + per-chain curves, chains split onto their own components, chains lengthened.

## Limits

- Locomotion/emote/sitting overrides are not migrated (by design, above); rebuild them on SDK3
  layers if wanted.
- DynamicBone distribution curves and `m_EndLength` are not carried; endpoint set from
  `m_EndOffset` only.
- Prefabs only (one root Transform); nested prefab instances (`stripped` documents) are not
  resolved.
- The Unity import itself is not verified here (no editor in this toolchain); `avatar lint` /
  `avatar stats` on the output are the offline checks.
