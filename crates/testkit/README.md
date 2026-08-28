# avatar-testkit

Shared **test-only** machinery for the workspace: a golden-file ("snapshot") harness plus the
in-code synthetic-asset builders that feed it. `publish = false`; pulled in as a `dev-dependency` by
the crates that golden-test. Nothing here is part of the shipped product.

## Purpose

Most read paths produce a *report* (`LintReport`, `PerfReport`, `ArmatureReport`) that agents consume
as JSON. Assertion tests check the few fields someone thought to assert; a **golden test** pins the
*whole* serialized report and diffs it against a committed snapshot, so any change — a new field, a
reworded diagnostic, a shifted count, a reordered list — surfaces as a reviewable diff instead of
passing silently. That is the regression net for "do I trust this report on an asset I didn't
hand-check."

## Key API

- `golden::assert_json(path, &value)` — serialize `value` to canonical pretty JSON (2-space indent,
  trailing newline) and compare against the file at `path` (resolved relative to the consuming
  crate). Mismatch → panic with a line-located diff; missing file → "regenerate with `UPDATE_GOLDEN=1`".
- `golden::redact_roots(&mut value)` / `golden::redact(&mut value, from, to)` — scrub
  machine-specific absolute paths (`project_root`, a report's `source`) out of a serialized report
  before snapshotting, replacing the workspace-root prefix with `<ROOT>`.
- `golden::update_enabled()` — true when `UPDATE_GOLDEN` is set (non-empty, non-`0`).
- `corpus(rel)` / `workspace_root()` — resolve a path in the shared `fixtures/` corpus from any
  crate's tests, at runtime.
- `fbx::humanoid_skeleton() -> Vec<u8>` (feature `fbx`) — bytes of a deterministic, humanoid-ready
  Mixamo-style skeleton FBX, built in memory via the `fbxcel` writer. No committed `.fbx` blob.

## Updating snapshots

After an intentional change, regenerate and review the diff before committing:

```sh
UPDATE_GOLDEN=1 cargo test --workspace
git diff -- '**/tests/golden/**'
```

## Status

Built. Used by the `avatar-lint`, `avatar-stats`, `avatar-armature`, `avatar-unity-yaml`,
`avatar-migrate`, and `avatar-vpm` golden tests, and by `avatar-web-analyzer`'s report-shape test. Core deps
`anyhow` + `serde` + `serde_json`; the optional `fbx` feature adds `fbxcel`. Corpus + workflow:
[`docs/reference/testing.md`](../../docs/reference/testing.md), [`fixtures/README.md`](../../fixtures/README.md).
