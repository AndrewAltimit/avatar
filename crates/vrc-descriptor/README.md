# avatar-vrc-descriptor

Typed extraction of VRChat SDK3 avatar data assets. Package `avatar-vrc-descriptor` · library
`avatar_vrc_descriptor`. Part of the [avatar](../../README.md) monorepo.

## What it does

Extracts the SDK3 assets that carry the most well-defined rules:

- **Expression Parameters** — the 256-bit sync budget (Bool = 1 bit, Int/Float = 8; only
  `networkSynced` params count).
- **Expression Menus** — the 8-control limit and sub-menu nesting.
- **VRC Avatar Descriptor** — view position, viseme lip-sync, eye-look config, expression refs, and
  playable layers, parsed from `.prefab`/`.unity` files.

VRChat assets are identified **structurally** (by the shape of their serialized fields) rather than
by a hardcoded script GUID, so this keeps working across SDK versions. The `m_Script` GUID is still
captured for reference.

## Key API

- `VrcAsset` — the classifier result: `Parameters(ExpressionParameters)`, `Menu(ExpressionsMenu)`,
  or `Descriptor(Box<AvatarDescriptor>)`.
- `ExpressionParameters`, `ExpressionsMenu`, `AvatarDescriptor` (+ `EyeLookSettings`,
  `AnimationLayer`, `AssetRef`) — the typed assets.
- `ValueType` and the SDK3 rule constants (sync-budget sizes, viseme count, lip-sync/eyelid modes,
  animation-layer types).

## Status

Built: **M2** (Expression Parameters/Menus, Avatar Descriptor incl. eye-look + parameter wiring).

## See also

- [`docs/reference/sdk3-lint-rules.md`](../../docs/reference/sdk3-lint-rules.md) — the `VRC01x`–`VRC03x`
  rules built on these types.
