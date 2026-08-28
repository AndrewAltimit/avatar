# Unity Humanoid bones & VRChat rig requirements

Reference for the bone mapping `avatar-armature` infers and validates. Sourced from the
[VRChat Rig Requirements](https://creators.vrchat.com/avatars/rig-requirements/) and Unity's
`HumanBodyBones`; the table of record is `crates/armature/src/humanoid.rs` (`HumanBone::ALL`,
25 slots, each with a `Requirement`).

## Required (Unity humanoid)

Fifteen bones are **Required** — Unity will not import the rig as humanoid without them, and a
missing one makes `avatar armature check` exit non-zero:

- **Hips, Spine, Head**.
- **Arm chain**: LeftUpperArm, LeftLowerArm, LeftHand (and the right side).
- **Leg chain**: LeftUpperLeg, LeftLowerLeg, LeftFoot (and the right side).

## Recommended

**Chest, Neck, and the Shoulders** are **Recommended**, not required: VRChat's guidance is to map
them for a well-behaved spine and arm rig, but a rig without them still imports as humanoid.
`armature check` reports them as missing-recommended without failing.

## Optional

UpperChest, toes, eyes, and jaw are **Optional** but enable extra features (simulated eye look,
visemes/jaw, toe articulation).

**Fingers have no slots** in the 25-bone table at all: finger bones are recognized by the name
classifier (`thumb`/`index`/`middle`/… tokens) precisely so they can be *excluded* from body
mapping — a `LeftHandMiddle1` must never be mistaken for the Hand.

## Structural check, and import guidance

Of the rig expectations, only one drives a validation rule here: a clean **single armature root**
— `armature check` warns when the skeleton has no bone-bearing root or more than one.

The rest is import guidance (nothing in `armature check` validates it):

- **Eye bones point up.** SDK3 expects eye bones oriented upward, not outward.
- **T-pose** is expected at import for correct muscle/retarget setup.

## Common naming conventions we infer from

Bone-name → Humanoid mapping must tolerate many rig exporters. Examples to map to `Hips`:
`Hips`, `hip`, `pelvis`, `mixamorig:Hips`, `Bip01_Pelvis`, `J_Bip_C_Hips` (VRoid), etc. The
inference table lives in `avatar-armature` (`humanoid.rs`) and is data-driven — per-bone synonym
tokens plus the requirement classification above — so it can grow.
