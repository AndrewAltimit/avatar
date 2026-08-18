//! Post-migration PhysBone work on an SDK3 prefab: **list** every `VRCPhysBone` with its chains
//! and tuning, **set** tuning (values and per-chain curves) on one, **split** chains off into
//! their own components, and **stretch** a chain's bones to lengthen what hangs off them (a
//! longer skirt, a longer tail) — all as surgical edits over [`PrefabRewriter`], so fileIDs and
//! everything untouched round-trip byte-for-byte.
//!
//! # Why these four
//!
//! The SDK's DynamicBone→PhysBone conversion (and any first-pass authoring) gives *a* set of
//! parameters; the first thing anyone does after wearing the avatar is retune. That is a
//! read-modify-write of one component ([`set`]). Long hair chains and short bangs usually share
//! one component rooted on `Head`/`Hair`, so tuning one ruins the other — [`split`] moves named
//! chains onto their own components (each rooted on its own first bone, added to the parent's
//! `ignoreTransforms`) so they can be tuned apart. [`stretch`] scales the offsets of the bones
//! *below* a chain's hinge, which the skinned mesh follows: a cheap, Unity-side "make it longer"
//! that needs no mesh edit and keeps every bone's rotation (no non-uniform scale, no shear).
//!
//! # Curves
//!
//! A PhysBone curve (`pullCurve`, `springCurve`, …) is a 0..1 multiplier of the base value
//! along the chain (0 = first bone, 1 = tip). "More weight at the ends" is a `pull` curve rising
//! toward 1 and a `spring` curve falling — see [`crate::sdk3::Curve`].

use std::collections::HashSet;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::rewrite::PrefabRewriter;
use crate::scene::{MONO_BEHAVIOUR, Scene};
use crate::sdk3::{Curve, LimitType, PhysBoneSpec, VRC_PHYS_BONE};

/// One chain under a PhysBone: the path from the first simulated bone to a leaf.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ChainInfo {
    /// Path (from the avatar root) of the chain's leaf transform.
    pub leaf: String,
    /// Bones in the chain (first simulated bone through the leaf).
    pub bones: usize,
    /// Chain length in avatar space (root-scale included), first bone → leaf.
    pub length: f64,
    /// Angle (degrees) between the first-bone→leaf direction and straight down (avatar −Y):
    /// 0 = hangs vertically, 90 = sticks out sideways. What [`flare`] adjusts.
    pub flare_deg: f64,
}

/// A `VRCPhysBone` as found in a prefab: where it is, what it drives, how it is tuned.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PhysBoneInfo {
    pub file_id: i64,
    /// Path of the GameObject carrying the component.
    pub object: String,
    /// Path of the root transform (the component's own when `rootTransform` is unset).
    pub root: String,
    pub ignore: Vec<String>,
    /// Paths of the collider components' GameObjects.
    pub colliders: Vec<String>,
    pub chains: Vec<ChainInfo>,
    /// Total transforms simulated (what VRChat counts as PhysBone transforms).
    pub transforms: usize,
    pub version: i64,
    pub integration_type: i64,
    pub multi_child_type: i64,
    pub pull: f64,
    pub pull_curve: Vec<(f64, f64)>,
    pub spring: f64,
    pub spring_curve: Vec<(f64, f64)>,
    pub stiffness: f64,
    pub stiffness_curve: Vec<(f64, f64)>,
    pub gravity: f64,
    pub gravity_curve: Vec<(f64, f64)>,
    pub gravity_falloff: f64,
    pub immobile_type: i64,
    pub immobile: f64,
    pub immobile_curve: Vec<(f64, f64)>,
    pub radius: f64,
    pub radius_curve: Vec<(f64, f64)>,
    pub allow_collision: bool,
    pub limit_type: i64,
    pub max_angle_x: f64,
    pub max_angle_x_curve: Vec<(f64, f64)>,
    pub max_angle_z: f64,
    pub allow_grabbing: bool,
    pub allow_posing: bool,
    pub max_stretch: f64,
    pub max_squish: f64,
    pub is_animated: bool,
    pub parameter: String,
}

/// True if `doc` is a `VRCPhysBone` (by the DLL class reference).
pub fn is_physbone(doc: &avatar_unity_yaml::UnityDocument) -> bool {
    doc.class_id == MONO_BEHAVIOUR
        && doc.script_guid() == Some(VRC_PHYS_BONE.guid)
        && avatar_unity_yaml::ref_fileid(&doc.body, "m_Script")
            == Some(i64::from(VRC_PHYS_BONE.file_id))
}

/// The spec of PhysBone `file_id`.
pub fn spec_of(scene: &Scene, file_id: i64) -> Result<PhysBoneSpec> {
    let doc = scene
        .doc(file_id)
        .with_context(|| format!("no document &{file_id}"))?;
    if !is_physbone(doc) {
        bail!("&{file_id} is not a VRCPhysBone");
    }
    Ok(PhysBoneSpec::from_yaml(&doc.body))
}

/// The transform a PhysBone simulates from: `rootTransform`, else its own GameObject's.
pub fn root_transform(scene: &Scene, spec: &PhysBoneSpec) -> Option<i64> {
    if spec.root_transform != 0 {
        Some(spec.root_transform)
    } else {
        scene
            .game_objects
            .get(&spec.game_object)
            .map(|g| g.transform)
    }
}

/// The chains a PhysBone drives: for each leaf reachable from the root without crossing an
/// ignored transform, the transforms from the first simulated bone down to it. When the root has
/// several (non-ignored) children and `multiChildType` is *Ignore* (0), the root itself is not
/// simulated and each child starts a chain; otherwise the root is the first bone.
pub fn chains(scene: &Scene, spec: &PhysBoneSpec) -> Vec<Vec<i64>> {
    let Some(root) = root_transform(scene, spec) else {
        return Vec::new();
    };
    let ignore: HashSet<i64> = spec.ignore_transforms.iter().copied().collect();
    let live_children = |t: i64| -> Vec<i64> {
        scene
            .transforms
            .get(&t)
            .map(|tr| {
                tr.children
                    .iter()
                    .copied()
                    .filter(|c| !ignore.contains(c) && scene.transforms.contains_key(c))
                    .collect()
            })
            .unwrap_or_default()
    };
    let starts: Vec<i64> = {
        let kids = live_children(root);
        if kids.len() > 1 && spec.multi_child_type == 0 {
            kids
        } else {
            vec![root]
        }
    };
    let mut out = Vec::new();
    for s in starts {
        // Depth-first; each leaf yields the path from `s`.
        let mut stack: Vec<(i64, Vec<i64>)> = vec![(s, vec![s])];
        while let Some((t, path)) = stack.pop() {
            let kids = live_children(t);
            if kids.is_empty() {
                out.push(path);
                continue;
            }
            for k in kids.into_iter().rev() {
                let mut p = path.clone();
                p.push(k);
                stack.push((k, p));
            }
        }
    }
    out
}

/// Angle in degrees between the chain's first→leaf direction and avatar −Y.
fn chain_flare_deg(scene: &Scene, chain: &[i64]) -> f64 {
    let (Some(a), Some(b)) = (chain.first(), chain.last()) else {
        return 0.0;
    };
    let d = scene.world(*b).position - scene.world(*a).position;
    let len = d.length();
    if len < 1e-9 {
        return 0.0;
    }
    (-d.y / len).clamp(-1.0, 1.0).acos().to_degrees()
}

fn chain_length(scene: &Scene, chain: &[i64]) -> f64 {
    let mut len = 0.0;
    for w in chain.windows(2) {
        let a = scene.world(w[0]).position;
        let b = scene.world(w[1]).position;
        len += (b - a).length();
    }
    len
}

/// Describe PhysBone `file_id`.
pub fn info(scene: &Scene, file_id: i64) -> Result<PhysBoneInfo> {
    let spec = spec_of(scene, file_id)?;
    Ok(info_of_spec(scene, file_id, &spec))
}

fn info_of_spec(scene: &Scene, file_id: i64, spec: &PhysBoneSpec) -> PhysBoneInfo {
    let object = scene
        .game_objects
        .get(&spec.game_object)
        .map(|g| scene.path_of(g.transform))
        .unwrap_or_default();
    let root = root_transform(scene, spec)
        .map(|t| scene.path_of(t))
        .unwrap_or_default();
    let chains_t = chains(scene, spec);
    let mut simulated: HashSet<i64> = HashSet::new();
    for c in &chains_t {
        simulated.extend(c.iter().copied());
    }
    let keys = |c: &Curve| c.0.clone();
    PhysBoneInfo {
        file_id,
        object,
        root,
        ignore: spec
            .ignore_transforms
            .iter()
            .map(|t| scene.path_of(*t))
            .collect(),
        colliders: spec
            .colliders
            .iter()
            .map(|c| {
                scene
                    .transform_of_component(*c)
                    .map(|t| scene.path_of(t))
                    .unwrap_or_else(|| format!("&{c}"))
            })
            .collect(),
        chains: chains_t
            .iter()
            .map(|c| ChainInfo {
                leaf: scene.path_of(*c.last().unwrap()),
                bones: c.len(),
                length: chain_length(scene, c),
                flare_deg: chain_flare_deg(scene, c),
            })
            .collect(),
        transforms: simulated.len(),
        version: spec.version,
        integration_type: spec.integration_type,
        multi_child_type: spec.multi_child_type,
        pull: spec.pull,
        pull_curve: keys(&spec.pull_curve),
        spring: spec.spring,
        spring_curve: keys(&spec.spring_curve),
        stiffness: spec.stiffness,
        stiffness_curve: keys(&spec.stiffness_curve),
        gravity: spec.gravity,
        gravity_curve: keys(&spec.gravity_curve),
        gravity_falloff: spec.gravity_falloff,
        immobile_type: spec.immobile_type,
        immobile: spec.immobile,
        immobile_curve: keys(&spec.immobile_curve),
        radius: spec.radius,
        radius_curve: keys(&spec.radius_curve),
        allow_collision: spec.allow_collision,
        limit_type: spec.limit_type as i64,
        max_angle_x: spec.max_angle_x,
        max_angle_x_curve: keys(&spec.max_angle_x_curve),
        max_angle_z: spec.max_angle_z,
        allow_grabbing: spec.allow_grabbing,
        allow_posing: spec.allow_posing,
        max_stretch: spec.max_stretch,
        max_squish: spec.max_squish,
        is_animated: spec.is_animated,
        parameter: spec.parameter.clone(),
    }
}

/// Every PhysBone in the prefab, ordered by object path.
pub fn list(scene: &Scene) -> Vec<PhysBoneInfo> {
    let mut out: Vec<PhysBoneInfo> = scene
        .docs
        .values()
        .filter(|d| is_physbone(d))
        .map(|d| info_of_spec(scene, d.file_id, &PhysBoneSpec::from_yaml(&d.body)))
        .collect();
    out.sort_by(|a, b| a.object.cmp(&b.object).then(a.file_id.cmp(&b.file_id)));
    out
}

/// Resolve `target` to a PhysBone fileID: a bare number is a fileID; otherwise a transform
/// (unique name or `A/B/C` path) that is a PhysBone's root, else the GameObject carrying one.
/// Errors when nothing or more than one matches (listing the candidates).
pub fn find(scene: &Scene, target: &str) -> Result<i64> {
    let target = target.trim();
    if let Ok(id) = target.parse::<i64>() {
        spec_of(scene, id)?;
        return Ok(id);
    }
    let t = scene
        .transform_by_path(target)
        .with_context(|| format!("PhysBone target '{target}'"))?;
    let all = list(scene);
    let path = scene.path_of(t);
    let by_root: Vec<&PhysBoneInfo> = all.iter().filter(|p| p.root == path).collect();
    let hits: Vec<&PhysBoneInfo> = if by_root.is_empty() {
        all.iter().filter(|p| p.object == path).collect()
    } else {
        by_root
    };
    match hits.len() {
        1 => Ok(hits[0].file_id),
        0 => bail!(
            "'{target}' is neither a PhysBone root nor carries a PhysBone (roots: {})",
            all.iter()
                .map(|p| p.root.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => bail!(
            "'{target}' matches {} PhysBones — pass a fileID: {}",
            hits.len(),
            hits.iter()
                .map(|p| format!("&{} (on {})", p.file_id, p.object))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Tuning overrides applied over an existing spec: every field optional, `None` = keep.
#[derive(Debug, Clone, Default)]
pub struct Tuning {
    pub version: Option<i64>,
    pub integration_type: Option<i64>,
    pub multi_child_type: Option<i64>,
    pub pull: Option<f64>,
    pub pull_curve: Option<Curve>,
    pub spring: Option<f64>,
    pub spring_curve: Option<Curve>,
    pub stiffness: Option<f64>,
    pub stiffness_curve: Option<Curve>,
    pub gravity: Option<f64>,
    pub gravity_curve: Option<Curve>,
    pub gravity_falloff: Option<f64>,
    pub immobile_type: Option<i64>,
    pub immobile: Option<f64>,
    pub immobile_curve: Option<Curve>,
    pub radius: Option<f64>,
    pub radius_curve: Option<Curve>,
    pub allow_collision: Option<bool>,
    pub limit_type: Option<LimitType>,
    pub max_angle_x: Option<f64>,
    pub max_angle_x_curve: Option<Curve>,
    pub max_angle_z: Option<f64>,
    pub allow_grabbing: Option<bool>,
    pub allow_posing: Option<bool>,
    pub max_stretch: Option<f64>,
    pub max_squish: Option<f64>,
    pub is_animated: Option<bool>,
}

impl Tuning {
    /// True when nothing is set.
    pub fn is_empty(&self) -> bool {
        self.changes().is_empty()
    }

    /// Human list of what is set (`pull=0.3`, `spring curve 0:1 → 1:0.5`, …).
    pub fn changes(&self) -> Vec<String> {
        let mut v = Vec::new();
        let f = |k: &str, x: Option<f64>, v: &mut Vec<String>| {
            if let Some(x) = x {
                v.push(format!("{k}={}", crate::math::fmt(x)));
            }
        };
        let i = |k: &str, x: Option<i64>, v: &mut Vec<String>| {
            if let Some(x) = x {
                v.push(format!("{k}={x}"));
            }
        };
        let b = |k: &str, x: Option<bool>, v: &mut Vec<String>| {
            if let Some(x) = x {
                v.push(format!("{k}={}", x as u8));
            }
        };
        let c = |k: &str, x: &Option<Curve>, v: &mut Vec<String>| {
            if let Some(x) = x {
                v.push(format!("{k} curve {}", x.describe()));
            }
        };
        i("version", self.version, &mut v);
        i("integrationType", self.integration_type, &mut v);
        i("multiChildType", self.multi_child_type, &mut v);
        f("pull", self.pull, &mut v);
        c("pull", &self.pull_curve, &mut v);
        f("spring", self.spring, &mut v);
        c("spring", &self.spring_curve, &mut v);
        f("stiffness", self.stiffness, &mut v);
        c("stiffness", &self.stiffness_curve, &mut v);
        f("gravity", self.gravity, &mut v);
        c("gravity", &self.gravity_curve, &mut v);
        f("gravityFalloff", self.gravity_falloff, &mut v);
        i("immobileType", self.immobile_type, &mut v);
        f("immobile", self.immobile, &mut v);
        c("immobile", &self.immobile_curve, &mut v);
        f("radius", self.radius, &mut v);
        c("radius", &self.radius_curve, &mut v);
        b("allowCollision", self.allow_collision, &mut v);
        if let Some(l) = self.limit_type {
            v.push(format!("limitType={:?}", l));
        }
        f("maxAngleX", self.max_angle_x, &mut v);
        c("maxAngleX", &self.max_angle_x_curve, &mut v);
        f("maxAngleZ", self.max_angle_z, &mut v);
        b("allowGrabbing", self.allow_grabbing, &mut v);
        b("allowPosing", self.allow_posing, &mut v);
        f("maxStretch", self.max_stretch, &mut v);
        f("maxSquish", self.max_squish, &mut v);
        b("isAnimated", self.is_animated, &mut v);
        v
    }

    /// Apply over `spec`.
    pub fn apply(&self, spec: &mut PhysBoneSpec) {
        macro_rules! set {
            ($($f:ident),*) => { $( if let Some(v) = self.$f.clone() { spec.$f = v; } )* };
        }
        set!(
            version,
            integration_type,
            multi_child_type,
            pull,
            pull_curve,
            spring,
            spring_curve,
            stiffness,
            stiffness_curve,
            gravity,
            gravity_curve,
            gravity_falloff,
            immobile_type,
            immobile,
            immobile_curve,
            radius,
            radius_curve,
            allow_collision,
            limit_type,
            max_angle_x,
            max_angle_x_curve,
            max_angle_z,
            allow_grabbing,
            allow_posing,
            max_stretch,
            max_squish,
            is_animated
        );
    }
}

/// Resolve a transform given as a name/path — or, for convenience, as a bare child name of
/// `under` — to its fileID.
fn resolve_transform(scene: &Scene, under: Option<i64>, name: &str) -> Result<i64> {
    if let Some(u) = under
        && !name.contains('/')
        && let Some(tr) = scene.transforms.get(&u)
        && let Some(c) = tr
            .children
            .iter()
            .copied()
            .find(|c| scene.name_of_transform(*c) == name)
    {
        return Ok(c);
    }
    scene
        .transform_by_path(name)
        .with_context(|| format!("transform '{name}'"))
}

/// Retune PhysBone `file_id`: apply `tuning`, add/remove ignored transforms (names/paths, or bare
/// child names of the root) and colliders (GameObject names/paths carrying a PhysBoneCollider).
/// Returns the new state.
pub fn set(
    rw: &mut PrefabRewriter,
    file_id: i64,
    tuning: &Tuning,
    ignore_add: &[String],
    ignore_remove: &[String],
    colliders_add: &[String],
    colliders_remove: &[String],
) -> Result<PhysBoneInfo> {
    let mut spec = spec_of(rw.scene(), file_id)?;
    tuning.apply(&mut spec);
    let root = root_transform(rw.scene(), &spec);
    for n in ignore_add {
        let t = resolve_transform(rw.scene(), root, n)?;
        if !spec.ignore_transforms.contains(&t) {
            spec.ignore_transforms.push(t);
        }
    }
    for n in ignore_remove {
        let t = resolve_transform(rw.scene(), root, n)?;
        spec.ignore_transforms.retain(|x| *x != t);
    }
    for n in colliders_add {
        let c = collider_on(rw.scene(), n)?;
        if !spec.colliders.contains(&c) {
            spec.colliders.push(c);
        }
    }
    for n in colliders_remove {
        let c = collider_on(rw.scene(), n)?;
        spec.colliders.retain(|x| *x != c);
    }
    rw.replace_component_body(file_id, &spec.to_body())?;
    Ok(info_of_spec(rw.scene(), file_id, &spec))
}

/// The `VRCPhysBoneCollider` component on the GameObject at `name` (unique name or path).
fn collider_on(scene: &Scene, name: &str) -> Result<i64> {
    let t = scene
        .transform_by_path(name)
        .with_context(|| format!("collider object '{name}'"))?;
    let go = scene.transforms[&t].game_object;
    let found = scene
        .game_objects
        .get(&go)
        .into_iter()
        .flat_map(|g| g.components.iter().copied())
        .find(|c| {
            scene.doc(*c).is_some_and(|d| {
                d.class_id == MONO_BEHAVIOUR
                    && d.script_guid() == Some(crate::sdk3::VRC_PHYS_BONE_COLLIDER.guid)
                    && avatar_unity_yaml::ref_fileid(&d.body, "m_Script")
                        == Some(i64::from(crate::sdk3::VRC_PHYS_BONE_COLLIDER.file_id))
            })
        });
    found.with_context(|| format!("'{name}' carries no VRCPhysBoneCollider"))
}

/// A chain moved onto its own component by [`split`].
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SplitChain {
    /// Path of the chain's first bone (the new component's GameObject and root).
    pub path: String,
    /// The new PhysBone's fileID.
    pub file_id: i64,
    pub bones: usize,
    pub length: f64,
}

/// Move the chains starting at `chain_roots` (children of PhysBone `file_id`'s root, by name or
/// path) onto new `VRCPhysBone` components — one per chain, on the chain's first bone (root =
/// itself), tuned like the parent plus `tuning`, with the parent's colliders — and add each to the
/// parent's `ignoreTransforms`. New fileIDs are derived from the bone path (stable across runs).
pub fn split(
    rw: &mut PrefabRewriter,
    file_id: i64,
    chain_roots: &[String],
    tuning: &Tuning,
) -> Result<Vec<SplitChain>> {
    if chain_roots.is_empty() {
        bail!("split: no chains named");
    }
    let parent = spec_of(rw.scene(), file_id)?;
    let root = root_transform(rw.scene(), &parent).context("PhysBone has no root transform")?;
    let mut targets = Vec::new();
    for n in chain_roots {
        let t = resolve_transform(rw.scene(), Some(root), n)?;
        // Must lie under the root, and not under an ignored branch — otherwise it isn't this
        // component's chain to give away.
        let under = rw.scene().descendants(root);
        if t == root || !under.contains(&t) {
            bail!(
                "'{n}' is not under the PhysBone root '{}'",
                rw.scene().path_of(root)
            );
        }
        for ig in &parent.ignore_transforms {
            if rw.scene().descendants(*ig).contains(&t) {
                bail!(
                    "'{n}' is already ignored by this PhysBone (under '{}')",
                    rw.scene().path_of(*ig)
                );
            }
        }
        targets.push(t);
    }
    let mut out = Vec::new();
    let mut parent_ignore = parent.ignore_transforms.clone();
    for t in targets {
        let path = rw.scene().path_of(t);
        let go = rw.scene().transforms[&t].game_object;
        let mut spec = parent.clone();
        spec.game_object = go;
        spec.root_transform = 0;
        // Ignores of the parent that live inside this chain stay with it.
        let inside = rw.scene().descendants(t);
        spec.ignore_transforms = parent
            .ignore_transforms
            .iter()
            .copied()
            .filter(|i| inside.contains(i))
            .collect();
        tuning.apply(&mut spec);
        let seed = format!("physbone:{path}");
        let id = rw.add_component(go, MONO_BEHAVIOUR, &spec.to_body(), &seed)?;
        let chains_t = chains(rw.scene(), &spec);
        let bones: HashSet<i64> = chains_t.iter().flatten().copied().collect();
        let length = chains_t
            .iter()
            .map(|c| chain_length(rw.scene(), c))
            .fold(0.0, f64::max);
        out.push(SplitChain {
            path,
            file_id: id,
            bones: bones.len(),
            length,
        });
        if !parent_ignore.contains(&t) {
            parent_ignore.push(t);
        }
    }
    let mut parent = parent;
    parent.ignore_transforms = parent_ignore;
    rw.replace_component_body(file_id, &parent.to_body())?;
    Ok(out)
}

/// One bone offset changed by [`stretch`].
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StretchedBone {
    pub path: String,
    /// Offset from the parent before/after, avatar space.
    pub before: f64,
    pub after: f64,
}

/// Result of [`stretch`].
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StretchReport {
    /// The uniform factor (`stretch`), or `NaN`-free 0 when `--by` was used (see `by`).
    pub factor: f64,
    /// The length added per chain when [`StretchAmount::By`] was used.
    pub by: Option<f64>,
    pub bones: Vec<StretchedBone>,
    /// Per chain: leaf path, length before → after (avatar space).
    pub chains: Vec<(String, f64, f64)>,
}

/// Lengthen the chains of PhysBone `file_id` by `factor`: every chain transform at depth ≥
/// `from_depth` below the PhysBone's root (1 = the root's children, 2 = grandchildren, …; the
/// root itself never moves) has its local position — its offset from the parent bone — multiplied
/// by `factor`. The skinned mesh follows the bones, so what hangs off the chain gets longer
/// without touching the mesh, and no rotation or scale changes, so PhysBone sees ordinary
/// (longer) bones. Depth 2 (the default) keeps the root's children in place — the hinges of a
/// many-chain skirt; use 1 for a component rooted on a chain's own first bone (a split-off
/// pigtail). A non-zero `endpointPosition` is scaled too.
pub fn stretch(
    rw: &mut PrefabRewriter,
    file_id: i64,
    factor: f64,
    from_depth: usize,
) -> Result<StretchReport> {
    if !(factor.is_finite() && factor > 0.0) {
        bail!("stretch factor must be > 0 (got {factor})");
    }
    stretch_with(rw, file_id, StretchAmount::Factor(factor), from_depth)
}

/// How much [`stretch_with`] lengthens each chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StretchAmount {
    /// Multiply every scaled offset by this (a 5-bone chain grows more than a 4-bone one).
    Factor(f64),
    /// Add this length (avatar space, metres) to every chain — each chain gets its own factor
    /// `1 + by / length`, so chains of unequal bone count grow by the same amount and an even hem
    /// stays even. Negative shortens.
    By(f64),
}

/// [`stretch`] with a per-chain amount; see [`StretchAmount`]. Chains sharing a bone (a
/// branching root) use the factor of the first chain that reaches it.
pub fn stretch_with(
    rw: &mut PrefabRewriter,
    file_id: i64,
    amount: StretchAmount,
    from_depth: usize,
) -> Result<StretchReport> {
    let factor = match amount {
        StretchAmount::Factor(f) => f,
        StretchAmount::By(b) => {
            if !b.is_finite() {
                bail!("stretch --by must be a finite length (got {b})");
            }
            f64::NAN // per chain, below
        }
    };
    if from_depth < 1 {
        bail!("--from-depth must be ≥ 1 (the root transform itself never moves)");
    }
    let mut spec = spec_of(rw.scene(), file_id)?;
    let root = root_transform(rw.scene(), &spec).context("PhysBone has no root transform")?;
    let chains_t = chains(rw.scene(), &spec);
    if chains_t.is_empty() {
        bail!("PhysBone &{file_id} drives no chain");
    }
    let before: Vec<(String, f64)> = chains_t
        .iter()
        .map(|c| {
            (
                rw.scene().path_of(*c.last().unwrap()),
                chain_length(rw.scene(), c),
            )
        })
        .collect();
    // Plan first (immutable reads), then edit.
    let scene = rw.scene();
    let depth_below_root = |t: i64| -> usize {
        let mut d = 0;
        let mut cur = t;
        while cur != root {
            match scene.transforms.get(&cur) {
                Some(tr) if tr.parent != 0 => {
                    cur = tr.parent;
                    d += 1;
                }
                _ => return 0,
            }
        }
        d
    };
    let mut seen: HashSet<i64> = HashSet::new();
    let mut plan: Vec<(i64, String, f64, crate::math::Vec3, f64)> = Vec::new();
    for c in &chains_t {
        // This chain's factor: uniform, or `1 + by / (length of the part being scaled)` so the
        // chain grows by exactly `by`.
        let f = match amount {
            StretchAmount::Factor(f) => f,
            StretchAmount::By(b) => {
                let scaled: f64 = c
                    .windows(2)
                    .filter(|w| depth_below_root(w[1]) >= from_depth)
                    .map(|w| {
                        scene
                            .world(w[1])
                            .position
                            .distance(scene.world(w[0]).position)
                    })
                    .sum();
                if scaled <= 1e-9 {
                    continue;
                }
                let f = 1.0 + b / scaled;
                if f <= 0.0 {
                    bail!(
                        "stretch --by {b}: chain to '{}' is only {scaled:.4} m long, it would invert",
                        scene.path_of(*c.last().unwrap())
                    );
                }
                f
            }
        };
        for &t in c {
            if t == root || depth_below_root(t) < from_depth || !seen.insert(t) {
                continue;
            }
            let tr = &scene.transforms[&t];
            let p = tr.local.position;
            let parent_scale = scene.world(tr.parent).scale;
            plan.push((t, scene.path_of(t), (p * parent_scale).length(), p, f));
        }
    }
    let mut bones = Vec::new();
    for (t, path, before_len, p, factor) in plan {
        {
            let np = p.scale(factor);
            for (k, v) in [("x", np.x), ("y", np.y), ("z", np.z)] {
                rw.set_scalar(
                    t,
                    &format!("m_LocalPosition/{k}"),
                    avatar_unity_yaml::Scalar::Float(v),
                )?;
            }
            bones.push(StretchedBone {
                path,
                before: before_len,
                after: before_len * factor,
            });
        }
    }
    if spec.endpoint_position.length() > 0.0 && factor.is_finite() {
        spec.endpoint_position = spec.endpoint_position.scale(factor);
        rw.replace_component_body(file_id, &spec.to_body())?;
    }
    // Chain lengths after: recompute on a re-parsed scene (set_scalar edits the text only).
    let after_scene = PrefabRewriter::new(rw.text())?;
    let after_spec = spec_of(after_scene.scene(), file_id)?;
    let after: Vec<(String, f64)> = chains(after_scene.scene(), &after_spec)
        .iter()
        .map(|c| {
            (
                after_scene.scene().path_of(*c.last().unwrap()),
                chain_length(after_scene.scene(), c),
            )
        })
        .collect();
    let chains_report = before
        .into_iter()
        .map(|(leaf, b)| {
            let a = after
                .iter()
                .find(|(l, _)| *l == leaf)
                .map(|(_, a)| *a)
                .unwrap_or(b);
            (leaf, b, a)
        })
        .collect();
    Ok(StretchReport {
        factor: if factor.is_finite() { factor } else { 0.0 },
        by: match amount {
            StretchAmount::By(b) => Some(b),
            StretchAmount::Factor(_) => None,
        },
        bones,
        chains: chains_report,
    })
}

/// How [`flare`] chooses each chain's new angle from vertical.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlareTarget {
    /// Multiply the current angle (0.5 = halve the flare; 0 = hang straight down).
    Scale(f64),
    /// Set the angle to this many degrees from straight down.
    Angle(f64),
}

/// One chain re-angled by [`flare`].
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FlaredChain {
    /// The hinge transform that was rotated.
    pub hinge: String,
    pub leaf: String,
    pub before_deg: f64,
    pub after_deg: f64,
}

/// Result of [`flare`].
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FlareReport {
    pub chains: Vec<FlaredChain>,
}

/// Re-angle the chains of PhysBone `file_id` toward (or away from) straight down: for each chain,
/// the transform at `hinge_depth` below the root (1 = the root's children — a skirt's hinge ring;
/// 0 = the root itself, for a component rooted on the chain's first bone) is rotated, in avatar
/// space, about the axis perpendicular to its chain direction and −Y, so that the hinge→leaf
/// direction makes the `target` angle with −Y. Only that one local rotation changes per chain
/// (`local' = R_parent⁻¹ · Δ · R_parent · local`), so the chain swings rigidly like a panel about
/// its hinge, and the skinned mesh follows: a funnel skirt hugs the legs, a drooping tail lifts.
/// This is a rest-pose edit — PhysBone takes the new pose as rest. Chains already within 0.01°
/// of the target are left alone.
pub fn flare(
    rw: &mut PrefabRewriter,
    file_id: i64,
    target: FlareTarget,
    hinge_depth: usize,
) -> Result<FlareReport> {
    match target {
        FlareTarget::Scale(s) if !(s.is_finite() && s >= 0.0) => {
            bail!("flare scale must be ≥ 0 (got {s})")
        }
        FlareTarget::Angle(a) if !(a.is_finite() && (0.0..=180.0).contains(&a)) => {
            bail!("flare angle must be 0..180 degrees (got {a})")
        }
        _ => {}
    }
    let spec = spec_of(rw.scene(), file_id)?;
    let root = root_transform(rw.scene(), &spec).context("PhysBone has no root transform")?;
    let chains_t = chains(rw.scene(), &spec);
    if chains_t.is_empty() {
        bail!("PhysBone &{file_id} drives no chain");
    }
    let down = crate::math::Vec3::new(0.0, -1.0, 0.0);
    // Plan every hinge rotation from the *current* scene (chains sharing a hinge are grouped: the
    // hinge is rotated once, aimed by its first chain), then apply.
    let scene = rw.scene();
    let mut planned: Vec<(i64, String, String, f64, f64, crate::math::Quat)> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for c in &chains_t {
        // The hinge: walk up from the leaf to `hinge_depth` below the root.
        let mut path_from_root: Vec<i64> = Vec::new();
        let mut cur = *c.last().unwrap();
        while cur != root {
            path_from_root.push(cur);
            match scene.transforms.get(&cur) {
                Some(tr) if tr.parent != 0 => cur = tr.parent,
                _ => break,
            }
        }
        path_from_root.reverse(); // root's child first
        let hinge = if hinge_depth == 0 {
            root
        } else {
            match path_from_root.get(hinge_depth - 1) {
                Some(h) => *h,
                None => continue,
            }
        };
        if !seen.insert(hinge) {
            continue;
        }
        let leaf = *c.last().unwrap();
        let hinge_w = scene.world(hinge);
        let d = scene.world(leaf).position - hinge_w.position;
        let len = d.length();
        if len < 1e-9 {
            continue;
        }
        let dn = d.scale(1.0 / len);
        let before = dn.dot(down).clamp(-1.0, 1.0).acos().to_degrees();
        let after = match target {
            FlareTarget::Scale(s) => before * s,
            FlareTarget::Angle(a) => a,
        };
        let delta_deg = before - after; // rotate d toward down by this much
        if delta_deg.abs() < 0.01 {
            continue;
        }
        // Axis that takes d toward down (right-hand rule); if d ∥ down pick any perpendicular.
        let mut axis = dn.cross(down);
        if axis.length() < 1e-6 {
            axis = if dn.x.abs() < 0.9 {
                crate::math::Vec3::new(1.0, 0.0, 0.0)
            } else {
                crate::math::Vec3::new(0.0, 0.0, 1.0)
            }
            .cross(dn);
        }
        let axis = axis.normalized();
        let delta = crate::math::Quat::axis_angle(axis, delta_deg);
        let tr = &scene.transforms[&hinge];
        let parent_rot = scene.world(tr.parent).rotation;
        let new_local =
            (parent_rot.inverse() * delta * parent_rot * tr.local.rotation).normalized();
        planned.push((
            hinge,
            scene.path_of(hinge),
            scene.path_of(leaf),
            before,
            after,
            new_local,
        ));
    }
    let mut out = Vec::new();
    for (hinge, hinge_path, leaf_path, before, after, q) in planned {
        for (k, v) in [("x", q.x), ("y", q.y), ("z", q.z), ("w", q.w)] {
            rw.set_scalar(
                hinge,
                &format!("m_LocalRotation/{k}"),
                avatar_unity_yaml::Scalar::Float(v),
            )?;
        }
        out.push(FlaredChain {
            hinge: hinge_path,
            leaf: leaf_path,
            before_deg: before,
            after_deg: after,
        });
    }
    Ok(FlareReport { chains: out })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk3::PhysBoneSpec;

    /// A prefab with `Root/Hips/{Hair/{A/A2, B/B2/B3}, Leg}` and a PhysBone rooted on Hair
    /// ignoring nothing, plus a collider on Leg.
    fn prefab() -> String {
        let mut s = String::from("%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n");
        // (go, tr, name, parent tr, children trs, local y)
        type Node = (i64, i64, &'static str, i64, &'static [i64], f64);
        let nodes: &[Node] = &[
            (100, 400, "Root", 0, &[401], 0.0),
            (101, 401, "Hips", 400, &[402, 410], 1.0),
            (102, 402, "Hair", 401, &[403, 405], 0.5),
            (103, 403, "A", 402, &[404], 0.1),
            (104, 404, "A2", 403, &[], 0.1),
            (105, 405, "B", 402, &[406], 0.1),
            (106, 406, "B2", 405, &[407], 0.2),
            (107, 407, "B3", 406, &[], 0.2),
            (110, 410, "Leg", 401, &[], -0.4),
        ];
        for (go, tr, name, parent, kids, y) in nodes {
            let mut comps = format!("  - component: {{fileID: {tr}}}\n");
            if *go == 102 {
                comps.push_str("  - component: {fileID: 900}\n");
            }
            if *go == 110 {
                comps.push_str("  - component: {fileID: 901}\n");
            }
            s.push_str(&format!(
                "--- !u!1 &{go}\nGameObject:\n  m_Component:\n{comps}  m_Name: {name}\n  m_IsActive: 1\n"
            ));
            let kids_yaml = if kids.is_empty() {
                " []".to_string()
            } else {
                let mut k = String::new();
                for c in *kids {
                    k.push_str(&format!("\n  - {{fileID: {c}}}"));
                }
                k
            };
            s.push_str(&format!(
                "--- !u!4 &{tr}\nTransform:\n  m_GameObject: {{fileID: {go}}}\n  m_LocalRotation: {{x: 0, y: 0, z: 0, w: 1}}\n  m_LocalPosition: {{x: 0, y: {y}, z: 0}}\n  m_LocalScale: {{x: 1, y: 1, z: 1}}\n  m_Children:{kids_yaml}\n  m_Father: {{fileID: {parent}}}\n"
            ));
        }
        let mut pb = PhysBoneSpec::new(102);
        pb.pull = 0.1;
        pb.colliders = vec![901];
        s.push_str(&format!("--- !u!114 &900\n{}", pb.to_body()));
        s.push_str(&format!(
            "--- !u!114 &901\nMonoBehaviour:\n  m_GameObject: {{fileID: 110}}\n  m_Script: {}\n  shapeType: 0\n",
            crate::sdk3::VRC_PHYS_BONE_COLLIDER.render()
        ));
        s
    }

    #[test]
    fn list_and_chains() {
        let rw = PrefabRewriter::new(&prefab()).unwrap();
        let all = list(rw.scene());
        assert_eq!(all.len(), 1);
        let pb = &all[0];
        assert_eq!(pb.root, "Hips/Hair");
        assert_eq!(pb.colliders, vec!["Hips/Leg"]);
        // Two chains: A→A2 (2 bones), B→B2→B3 (3 bones); root Hair itself not simulated.
        let leaves: Vec<_> = pb
            .chains
            .iter()
            .map(|c| (c.leaf.as_str(), c.bones))
            .collect();
        assert_eq!(
            leaves,
            vec![("Hips/Hair/A/A2", 2), ("Hips/Hair/B/B2/B3", 3)]
        );
        assert_eq!(pb.transforms, 5);
        assert!((pb.chains[1].length - 0.4).abs() < 1e-9);
        assert_eq!(find(rw.scene(), "Hair").unwrap(), 900);
        assert_eq!(find(rw.scene(), "Hips/Hair").unwrap(), 900);
        assert_eq!(find(rw.scene(), "900").unwrap(), 900);
        assert!(find(rw.scene(), "Leg").is_err());
    }

    #[test]
    fn set_round_trips_and_applies_curves() {
        let mut rw = PrefabRewriter::new(&prefab()).unwrap();
        let t = Tuning {
            pull: Some(0.3),
            pull_curve: Some(Curve::parse("0:0.5,1:1").unwrap()),
            spring_curve: Some(Curve::parse("0:1,0.5:0.8,1:0.4").unwrap()),
            limit_type: Some(LimitType::Angle),
            max_angle_x: Some(60.0),
            ..Default::default()
        };
        let info = set(&mut rw, 900, &t, &["A".into()], &[], &[], &[]).unwrap();
        assert_eq!(info.pull, 0.3);
        assert_eq!(info.pull_curve, vec![(0.0, 0.5), (1.0, 1.0)]);
        assert_eq!(info.ignore, vec!["Hips/Hair/A"]);
        // Only chain B remains, and it starts at B (root has one live child but multiChild=0
        // with a single child → root is the first bone).
        assert_eq!(info.chains.len(), 1);
        // Re-parse: the written text reads back identically.
        let again = PrefabRewriter::new(rw.text()).unwrap();
        let spec = spec_of(again.scene(), 900).unwrap();
        assert_eq!(spec.pull_curve, Curve(vec![(0.0, 0.5), (1.0, 1.0)]));
        assert_eq!(spec.spring_curve.0.len(), 3);
        assert_eq!(spec.limit_type, LimitType::Angle);
        assert_eq!(spec.max_angle_x, 60.0);
        assert_eq!(spec.ignore_transforms, vec![403]);
        assert_eq!(spec.colliders, vec![901]);
        // Body is stable under a second round trip.
        assert_eq!(
            spec.to_body(),
            PhysBoneSpec::from_yaml(&again.scene().doc(900).unwrap().body).to_body()
        );
        // Curve text is linear: middle key of the spring curve has secant slopes.
        assert!(rw.text().contains("inSlope: -0.4\n      outSlope: -0.8"));
    }

    #[test]
    fn split_moves_chain_to_own_component() {
        let mut rw = PrefabRewriter::new(&prefab()).unwrap();
        let t = Tuning {
            gravity: Some(0.2),
            ..Default::default()
        };
        let out = split(&mut rw, 900, &["B".into()], &t).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "Hips/Hair/B");
        assert_eq!(out[0].bones, 3);
        let again = PrefabRewriter::new(rw.text()).unwrap();
        let all = list(again.scene());
        assert_eq!(all.len(), 2);
        let parent = all.iter().find(|p| p.file_id == 900).unwrap();
        assert_eq!(parent.ignore, vec!["Hips/Hair/B"]);
        assert_eq!(parent.chains.len(), 1);
        let child = all.iter().find(|p| p.file_id != 900).unwrap();
        assert_eq!(child.object, "Hips/Hair/B");
        assert_eq!(child.root, "Hips/Hair/B");
        assert_eq!(child.pull, 0.1); // inherited
        assert_eq!(child.gravity, 0.2); // override
        assert_eq!(child.colliders, vec!["Hips/Leg"]);
        assert_eq!(child.chains[0].bones, 3);
        // Deterministic id.
        assert_eq!(
            child.file_id,
            crate::rewrite::derive_file_id("physbone:Hips/Hair/B")
        );
        // Splitting an already-ignored chain is refused.
        let mut rw2 = PrefabRewriter::new(rw.text()).unwrap();
        assert!(split(&mut rw2, 900, &["B".into()], &Tuning::default()).is_err());
    }

    #[test]
    fn flare_re_angles_hinges_toward_down() {
        // Tilt chain A: give A a rotation so A→A2 (local +Y·0.1) points 60° from −Y, i.e. A2
        // sits at (sin60·0.1, −cos60·0.1) relative to A once we look at world space. The fixture
        // offsets are +Y (up), so first make A2's offset hang: pos (0, −0.1, 0), then rotate A by
        // 60° about Z: −Y → (sin60, −cos60, 0)... simplest: rotate A about Z by −60° so its −Y
        // axis swings toward +X.
        let text = prefab()
            .replace(
                "--- !u!4 &403\nTransform:\n  m_GameObject: {fileID: 103}\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}",
                "--- !u!4 &403\nTransform:\n  m_GameObject: {fileID: 103}\n  m_LocalRotation: {x: 0, y: 0, z: -0.5, w: 0.8660254}",
            )
            .replace(
                "--- !u!4 &404\nTransform:\n  m_GameObject: {fileID: 104}\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}\n  m_LocalPosition: {x: 0, y: 0.1, z: 0}",
                "--- !u!4 &404\nTransform:\n  m_GameObject: {fileID: 104}\n  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}\n  m_LocalPosition: {x: 0, y: -0.1, z: 0}",
            );
        let mut rw = PrefabRewriter::new(&text).unwrap();
        let before = info(rw.scene(), 900).unwrap();
        let a = before
            .chains
            .iter()
            .find(|c| c.leaf.ends_with("A2"))
            .unwrap();
        assert!((a.flare_deg - 60.0).abs() < 0.01, "{}", a.flare_deg);
        let r = flare(&mut rw, 900, FlareTarget::Angle(10.0), 1).unwrap();
        let ra = r.chains.iter().find(|c| c.leaf.ends_with("A2")).unwrap();
        assert!((ra.before_deg - 60.0).abs() < 0.01 && (ra.after_deg - 10.0).abs() < 1e-9);
        assert_eq!(ra.hinge, "Hips/Hair/A");
        let again = PrefabRewriter::new(rw.text()).unwrap();
        let after = info(again.scene(), 900).unwrap();
        let a2 = after
            .chains
            .iter()
            .find(|c| c.leaf.ends_with("A2"))
            .unwrap();
        assert!((a2.flare_deg - 10.0).abs() < 0.01, "{}", a2.flare_deg);
        // Chain B pointed straight up (180°, axis arbitrary) and was brought to 10° too; a
        // second pass with Scale(0.5) halves it.
        let rb = r.chains.iter().find(|c| c.leaf.ends_with("B3")).unwrap();
        assert!((rb.before_deg - 180.0).abs() < 0.01 && (rb.after_deg - 10.0).abs() < 1e-9);
        let mut rw2 = PrefabRewriter::new(rw.text()).unwrap();
        let r2 = flare(&mut rw2, 900, FlareTarget::Scale(0.5), 1).unwrap();
        let rb = r2.chains.iter().find(|c| c.leaf.ends_with("B3")).unwrap();
        assert!((rb.after_deg - 5.0).abs() < 1e-6, "{rb:?}");
        let again2 = PrefabRewriter::new(rw2.text()).unwrap();
        let b = info(again2.scene(), 900).unwrap();
        let b3 = b.chains.iter().find(|c| c.leaf.ends_with("B3")).unwrap();
        assert!((b3.flare_deg - 5.0).abs() < 0.01, "{}", b3.flare_deg);
        assert!(flare(&mut rw2, 900, FlareTarget::Scale(-1.0), 1).is_err());
    }

    #[test]
    fn stretch_by_adds_the_same_length_to_every_chain() {
        // Chain A: A→A2 (0.1 scaled part); chain B: B→B2→B3 (0.4 scaled part). +0.2 each.
        let mut rw = PrefabRewriter::new(&prefab()).unwrap();
        let r = stretch_with(&mut rw, 900, StretchAmount::By(0.2), 2).unwrap();
        assert_eq!(r.by, Some(0.2));
        for (leaf, b, a) in &r.chains {
            assert!((a - b - 0.2).abs() < 1e-9, "{leaf}: {b} -> {a}");
        }
        // Shortening past zero is refused.
        let mut rw2 = PrefabRewriter::new(&prefab()).unwrap();
        assert!(stretch_with(&mut rw2, 900, StretchAmount::By(-0.5), 2).is_err());
    }

    #[test]
    fn stretch_scales_offsets_below_hinge() {
        let mut rw = PrefabRewriter::new(&prefab()).unwrap();
        let r = stretch(&mut rw, 900, 1.5, 2).unwrap();
        // A (hinge) stays; A2, B2, B3 move.
        let paths: Vec<_> = r.bones.iter().map(|b| b.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["Hips/Hair/A/A2", "Hips/Hair/B/B2", "Hips/Hair/B/B2/B3"]
        );
        let b = r
            .chains
            .iter()
            .find(|c| c.0 == "Hips/Hair/B/B2/B3")
            .unwrap();
        assert!(
            (b.1 - 0.4).abs() < 1e-9 && (b.2 - 0.6).abs() < 1e-9,
            "{b:?}"
        );
        let again = PrefabRewriter::new(rw.text()).unwrap();
        assert!((again.scene().transforms[&407].local.position.y - 0.3).abs() < 1e-9);
        assert!((again.scene().transforms[&405].local.position.y - 0.1).abs() < 1e-9);
        assert!(stretch(&mut rw, 900, 0.0, 2).is_err());
        assert!(stretch(&mut rw, 900, 1.2, 0).is_err());
    }
}
