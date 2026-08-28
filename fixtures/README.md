# Fixture corpus

The shared, committed test corpus for the workspace. Centralizing it here (rather than scattering
per-crate `tests/fixtures/` trees) is what lets several crates golden-test the *same* assets and
makes the corpus browsable in one place.

## Layout

| Path | What it is | Consumed by |
|------|-----------|-------------|
| `projects/SampleProject/` | Synthetic Unity project: duplicate param, oversized menu, dangling menu ref. | `avatar-lint`, `avatar-stats` golden + assertion tests |
| `projects/AvatarProject/` | Synthetic project with a VRC Avatar Descriptor prefab (missing FX controller, viseme/eye-look issues, parameter-wiring mismatches). | same |
| `projects/DynamicsProject/` | Synthetic project exercising the PhysBone / Avatar-Dynamics rules (VRC050–052), avatar-level Write-Defaults (VRC045), viseme entries (VRC038). | same |
| `projects/ClipProject/` | Synthetic project exercising the animation-clip rules (VRC046 missing motion, VRC047 FX transform curves, VRC049 empty clip). | same |
| `projects/QuestProject/` | Synthetic project exercising the hygiene/Android rules (VRC060 missing script, VRC061 non-mobile shader vs a whitelisted `VRChat/Mobile/*` one). | same |
| `projects/Sdk2Project/` | Synthetic **SDK2** avatar project (SDK2 descriptor + PipelineManager, root motion on, DynamicBone (two hair chains: a pigtail + a bang) + collider, Cloth skirt + capsule, a strippable vest subtree with a camera, eye bones, a gesture override controller with a muscle-carrying clip, a material whose shader has a missing `#include`). | `avatar-migrate` golden + assertion tests |

There is deliberately **no committed FBX or `.unitypackage`** here. Two reasons:

1. **User avatars are never committed** (see `.gitignore`). Real-asset coverage runs through the
   env-gated integration tests (`AVATAR_SAMPLE_FBX`, `AVATAR_SAMPLE_UNITYPACKAGE`, …) that self-skip
   when the path is absent — on the dev machine / self-hosted CI runner these point at real files.
2. **Synthetic FBX is built in-code**, not stored as a binary blob. `avatar-testkit`'s `fbx` feature
   (`avatar_testkit::fbx::humanoid_skeleton()`) emits a deterministic binary FBX via the `fbxcel`
   writer, so the FBX read paths get hermetic golden coverage without a committed `.fbx`.

So the corpus is three layers: **committed synthetic projects** (here), **in-code synthetic FBX**
(`avatar-testkit`), and **env-gated real assets** (the dev machine).

## Golden tests

Each report-producing path has a golden ("snapshot") test: it serializes the whole report to
canonical JSON and diffs it against a committed file under the consuming crate's `tests/golden/`.
This catches *any* change to the report surface, not just the fields an assertion happened to check.

After an **intentional** change to a report shape or a fixture, regenerate the snapshots and review
the diff before committing:

```sh
UPDATE_GOLDEN=1 cargo test --workspace
git diff -- '**/tests/golden/**'   # review what changed, then commit
```

The harness lives in `avatar-testkit` (`golden::assert_json`, `golden::redact_roots`,
`corpus(...)`). See `docs/reference/testing.md`.

## Adding a fixture

1. Drop the asset under `projects/` (or add an in-code builder to `avatar-testkit::fbx`).
2. Add a golden test that runs the relevant analysis over it and calls `golden::assert_json`.
3. `UPDATE_GOLDEN=1 cargo test -p <crate>` to write the snapshot; review and commit it.
