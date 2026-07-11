# SDK3 lint rules

Rules emitted by `avatar lint <project>` (the `avatar-lint` crate). Errors are things VRChat
will reject or that break the avatar; warnings are likely-but-not-certain problems.

Codes are grouped: `VRC00x` project, `VRC01x` parameters, `VRC02x` menus, `VRC03x` Avatar
Descriptor, `VRC04x` animator controllers + animation clips, `VRC05x` PhysBones / Avatar Dynamics.

| Code | Severity | Rule | Source |
|------|----------|------|--------|
| `VRC001` | warn | VRChat avatar SDK (`com.vrchat.avatars`) not found in `vpm-manifest.json` | [VPM](https://vcc.docs.vrchat.com/vpm/) |
| `VRC002` | info | No VRC Avatar Descriptor found in any scene/prefab under `Assets/` (only emitted when the avatar SDK is present) | [descriptor](https://creators.vrchat.com/avatars/) |
| `VRC010` | error | Synced expression parameters exceed the **256-bit** budget (Bool = 1 bit, Int/Float = 8 bits; only `networkSynced` params count) | [budget](https://feedback.vrchat.com/avatar-30/p/1332-bug-vrcexpressionparameters-fail-to-load-correctly-with-more-than-256-param) |
| `VRC011` | warn | Duplicate expression parameter name within one asset | — |
| `VRC012` | info | An Expression Parameter is referenced by **no** menu control, animator condition, or blend tree **anywhere in the project** — a project-wide superset of `VRC036`. Advisory only (high false-positive rate: a name can be supplied by OSC/contacts/Modular Avatar/VRCFury). Built-ins and default-layer params (`VRCEmote`, `VRCFaceBlendH/V`) are excluded | — |
| `VRC020` | error | Expression menu has more than **8** controls | [menus](https://creators.vrchat.com/avatars/expression-menu-and-controls/) |
| `VRC021` | warn | Menu control references a parameter not declared in any Expression Parameters asset (and not a [built-in](https://creators.vrchat.com/avatars/animator-parameters/)). May be generated at build time (e.g. Modular Avatar) | — |
| `VRC022` | warn | A menu control drives nothing — no `parameter`, no `subParameters`, and no sub-menu | [menus](https://creators.vrchat.com/avatars/expression-menu-and-controls/) |
| `VRC030` | warn | Descriptor uses custom expressions but its **Expression Parameters** reference is unassigned, points at a missing asset (guid not in the project), or resolves to something that isn't an Expression Parameters asset | — |
| `VRC031` | warn | Same as `VRC030`, for the descriptor's **Expression Menu** reference | — |
| `VRC032` | warn | A custom (non-default) playable layer references an animator controller whose guid is not present in the project | [playable layers](https://creators.vrchat.com/avatars/playable-layers/) |
| `VRC033` | warn | Lip-sync is set to Viseme Blend Shape but the viseme mesh is unassigned, or the viseme blend-shape count is not 15 | [lip sync](https://creators.vrchat.com/avatars/avatar-descriptor/#lipsync) |
| `VRC034` | warn | Eye Look is enabled but no eye bones are assigned | [eye look](https://creators.vrchat.com/avatars/avatar-descriptor/#eye-look) |
| `VRC035` | warn | Eyelid Type is Blendshapes but no eyelid skinned mesh is assigned | [eye look](https://creators.vrchat.com/avatars/avatar-descriptor/#eye-look) |
| `VRC036` | warn | An Expression Parameter is used by none of the avatar's (resolvable, non-default) playable-layer animator controllers — likely a forgotten wiring, unless driven by OSC/contacts/Modular Avatar/VRCFury. Default-layer params (`VRCEmote`, `VRCFaceBlendH/V`) and built-ins are excluded | — |
| `VRC037` | warn | An Expression Parameter shares a name with an animator parameter but has an incompatible type (Int↔Int, Float↔Float, Bool↔Bool) | — |
| `VRC038` | warn | When lip-sync uses viseme blend shapes and the entry count is correct (15), a viseme entry is empty/`-none-`, or two entries name the same blend shape — complements `VRC033` (which only checks the mesh + count) | [lip sync](https://creators.vrchat.com/avatars/avatar-descriptor/#lipsync) |
| `VRC040` | warn | A transition condition references an animator parameter not declared in that controller (Unity requires the parameter to exist, so the transition silently never fires) | [animator](https://docs.unity3d.com/Manual/class-AnimatorController.html) |
| `VRC041` | warn | A blend tree reads an animator parameter not declared in that controller (respects blend type: 1D reads X, 2D reads X+Y, Direct reads each child's direct parameter) | [blend trees](https://docs.unity3d.com/Manual/class-BlendTree.html) |
| `VRC042` | warn | A state machine has child states but no default state set (it never enters any state) | [animator](https://docs.unity3d.com/Manual/class-AnimatorController.html) |
| `VRC043` | warn | Duplicate animator parameter name within a controller | — |
| `VRC044` | warn | States in **one** controller mix Write Defaults on and off — a common cause of broken/sticky VRChat animations | [write defaults](https://creators.vrchat.com/avatars/best-practices/migrating-existing-avatars-to-write-defaults-off/) |
| `VRC045` | warn | Write Defaults is inconsistent **across** the avatar's resolvable, non-default playable-layer controllers (e.g. one layer all-on, another all-off) — the avatar-level counterpart to `VRC044`. Needs ≥2 resolvable controllers | [write defaults](https://creators.vrchat.com/avatars/best-practices/migrating-existing-avatars-to-write-defaults-off/) |
| `VRC046` | warn | A state's `m_Motion` (or a blend-tree child's) references a motion by a guid not present in the project — the clip was moved or deleted, so the state silently plays nothing | — |
| `VRC047` | warn | A clip played by the avatar's **FX** playable layer animates transform (position/rotation/scale) or humanoid-muscle curves; the FX layer is for non-transform animation (blendshapes, toggles, materials). Only standalone `.anim` assets are inspectable; FBX-embedded clips are skipped | [playable layers](https://creators.vrchat.com/avatars/playable-layers/) |
| `VRC048` | info | A state has no Motion assigned at all (plays nothing). Advisory — empty states are a common intentional idiom (e.g. a Write-Defaults-off buffer state) | — |
| `VRC049` | info | An animation clip has **no curves** — a no-op asset, usually an authoring slip | — |
| `VRC050` | warn | A PhysBone's root transform can't be resolved in the file — `rootTransform` (or, when unset, the transform on the PhysBone's own GameObject) doesn't point at a transform present here (e.g. stripped from a nested prefab) | [PhysBones](https://creators.vrchat.com/avatars/avatar-dynamics/physbones/) |
| `VRC051` | warn | A PhysBone's root resolves but it moves **zero** transforms (no child bones under the root and no endpoint) — it simulates nothing | [PhysBones](https://creators.vrchat.com/avatars/avatar-dynamics/physbones/) |
| `VRC052` | warn | A PhysBone's `colliders` list has slots but **every** slot is a null reference. A genuinely empty `colliders: []` is fine | [PhysBone colliders](https://creators.vrchat.com/avatars/avatar-dynamics/physbones/#colliders) |

## How assets are identified

VRChat assets are recognized **structurally** (an Expression Parameters asset has a `parameters`
list whose entries carry `valueType`/`name`; an Expression Menu has a `controls` list whose
entries carry `type`/`parameter`; an Avatar Descriptor MonoBehaviour carries `baseAnimationLayers`
+ `ViewPosition`), not by a hardcoded script GUID. This keeps the linter working across SDK
versions. The `m_Script` GUID is still captured for reference.

`.asset` ScriptableObjects, `.prefab`, and `.unity` scenes are all scanned. Cross-asset
references (e.g. a descriptor's `expressionParameters`/`expressionsMenu`/`animatorController`) are
resolved by building a guid→path index from the project's `.meta` files. Note: a Unity guid is 32
hex characters and is parsed as a *string*; a reference with only a local `fileID` (no guid)
cannot be resolved across files and is skipped rather than flagged.

`.controller` files are parsed by `avatar-unity-asset` into a typed `AnimatorController`. A
controller is one Unity-YAML stream of objects linked by local `fileID`s — the controller itself
(class 91), its state machines (1107), states (1102), transitions (1101/1109) and blend trees
(206). The animator rules (`VRC04x`) aggregate the relevant fields across those objects by class
id rather than rebuilding the full graph, which keeps them robust to SDK drift. The parameter
checks are controller-internal (Unity itself requires a referenced parameter to be declared), so
they don't need a built-in allow-list.

## valueType encoding

`valueType` in `VRCExpressionParameters`: `0 = Int`, `1 = Float`, `2 = Bool`.

## Scope / not yet covered

Current scope: Expression Parameters/Menus (`*.asset`); the VRC Avatar Descriptor in
prefabs/scenes (expression + playable-layer references, viseme lip-sync incl. per-entry checks,
eye-look config, a cross-check that expression parameters are actually wired to the avatar's
animator controllers by name and type, and avatar-level Write-Defaults consistency); animator
controllers (`.controller`: parameter references, default states, Write Defaults consistency,
duplicate parameters); **animation clips** (`.anim`: missing/unassigned state motions, FX-layer
transform/muscle curves, empty clips — `VRC046`–`VRC049`); and **PhysBones / Avatar Dynamics** in
prefabs/scenes (`VRC05x`: root resolution, zero-transform PhysBones, unwired collider slots), plus
project/VPM info. Not yet: contacts. See `PLAN.md` for the roadmap.

### PhysBone (Avatar-Dynamics) recognition (`VRC05x`)

PhysBones are recognized **structurally** (a `MonoBehaviour` carrying `endpointPosition` or
`multiChildType`), never by `m_Script` guid — the same test `avatar stats` uses. A PhysBone's root
is its `rootTransform` when set, else the transform on its own GameObject; the affected-transform
count walks the transform hierarchy under that root (descendants minus `ignoreTransforms`, plus one
endpoint per chain tip when `endpointPosition` is non-zero), mirroring the stats estimate. These
checks read the prefab/scene's class-114 PhysBone bodies and class-4 transforms directly (the things
the descriptor/menu/parameter `extract` step discards).

### The expression-parameter ↔ animator cross-check (`VRC036`/`037`)

The descriptor's playable layers point at animator controllers by guid; `VRC036`/`037` resolve
those (skipping `isDefault` layers, which use VRChat's built-in controllers that aren't in the
project) and compare the union of their declared parameters against the avatar's Expression
Parameters. A name with no match anywhere is `VRC036` (forgotten wiring — hedged, since OSC/contacts
or build-time tools can supply it); a name that matches but with a mismatched type is `VRC037`. The
check is skipped entirely when no playable-layer controller resolves (nothing to conclude from).
