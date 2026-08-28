# Testing: the fixture corpus and golden harness

How the workspace pins behaviour against regressions. Two pieces work together: a **fixture
corpus** (the assets under test) and the **golden harness** in [`avatar-testkit`](../../crates/testkit)
(the machinery that snapshots a report and diffs it).

The motivation: most read paths produce a *report* (a `LintReport`, a `PerfReport`, an
`ArmatureReport`) that agents consume as JSON. Assertion tests check the handful of fields someone
thought to assert; a golden test pins the **whole serialized report**, so any change — a new field,
a reworded diagnostic, a shifted count, a reordered list — surfaces as a reviewable diff instead of
passing silently. That is the regression net that lets you trust a report on an asset you didn't
hand-check.

## The corpus

Three layers, by design (see [`fixtures/README.md`](../../fixtures/README.md)):

1. **Committed synthetic Unity projects** — `fixtures/projects/{Sample,Avatar,Dynamics}Project`.
   Hand-authored to exercise specific lint rules and component-stats paths. Hermetic: run on any
   machine. Resolved in tests via `avatar_testkit::corpus("projects/SampleProject")`.
2. **In-code synthetic FBX** — `avatar_testkit::fbx::humanoid_skeleton()` builds a deterministic
   binary FBX in memory via the `fbxcel` writer (feature `fbx`). No committed `.fbx` blob; follows
   the workspace rule that user FBX is never committed. Covers the armature + geometry-stats paths.
3. **Env-gated real assets** — `AVATAR_SAMPLE_FBX`, `AVATAR_SAMPLE_UNITYPACKAGE`,
   `AVATAR_SAMPLE_UNITYPACKAGE_WORLD`. Tests self-skip when the path is absent; on the self-hosted
   CI runner (the dev machine) they point at real files, so the real-data paths run every push. See
   [`CONTRIBUTING.md`](../../CONTRIBUTING.md).

Layers 1–2 are hermetic and run everywhere (including forks); layer 3 is the ground-truth pass.

## The harness — `avatar-testkit`

A `publish = false` workspace member. Added as a `dev-dependency` by the crates that golden-test.

- `golden::assert_json(path, &value)` — serialize `value` to canonical pretty JSON (2-space indent,
  trailing newline) and compare against the file at `path` (resolved relative to the consuming
  crate). On mismatch it panics with a line-located diff; a missing file tells you to regenerate.
- `golden::redact_roots(&mut value)` / `golden::redact(&mut value, from, to)` — scrub
  machine-specific absolute paths out of a serialized report (`project_root`, a report's `source`)
  before snapshotting, replacing the workspace-root prefix with `<ROOT>` so snapshots are identical
  on every machine.
- `corpus(rel)` / `workspace_root()` — resolve a corpus path from any crate's tests, at runtime.
- `fbx::humanoid_skeleton()` (feature `fbx`) — the in-code synthetic FBX.

Snapshots live beside the consuming test, under `crates/<crate>/tests/golden/*.json`, and are
committed. Lists that have no guaranteed order (lint diagnostics, the per-avatar `PerfReport` vec)
are sorted in the test before snapshotting so the golden is stable.

### Writing a golden test

```rust
use avatar_testkit::{corpus, golden};

#[test]
fn golden_my_project() {
    let report = avatar_lint::run(&corpus("projects/SampleProject")).unwrap();
    let mut value = serde_json::to_value(&report).unwrap();
    golden::redact_roots(&mut value);
    golden::assert_json("tests/golden/SampleProject.lint.json", &value);
}
```

### Updating snapshots

After an **intentional** change to a report shape or a fixture, regenerate and review the diff
before committing — the diff *is* the change-review:

```sh
UPDATE_GOLDEN=1 cargo test --workspace      # rewrite every snapshot
git diff -- '**/tests/golden/**'            # review, then commit
```

`UPDATE_GOLDEN` is honored for any non-empty, non-`0` value. With it unset, a mismatch fails the
test — which is the point.

## Current golden coverage

| Crate | Snapshot | Covers |
|-------|----------|--------|
| `avatar-lint` | `{Sample,Avatar,Dynamics}Project.lint.json` | the full `LintReport` per corpus project |
| `avatar-stats` | `{Sample,Avatar,Dynamics}Project.project-stats.json` | per-avatar `PerfReport`s (component side) |
| `avatar-stats` | `humanoid_skeleton.fbx-stats.json` | FBX geometry `PerfReport` |
| `avatar-armature` | `humanoid_skeleton.armature.json` | the full `ArmatureReport` (humanoid mapping) |
| `avatar-migrate` | `Sdk2Project.migrate.json`, `Sdk2Project.migrated.prefab.txt`, `Sdk2Project.FX.controller.txt`, `Sdk2Project.physbones.json`, `Sdk2Project.physbones.tuned.json` | the full `MigrationReport`, the rewritten prefab text, the generated FX controller for the synthetic SDK2 fixture, and the `avatar physbone list` of the migrated prefab before / after a split + set (curves) + stretch pass |

To extend coverage, drop a fixture into the corpus (or add an `avatar-testkit::fbx` builder) and add
a golden test that runs the analysis over it.
