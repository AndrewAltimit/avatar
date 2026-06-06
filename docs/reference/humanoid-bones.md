# Unity Humanoid bones & VRChat rig requirements

Reference for the bone mapping `avatar-armature` infers and validates. Sourced from the
[VRChat Rig Requirements](https://creators.vrchat.com/avatars/rig-requirements/) and Unity's
`HumanBodyBones`.

## Required by VRChat (humanoid)

VRChat requires these to be mapped for a humanoid avatar:

- **Spine chain**: Hips, Spine, Chest, Neck — and **Shoulders** — must all be present/mapped.
- **Head**: Head.
- **Hands**: LeftHand, RightHand (plus the lower/upper arm chain feeding them).
- **Feet**: LeftFoot, RightFoot (plus the leg chain).

Fingers, eyes, jaw, toes, and the upper-chest are **optional** but enable extra features (hand
tracking detail, simulated eye look, visemes/jaw).

## Notes that drive validation rules

- **Eye bones point up.** SDK3 expects eye bones oriented upward, not outward.
- **T-pose** is expected at import for correct muscle/retarget setup.
- A clean single skeleton root (no stray extra roots) is expected.

## Common naming conventions we infer from

Bone-name → Humanoid mapping must tolerate many rig exporters. Examples to map to `Hips`:
`Hips`, `hip`, `pelvis`, `mixamorig:Hips`, `Bip01_Pelvis`, `J_Bip_C_Hips` (VRoid), etc. The
inference table lives in `avatar-armature` and is data-driven so it can grow.

> TODO: expand into the full `HumanBodyBones` enum table with per-bone synonym lists and
> required/optional flags as the inference table in `avatar-armature` matures.
