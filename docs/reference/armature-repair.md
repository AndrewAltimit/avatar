# Armature repair (`avatar armature fix`)

`avatar armature fix <model.fbx>` plans — and, with `-o`, writes — repairs that make a non-standard
humanoid rig import cleanly into Unity. It is implemented by the `repair` module of
`avatar-armature` (planning) over the `FbxDocument` write API of `avatar-fbx` (mutation +
serialization).

```sh
avatar armature fix model.fbx                 # dry run: print the plan
avatar armature fix model.fbx -o fixed.fbx    # apply native repairs and write fixed.fbx
avatar armature fix model.fbx --json          # machine-readable plan
```

The command is a **dry run by default**: without `-o` it only prints the plan. It refuses to
overwrite the input file unless `--force` is given.

## What it repairs

Repairs are tiered by how confidently they can be applied without re-transforming geometry.

| Class | Applied? | What it does |
|-------|----------|--------------|
| **Rename** | natively | Renames each uniquely-mapped bone to its canonical Unity humanoid name (e.g. `mixamorig:LeftArm` → `LeftUpperArm`). This is what makes Unity's humanoid auto-mapper succeed. |
| **Reparent** | **flagged only** | Detects a bone wired onto the wrong humanoid parent (see below) and reports it. Conservative — most clean rigs flag zero reparents. |
| **Normalize** (scale / orientation) | **flagged only** | A non-standard `UnitScaleFactor` or non-Y-up `UpAxis` is *reported* but not changed. |

For a stock Mixamo skeleton (correct hierarchy, non-canonical names) the renames alone are what
Unity's humanoid auto-mapper keys on, so `fix -o` produces a directly importable rig.

### Why renaming is safe

FBX skin clusters and animation curves reference bones by **object id** (through the `Connections`
graph), never by name. `rename_object` rewrites only a `Model`'s `Name\0\1Class` attribute, so
skinning and animation inside the file are untouched — only the human-facing bone name (the one
Unity's avatar mapper reads) changes.

### Why reparenting is flagged, not applied

The mapper already knows which bone occupies each humanoid slot. For each slot, `fix` computes the
*expected* humanoid parent from a fixed topology table (with the standard fallbacks: no shoulder →
the arm hangs off the upper torso; no upper chest → chest/spine; etc.). It reports a reparent
**only** when:

- the expected parent bone is itself present and unambiguous, **and**
- the bone's current parent is either missing, or a *different* mapped humanoid bone (clearly wrong
  wiring) — never an unmapped intermediate (a twist or accessory bone we shouldn't cut through).

But unlike a rename, a reparent is **not auto-applied**. In FBX a bone's world rest position is its
parent's world transform composed with the bone's own *local* transform. Re-pointing only the `OO`
connection leaves that local transform untouched — it was authored against the *old* parent — so the
bone's world rest pose shifts, which breaks the bind pose Unity reads to build the humanoid. A
correct reparent recomposes the local transform against the new parent, including the
`PreRotation` / pivot stack Mixamo/Maya rigs emit. That is a geometry transform (Blender territory,
`PLAN.md` §8), not a metadata relabel, so `fix` surfaces the mis-wiring as a flag and leaves it for a
DCC tool. (The low-level `FbxDocument::reparent_object` primitive exists and is correct as a raw
connection edit — it is simply not wired into `apply_plan` until transform recomposition lands.)

A correctly-built rig (e.g. a stock Mixamo skeleton) flags **no** reparents.

### Why scale / orientation are only flagged

`UnitScaleFactor` and `UpAxis` are *metadata that describes the coordinate data*. Flipping the flag
without re-transforming vertices, bind poses, and keyframes would misrepresent the model (it would
import at the wrong size or lying on its side). A correct fix re-transforms the skinned geometry —
the kind of mutation that belongs in a Blender headless pass (see `PLAN.md` §8), not a metadata
edit. So `fix` surfaces these as flags and leaves them for you to resolve in your DCC tool.

## The FBX writer

`FbxDocument` (in `avatar-fbx`) retains `fbxcel`'s mutable node `Tree`, applies edits by object id,
and serializes back via `fbxcel`'s binary writer (`Writer::new` → `write_tree` → `finalize`). This
resolves the long-standing open risk of native FBX write-back (`PLAN.md` §8). Round-trip fidelity is
covered by non-gated unit tests in `avatar-fbx`; by a non-gated `avatar-armature` integration test
(`tests/synthetic_fix.rs`) that synthesizes a broken Mixamo-style rig as a binary FBX in memory and
runs the full `plan → apply → write → reload` pipeline; and by an `AVATAR_SAMPLE_FBX`-gated test that
applies a full plan to a real Mixamo FBX. All assert the renames persisted, the plan is idempotent,
and the flagged reparent is *not* applied.

**Known characteristic:** the writer re-emits array data (vertices, weights, keyframes)
*uncompressed*, so a written FBX is typically larger than an original that used deflate. This is
semantically identical and accepted by Unity; it is a property of `fbxcel`'s `write_tree`, not a
correctness issue.

## Acceptance: what's proven, and the last mile

Confidence in `armature fix` comes in three layers:

1. **Automated, in-repo (green in CI).** The Rust tests above prove the writer produces a parseable
   binary FBX, that renames persist by object id, that re-planning is idempotent, and that flagged
   edits are never silently applied. The `avatar-cli` exit-code tests (`tests/exit_codes.rs`) drive
   the real binary and assert `armature check` exits non-zero when a required bone is missing — so a
   repaired rig can gate a pipeline. None of this needs Unity.

2. **Automated Unity acceptance (opt-in CI).** The `Unity acceptance` workflow
   (`.github/workflows/unity-acceptance.yml`) runs the *last mile* headlessly: it emits a broken
   Mixamo-named rig (`cargo run -p avatar-fbx --example emit_broken_rig`), repairs it with
   `armature fix`, then imports the result into a real Unity editor (GameCI's x86_64 Docker image)
   and asserts the generated avatar is a valid Humanoid (`Avatar.isValid && isHuman`) with no manual
   bone assignment, via `acceptance/unity/Assets/Editor/HumanoidAcceptance.cs`. It runs on
   `ubuntu-latest` (amd64) — no local Unity of any architecture is involved — and self-skips until a
   `UNITY_LICENSE` secret is configured. Unity retired web activation for *Personal* (free)
   licenses and ships no arm64 Linux Hub, so `scripts/acquire-unity-license.sh` mints the `.ulf`
   locally (your account credentials never touch a runner; only the expiring `.ulf` is uploaded).
   The acceptance fixture is a *skeleton only*: if a Unity run rejects a mesh-less skeleton, the
   follow-up is to extend `emit_broken_rig` with a one-triangle skinned mesh.

3. **The interactive last mile remains yours.** Confirming a *specific real avatar* imports cleanly,
   and the VRChat SDK upload itself, still belong to the Unity editor and an interactive VRChat
   login, which these tools deliberately don't own (see `PLAN.md` §1, §5). The manual procedure
   below reproduces the automated check by hand; record real runs in the ledger.

### Reproducing the manual acceptance check

```sh
# 1. Produce a repaired FBX from a non-standard rig (e.g. a raw Mixamo export).
cargo run -p avatar-cli -- armature fix path/to/raw.fbx -o /tmp/fixed.fbx

# 2. Sanity-check the artifact before touching Unity: it should now be humanoid-ready (exit 0),
#    with canonical bone names.
cargo run -p avatar-cli -- armature check /tmp/fixed.fbx
```

Then, in Unity (with the VRChat SDK present):

3. Import `/tmp/fixed.fbx`. In the model importer, set **Rig → Animation Type → Humanoid**,
   **Avatar Definition → Create From This Model**, and click **Apply**.
4. Open **Configure…** and confirm Unity mapped the full required bone set (Hips, Spine, Head, both
   arms/hands, both legs/feet) **with no manual assignment** and reports a valid avatar.
5. Record the outcome in the ledger below (Unity version, SDK version, source rig, result, notes).

> Note the **scale** caveat: `fix` flags but does not change `UnitScaleFactor`. A rig authored in
> meters (factor `1`, like stock Mixamo) imports at 1/100th scale; set the importer **Scale Factor**
> to `100`, or fix units in your DCC tool. This is expected, not a regression — it's the geometry
> transform `fix` deliberately leaves to Blender (`PLAN.md` §8).

### Acceptance ledger

Record each verified import here so the proof is durable and not re-litigated. The `Unity acceptance`
workflow above is the standing automated proof once `UNITY_LICENSE` is set; this table is for the
first green run of that workflow and for any manual imports of *real* avatars.

| Date | Unity | VRChat SDK | Source rig | Result | Notes |
|------|-------|------------|------------|--------|-------|
| _pending_ | 2022.3.22f1 | — | `emit_broken_rig` → `fix -o` (CI) | _not yet run_ | First target: the headless `Unity acceptance` workflow. Skeleton-only fixture. |
| _pending_ | — | — | `samples/SambaDancing.fbx` → `fix -o` | _not yet run_ | Manual import of a real rig. Renames applied (24); unit scale flagged — set importer Scale Factor 100. |

When you record a row, also update `PLAN.md` §8 (the residual "manual Unity import" risk note) to
point at it.
