# SDK3 lint rules

Rules emitted by `avatar lint <project>` (the `avatar-lint` crate). Errors are things VRChat
will reject or that break the avatar; warnings are likely-but-not-certain problems.

Codes are grouped: `VRC00x` project, `VRC01x` parameters, `VRC02x` menus, `VRC03x` Avatar
Descriptor, `VRC04x` animator controllers.

| Code | Severity | Rule | Source |
|------|----------|------|--------|
| `VRC001` | warn | VRChat avatar SDK (`com.vrchat.avatars`) not found in `vpm-manifest.json` | [VPM](https://vcc.docs.vrchat.com/vpm/) |
| `VRC002` | info | No VRC Avatar Descriptor found in any scene/prefab under `Assets/` (only emitted when the avatar SDK is present) | [descriptor](https://creators.vrchat.com/avatars/) |
| `VRC010` | error | Synced expression parameters exceed the **256-bit** budget (Bool = 1 bit, Int/Float = 8 bits; only `networkSynced` params count) | [budget](https://feedback.vrchat.com/avatar-30/p/1332-bug-vrcexpressionparameters-fail-to-load-correctly-with-more-than-256-param) |
| `VRC011` | warn | Duplicate expression parameter name within one asset | — |
| `VRC020` | error | Expression menu has more than **8** controls | [menus](https://creators.vrchat.com/avatars/expression-menu-and-controls/) |
| `VRC021` | warn | Menu control references a parameter not declared in any Expression Parameters asset (and not a [built-in](https://creators.vrchat.com/avatars/animator-parameters/)). May be generated at build time (e.g. Modular Avatar) | — |
| `VRC030` | warn | Descriptor uses custom expressions but its **Expression Parameters** reference is unassigned, points at a missing asset (guid not in the project), or resolves to something that isn't an Expression Parameters asset | — |
| `VRC031` | warn | Same as `VRC030`, for the descriptor's **Expression Menu** reference | — |
| `VRC032` | warn | A custom (non-default) playable layer references an animator controller whose guid is not present in the project | [playable layers](https://creators.vrchat.com/avatars/playable-layers/) |
| `VRC033` | warn | Lip-sync is set to Viseme Blend Shape but the viseme mesh is unassigned, or the viseme blend-shape count is not 15 | [lip sync](https://creators.vrchat.com/avatars/avatar-descriptor/#lipsync) |
| `VRC034` | warn | Eye Look is enabled but no eye bones are assigned | [eye look](https://creators.vrchat.com/avatars/avatar-descriptor/#eye-look) |
| `VRC035` | warn | Eyelid Type is Blendshapes but no eyelid skinned mesh is assigned | [eye look](https://creators.vrchat.com/avatars/avatar-descriptor/#eye-look) |
| `VRC036` | warn | An Expression Parameter is used by none of the avatar's (resolvable, non-default) playable-layer animator controllers — likely a forgotten wiring, unless driven by OSC/contacts/Modular Avatar/VRCFury. Default-layer params (`VRCEmote`, `VRCFaceBlendH/V`) and built-ins are excluded | — |
| `VRC037` | warn | An Expression Parameter shares a name with an animator parameter but has an incompatible type (Int↔Int, Float↔Float, Bool↔Bool) | — |
| `VRC040` | warn | A transition condition references an animator parameter not declared in that controller (Unity requires the parameter to exist, so the transition silently never fires) | [animator](https://docs.unity3d.com/Manual/class-AnimatorController.html) |
| `VRC041` | warn | A blend tree reads an animator parameter not declared in that controller (respects blend type: 1D reads X, 2D reads X+Y, Direct reads each child's direct parameter) | [blend trees](https://docs.unity3d.com/Manual/class-BlendTree.html) |
| `VRC042` | warn | A state machine has child states but no default state set (it never enters any state) | [animator](https://docs.unity3d.com/Manual/class-AnimatorController.html) |
| `VRC043` | warn | Duplicate animator parameter name within a controller | — |
| `VRC044` | warn | States in one controller mix Write Defaults on and off — a common cause of broken/sticky VRChat animations | [write defaults](https://creators.vrchat.com/avatars/best-practices/migrating-existing-avatars-to-write-defaults-off/) |

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
prefabs/scenes (expression + playable-layer references, viseme lip-sync, eye-look config, and a
cross-check that expression parameters are actually wired to the avatar's animator controllers by
name and type); and animator controllers (`.controller`: parameter references, default states,
Write Defaults consistency, duplicate parameters), plus project/VPM info. Not yet: animation-clip
contents, or PhysBones/contacts. See `PLAN.md` for the roadmap.

### The expression-parameter ↔ animator cross-check (`VRC036`/`037`)

The descriptor's playable layers point at animator controllers by guid; `VRC036`/`037` resolve
those (skipping `isDefault` layers, which use VRChat's built-in controllers that aren't in the
project) and compare the union of their declared parameters against the avatar's Expression
Parameters. A name with no match anywhere is `VRC036` (forgotten wiring — hedged, since OSC/contacts
or build-time tools can supply it); a name that matches but with a mismatched type is `VRC037`. The
check is skipped entirely when no playable-layer controller resolves (nothing to conclude from).
