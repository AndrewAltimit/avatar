# avatar-lint

The SDK3 diagnostics engine. Package `avatar-lint` · library `avatar_lint`. Part of the
[avatar](../../README.md) monorepo.

## What it does

Discovers a Unity/VPM project, scans its VRChat assets, and reports SDK3 compliance problems:
expression-parameter sync budget and duplicates, menu size and dangling parameter references, the
Avatar Descriptor (expression/playable-layer reference resolution via a guid→path `.meta` index,
visemes, eye-look, parameter↔animator wiring), and animator-controller contents — plus project/SDK
info.

Rules are intentionally conservative: **errors** are things VRChat will reject or that break the
avatar; **warnings** are likely-but-not-certain problems (e.g. a menu referencing a parameter we
can't find a declaration for, which another tool may generate at build time).

## Key API

- `run(path) -> Result<LintReport>` — discover + lint a project.
- `LintReport` — counts + `diagnostics`, with `error_count()` / `warn_count()`.
- `Diagnostic` (code, `Severity`, message, file, hint) and `Severity` (Error / Warn / Info).

It composes [`avatar-vpm`](../vpm/README.md), [`avatar-vrc-descriptor`](../vrc-descriptor/README.md),
and [`avatar-unity-asset`](../unity-asset/README.md) over
[`avatar-unity-yaml`](../unity-yaml/README.md).

## Status

Built: **M2**.

## See also

- [`docs/reference/sdk3-lint-rules.md`](../../docs/reference/sdk3-lint-rules.md) — the full rule
  table (`VRC001`–`VRC052`) and how assets are identified.
