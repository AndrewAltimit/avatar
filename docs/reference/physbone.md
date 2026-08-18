# PhysBone tuning — `avatar physbone list|set|split|stretch|flare`

The first thing anyone does after wearing a migrated (or freshly authored) avatar is retune its
PhysBones: the hair is too floppy, the skirt flaps, the tips of a long chain go wild. That is a
read-modify-write of one `VRCPhysBone` component — a job for the surgical editor, not a
regeneration. `avatar physbone` does it on the prefab file, so a change is a one-line command
that lands on the exact component and leaves every other byte of the prefab (fileIDs, references,
the blueprint id, formatting) alone. It lives in `avatar-migrate` (`avatar_migrate::physbone`)
because it is the post-migration step and reuses the migration's prefab graph + rewriter; it works
on any SDK3 prefab, migrated or not.

```sh
avatar physbone list    Avatar.prefab [--json]
avatar physbone set     Avatar.prefab <TARGET> [tuning…] [--ignore N]… [--collider N]… -o Avatar.prefab --force
avatar physbone split   Avatar.prefab <TARGET> --chain Hair_1 --chain Hair_2 [tuning…] -o … --force
avatar physbone stretch Avatar.prefab <TARGET> --factor 1.5 [--from-depth 2] -o … --force
avatar physbone flare   Avatar.prefab <TARGET> --angle 10 | --scale 0.5 [--hinge-depth 1] -o … --force
```

`TARGET` is the PhysBone's **root transform** (a unique bone name or an `A/B/C` path from the
avatar root), the GameObject carrying it, or its `&fileID` (the `list` output prints all three;
an ambiguous name errors with the candidates). Edits follow the shared write policy: `-o FILE`
writes (the input path with `--force` edits in place), no `-o` prints the edited prefab to stdout,
`--dry-run` reports without writing, `--json` prints a machine-readable before/after report
(the schema is `avatar schema physbone` — one `PhysBoneInfo`). The file is written *before* the
report is printed, so a closed pipe can never lose the edit.

## `list` — what a PhysBone is doing

For each `VRCPhysBone` (recognised by its DLL class reference, `{fileID: 1661641543, guid:
2a2c05204084d904aa4945ccff20d8e5}`): the object it sits on, the root it simulates from
(`rootTransform`, else its own), the **chains** it drives — for each leaf reachable from the root
without crossing an `ignoreTransforms` entry, the bones from the first simulated bone to the
leaf and their length in avatar space (root scale included) — the total transforms simulated
(the number VRChat's perf rank counts), colliders (by the objects that carry them), and the
tuning: version, integration, multi-child, pull/spring/stiffness, gravity + falloff, immobile
(+ type), radius, limit + max angles, grab/pose/collision flags, stretch/squish, `isAnimated`,
`parameter` — each with its **curve** if one is set. Each chain also reports its **flare**: the
angle between its first-bone→leaf direction and straight down (0° = hangs vertically).

*Which bone is the first bone?* PhysBone simulates from the root transform. When the root has
several live children and `multiChildType` is *Ignore* (the default), the root itself stays put
and each child starts a chain; with a single child the root **is** the first bone. `list` shows
exactly that (a 14-bone pigtail rooted on `Hair_1` lists 14 bones; a hair root with two chains
lists 2×N and doesn't count itself) — and it is why splitting all-but-one chain off a root turns
the root into a simulated bone.

## `set` — retune one component

Every tuning value is optional; unset = keep. Values: `--pull --spring --stiffness --gravity
--gravity-falloff --immobile --immobile-type all|world --radius --limit-type none|angle|hinge|polar
--max-angle --max-angle-z --integration simplified|advanced --version 1.0|1.1 --multi-child
ignore|first|average --allow-collision 0|1 --allow-grabbing 0|1 --allow-posing 0|1 --max-stretch
--max-squish --is-animated 0|1`. Lists: `--ignore NAME` / `--unignore NAME` (a bare child name of
the root, or a name/path), `--collider OBJ` / `--uncollider OBJ` (GameObjects carrying a
`VRCPhysBoneCollider`).

**Curves.** `--pull-curve`, `--spring-curve`, `--stiffness-curve`, `--gravity-curve`,
`--immobile-curve`, `--radius-curve`, `--max-angle-curve` take `T:V,T:V,…` keys — `T` is the
position along the chain (0 = first bone, 1 = tip), `V` a 0..1 **multiplier of the base value**,
which is how the SDK applies them (`pull 0.3` with `0:0.5,1:1` pulls 0.15 at the root and 0.3 at
the tip). Keys are written with free tangents equal to the secants to their neighbours, so the
Hermite curve Unity evaluates is exactly piecewise-**linear** between your keys — no editor
smoothing surprises; an empty string clears a curve. "More weight at the ends" is a pull curve
rising toward the tip plus a spring curve falling toward it.

The component body is re-rendered from a typed `PhysBoneSpec` read back from the prefab
(`PhysBoneSpec::from_yaml` → tune → `to_body`, the same emitter the migration uses; round trip
is byte-stable and test-pinned), so a `set` rewrites exactly one document and touches nothing
else.

## `split` — tune chains apart

Long pigtails and short bangs usually share one component rooted on `Head`/`Hair`, and one set of
numbers can't suit both. `split` moves each named chain (a child of the root, by name or path)
onto its **own** `VRCPhysBone` on that chain's first bone (root = itself), tuned like the parent
plus whatever tuning flags you pass, with the parent's colliders and any of its ignores that lie
inside the chain — and adds the chain to the parent's `ignoreTransforms`. New fileIDs are derived
from the bone path (stable across runs). Component count goes up by one per chain (PhysBone
*components* are a perf metric: PC 4/8/16/32); simulated transforms don't change.

## `stretch` — a longer skirt / tail without a mesh edit

`--factor F` multiplies the local position (the offset from the parent bone) of every chain
transform at depth ≥ `--from-depth` below the PhysBone root (1 = the root's children, 2 =
grandchildren, …; the root itself never moves). The skinned mesh follows its bones, so what hangs
off the chain gets longer: each ring of vertices bound to a deeper bone moves further down the
chain and the faces between rings stretch. Only translations change — no rotation, no
non-uniform scale, so **no shear**, and PhysBone sees ordinary (longer) bones with the same
radius. A non-zero `endpointPosition` is scaled too. Default depth 2 keeps the root's children —
the hinges of a many-chain skirt — in place; use `--from-depth 1` on a component rooted on a
chain's own first bone (a split-off pigtail).

Caveats: it's a stretch, so a texture stretches with it (×F along the chain; up to ~1.5–1.7 reads
fine on a plain skirt, beyond that a hem's frill ring visibly distorts); a mesh's `m_AABB` /
bounds are not enlarged (Unity's skinned bounds are usually generous — turn on *Update When
Offscreen* if the hem gets culled at frame edges); and colliders placed for the old length may
now sit above the hem.

**Preview before Unity:** `avatar render --avatar model.fbx --pose Avatar.prefab -o out.png`
draws the FBX with every bone's local transform taken from the prefab (matched by name, Unity's
mirrored import undone), so what you see is what Unity will show for that prefab — a stretched,
re-angled skirt, anything. `--stretch 'Skirt_0_*:1.5'` is the lighter one-knob form for a
stretch alone ([render.md](render.md)).

## `flare` — hug the legs / lift a tail

A skirt whose chains tilt outward gets a funnel silhouette, and stretching it along that tilt
makes the funnel wider. `flare` re-angles each chain in **avatar space**: the transform at
`--hinge-depth` below the root (1 = the root's children, a skirt's hinge ring; 0 = the root itself,
for a component rooted on the chain's first bone — careful, that swings every child of the root,
ignored ones too) is rotated about the axis perpendicular to its chain direction and −Y so the
hinge→leaf direction makes the target angle with straight down: `--angle 10` for every chain, or
`--scale 0.5` to halve each chain's current angle. Only that one local rotation changes per chain
(`local' = R_parent⁻¹·Δ·R_parent·local`, the eye-look construction), so the chain swings rigidly
like a panel about its hinge and the skinned mesh follows. It is a **rest-pose** edit — PhysBone
takes the new pose as rest, and colliders still push the chain out where the new rest sits inside
them (thighs), which is exactly the "drape over the legs" you want. `list` shows the resulting
angles; note it measures from the chain's *first simulated bone*, which is the hinge for a
many-chain root but the root itself for a single-chain component.

## Example — the mikunpc pass

The pigtails go on their own components with weight and damped tips, the bangs/antenna/sideburns
that stay on `Hair` get calm settings, the skirt gets firmer and 50 % longer:

```sh
P=Assets/MikuNPC_SDK3/MikuNPC.prefab
avatar physbone split   $P Hair --chain Hair_1 --chain Hair_2 \
  --pull 0.3 --pull-curve "0:0.7,1:1" --spring 0.3 --spring-curve "0:1,1:0.5" --stiffness 0.2 \
  --gravity 0.15 --gravity-falloff 0 --immobile 0.7 --immobile-type world \
  --limit-type angle --max-angle 60 -o $P --force
avatar physbone set     $P Hair --pull 0.35 --spring 0.3 --stiffness 0.3 --immobile 0.7 \
  --immobile-type world --limit-type angle --max-angle 30 -o $P --force
avatar physbone set     $P SkirtRoot --pull 0.4 --spring 0.2 --stiffness 0.35 --gravity 0.05 \
  --gravity-falloff 0.5 --immobile 0.6 --immobile-type world --limit-type angle --max-angle 35 \
  -o $P --force
avatar physbone stretch $P SkirtRoot --factor 1.5 -o $P --force
avatar physbone list    $P
# round two, after wearing it: longer still, and the funnel pulled in to hug the legs
avatar physbone stretch $P SkirtRoot --factor 1.3 -o $P --force     # x1.95 in total
avatar physbone flare   $P SkirtRoot --angle 10 -o $P --force       # chains were 13–33° out
avatar render --avatar final.fbx --pose $P -o preview.png            # what Unity will show
```

`avatar lint` stays clean and `avatar stats` reports the change (5 PhysBone components: Good;
transforms/collision checks unchanged: Medium).

## Testing

`crates/migrate/src/physbone.rs` unit-tests list/find/set/split/stretch on a synthetic prefab;
`crates/migrate/tests/golden.rs` runs the migration on `fixtures/projects/Sdk2Project`, then
lists (`Sdk2Project.physbones.json`), splits the fixture's `Bang` chain off its hair, retunes the
pigtail with curves, re-angles it, stretches the skirt, and pins the result (`Sdk2Project.physbones.tuned.json`)
while asserting untouched documents are byte-identical. `sdk3.rs` pins the
`PhysBoneSpec` YAML round trip and the linear-tangent curve text.
