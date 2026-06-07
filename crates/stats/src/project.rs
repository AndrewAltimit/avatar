//! Component- and geometry-side performance stats from a Unity project: walk the prefabs/scenes,
//! find the ones that define an avatar (carry a VRC Avatar Descriptor), and rank the avatar against
//! VRChat's limits.
//!
//! Three things are measured per avatar:
//! - **Components**, counted by stable Unity **class id** (renderers, particles, lights, …) and —
//!   for the VRChat dynamics `MonoBehaviour`s, like `avatar-vrc-descriptor` — **structurally** by
//!   their distinctive serialized fields (`collisionTags` ⇒ contact, `bonesAsSpheres`/`insideBounds`
//!   ⇒ collider, `endpointPosition`/`multiChildType` ⇒ PhysBone).
//! - **Triangles**, by resolving each renderer's mesh reference (`m_Mesh` guid) through the
//!   project's `.meta` index to its **source FBX**, loading it, and summing its triangles. Distinct
//!   source files are counted once (so an avatar split into several renderers over one body FBX
//!   counts that FBX's geometry once). Meshes that don't resolve to an FBX (built-in/baked `.asset`
//!   meshes) are reported as unresolved rather than silently dropped.
//! - **Bones**, from the union of distinct `m_Bones` transforms across the skinned mesh renderers —
//!   exactly what VRChat counts, straight from the prefab.
//!
//! - **PhysBone affected-transform & collision-check counts**, by walking the transform hierarchy
//!   under each PhysBone's `rootTransform` (its descendants, minus `ignoreTransforms` subtrees, plus
//!   an endpoint per chain tip when `endpointPosition` is set) and multiplying by the colliders
//!   assigned to that PhysBone — VRChat's two Avatar-Dynamics cost metrics.
//!
//! - **Total particle count**, by parsing each `ParticleSystem`'s emission modules and estimating
//!   its live-particle ceiling as `min(maxParticles, ceil(rate × lifetime))`, summed across systems.
//! - **Constraint count & depth**, by tallying Unity built-in constraints (class ids 320–325) and
//!   VRChat constraint `MonoBehaviour`s (recognized structurally), and walking the constraint→source
//!   graph for the longest chain of constraints that drive one another.
//!
//! Scope: each avatar is counted over the whole file that declares it — accurate for the common
//! "one avatar per prefab" case.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use avatar_fbx::FbxDocument;
use avatar_unity_yaml::{
    UnityFile, Yaml, build_guid_index, field_f64, field_i64, meta_path, relative, walk_assets,
};
use avatar_vpm::UnityProject;
use avatar_vrc_descriptor::{AssetRef, VrcAsset, extract};

use crate::texture::estimate_bytes;
use crate::{Metric, MetricStat, PerfReport, Platform};

/// Rank-affecting metrics the project view still does not measure. Everything the file layer can
/// reproduce — including mesh-particle polygons and the particle trail/collision flags — is now
/// measured, so this list is empty; an individual metric still *adds* a note when its inputs were
/// incomplete (a lower bound), via [`Counts::into_report`].
const PROJECT_NOT_EVALUATED: &[&str] = &[];

/// Maps each asset GUID to the file it describes (`.meta` minus the `.meta` suffix).
type GuidIndex = HashMap<String, PathBuf>;
/// Caches triangle counts per source FBX path, so a body FBX shared by several renderers /
/// several avatars in one project is loaded once.
type TriangleCache = HashMap<PathBuf, Option<u64>>;
/// Caches the texture GUIDs referenced by each material (keyed by material GUID).
type MaterialCache = HashMap<String, Vec<String>>;
/// Caches estimated `(PC, Android)` VRAM bytes per texture (keyed by texture GUID; `None` =
/// couldn't estimate).
type TextureCache = HashMap<String, Option<(u64, u64)>>;

/// The shared, per-project resolution caches threaded through every avatar in one analysis.
#[derive(Default)]
struct Resolver {
    triangles: TriangleCache,
    materials: MaterialCache,
    textures: TextureCache,
}

/// Compute performance stats for every avatar in the Unity project at or above `path`. Returns one
/// [`PerfReport`] per prefab/scene that contains a VRC Avatar Descriptor; empty if none are found.
pub fn analyze_project(path: &Path) -> Result<Vec<PerfReport>> {
    let project = UnityProject::discover(path)?;

    let assets = project.assets_dir();
    let files = if assets.is_dir() {
        walk_assets(&assets)
    } else {
        Vec::new()
    };

    let guids = build_guid_index(&files);
    let mut resolver = Resolver::default();

    let mut reports = Vec::new();
    for file_path in &files {
        if !is_scene_or_prefab(file_path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file_path) else {
            continue;
        };
        let Ok(file) = UnityFile::parse(&text) else {
            continue;
        };
        let Some(avatar) = avatar_label(&file) else {
            continue; // not an avatar-defining file.
        };

        let counts = Counts::of(&file);
        let geometry = resolve_geometry(&file, &guids, &mut resolver.triangles);
        let texture_memory = resolve_texture_memory(&file, &guids, &mut resolver);
        let bones = bone_count(&file);
        let dynamics = resolve_physbone_dynamics(&file);
        let particles = resolve_particles(&file, &guids, &mut resolver.triangles);
        let constraints = resolve_constraints(&file);
        let source = format!("{} ({avatar})", relative(&project.root, file_path));
        reports.push(counts.into_report(
            source,
            &geometry,
            &texture_memory,
            bones,
            &dynamics,
            &particles,
            &constraints,
        ));
    }

    Ok(reports)
}

/// The avatar's name if this file declares one (carries a VRC Avatar Descriptor), else `None`.
fn avatar_label(file: &UnityFile) -> Option<String> {
    extract(file).into_iter().find_map(|asset| match asset {
        VrcAsset::Descriptor(d) => Some(
            d.asset_name
                .unwrap_or_else(|| "Avatar Descriptor".to_string()),
        ),
        _ => None,
    })
}

/// The triangle side of an avatar, resolved through the project's mesh references.
struct Geometry {
    /// Triangles summed across the distinct source FBX files the avatar's meshes resolve to.
    triangles: u64,
    /// Mesh references that did not resolve to a readable FBX (built-in / baked `.asset` meshes, or
    /// a missing guid) — their triangles are not included, so `triangles` is a lower bound.
    unresolved_meshes: usize,
}

/// Resolve every renderer's mesh reference to its source FBX and sum triangles. Skinned mesh
/// renderers (class 137) carry `m_Mesh` directly; mesh renderers (class 23) read it from the
/// sibling mesh filter (class 33), so both `m_Mesh`-bearing classes are gathered.
fn resolve_geometry(file: &UnityFile, guids: &GuidIndex, cache: &mut TriangleCache) -> Geometry {
    let mut seen_files: HashSet<PathBuf> = HashSet::new();
    let mut triangles = 0u64;
    let mut unresolved = 0usize;

    for doc in &file.documents {
        if !matches!(doc.class_id, 137 | 33) {
            continue;
        }
        let mesh = AssetRef::parse(&doc.body["m_Mesh"]);
        if !mesh.is_set() {
            continue; // no mesh assigned on this renderer.
        }
        match mesh.guid.as_deref().and_then(|g| guids.get(g)) {
            Some(path) if is_fbx(path) => {
                // Count each distinct source FBX once (its triangles cover all its sub-meshes).
                if seen_files.insert(path.clone()) {
                    match fbx_triangles(path, cache) {
                        Some(n) => triangles += n,
                        None => unresolved += 1, // FBX failed to load/parse.
                    }
                }
            }
            _ => unresolved += 1, // unknown guid, or a non-FBX (baked/built-in) mesh.
        }
    }

    Geometry {
        triangles,
        unresolved_meshes: unresolved,
    }
}

/// Triangle count of an FBX, loaded at most once per path.
fn fbx_triangles(path: &Path, cache: &mut TriangleCache) -> Option<u64> {
    if let Some(cached) = cache.get(path) {
        return *cached;
    }
    let count = (|| {
        let doc = FbxDocument::load(path).ok()?;
        let meshes = doc.meshes().ok()?;
        Some(meshes.iter().map(|m| (m.indices.len() / 3) as u64).sum())
    })();
    cache.insert(path.to_path_buf(), count);
    count
}

/// VRChat's bone count: distinct bone transforms across all skinned mesh renderers (`m_Bones`).
fn bone_count(file: &UnityFile) -> u64 {
    let mut bones: HashSet<i64> = HashSet::new();
    for doc in &file.documents {
        if doc.class_id != 137 {
            continue;
        }
        let Some(list) = doc.body["m_Bones"].as_vec() else {
            continue;
        };
        for bone in list {
            match field_i64(bone, "fileID") {
                Some(id) if id != 0 => {
                    bones.insert(id);
                }
                _ => {}
            }
        }
    }
    bones.len() as u64
}

fn is_fbx(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("fbx"))
}

/// The transform hierarchy of a prefab/scene: for each Transform (`m_GameObject`/`m_Children`),
/// the children it parents and the transform sitting on each GameObject. Built once per file so the
/// PhysBone walk can resolve a `rootTransform` (or a root-less PhysBone's own GameObject) to its
/// descendant chain.
struct Hierarchy {
    /// Transform fileID → its child transform fileIDs (from `m_Children`).
    children: HashMap<i64, Vec<i64>>,
    /// GameObject fileID → the fileID of the Transform attached to it (from `m_GameObject`).
    transform_of_gameobject: HashMap<i64, i64>,
    /// Transform fileID → the GameObject fileID it sits on (the reverse map, for source refs).
    gameobject_of_transform: HashMap<i64, i64>,
}

impl Hierarchy {
    fn of(file: &UnityFile) -> Self {
        let mut children = HashMap::new();
        let mut transform_of_gameobject = HashMap::new();
        let mut gameobject_of_transform = HashMap::new();
        for doc in &file.documents {
            // Transform (4) and RectTransform (224) both carry the hierarchy fields.
            if !matches!(doc.class_id, 4 | 224) {
                continue;
            }
            if let Some(go) = field_i64(&doc.body["m_GameObject"], "fileID") {
                transform_of_gameobject.insert(go, doc.file_id);
                gameobject_of_transform.insert(doc.file_id, go);
            }
            let kids: Vec<i64> = doc.body["m_Children"]
                .as_vec()
                .map(|list| {
                    list.iter()
                        .filter_map(|c| field_i64(c, "fileID"))
                        .filter(|&id| id != 0)
                        .collect()
                })
                .unwrap_or_default();
            children.insert(doc.file_id, kids);
        }
        Hierarchy {
            children,
            transform_of_gameobject,
            gameobject_of_transform,
        }
    }

    /// `true` if `id` is a transform we know about (so a root reference into it is resolvable).
    fn has(&self, id: i64) -> bool {
        self.children.contains_key(&id)
    }

    /// The GameObject a transform sits on, if known.
    fn gameobject_of_transform(&self, transform_id: i64) -> Option<i64> {
        self.gameobject_of_transform.get(&transform_id).copied()
    }

    /// Count the transforms in the subtree rooted at `id` (including `id`) and the chain tips
    /// (leaves) within it, skipping any transform in `ignored` and its descendants.
    fn walk(&self, id: i64, ignored: &HashSet<i64>, nodes: &mut u64, leaves: &mut u64) {
        self.walk_inner(id, ignored, &mut HashSet::new(), nodes, leaves);
    }

    /// Recursion body of [`walk`](Self::walk), carrying a `seen` set so a malformed prefab whose
    /// `m_Children` lists form a cycle (impossible in a valid Unity tree, but not in hand-crafted or
    /// corrupted YAML) terminates instead of overflowing the stack — a visited transform is treated
    /// as an already-counted leaf.
    fn walk_inner(
        &self,
        id: i64,
        ignored: &HashSet<i64>,
        seen: &mut HashSet<i64>,
        nodes: &mut u64,
        leaves: &mut u64,
    ) {
        if !seen.insert(id) {
            return;
        }
        *nodes += 1;
        let kids: Vec<i64> = self
            .children
            .get(&id)
            .map(|cs| {
                cs.iter()
                    .copied()
                    .filter(|c| {
                        !ignored.contains(c) && self.children.contains_key(c) && !seen.contains(c)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if kids.is_empty() {
            *leaves += 1; // a chain tip — gets an endpoint bone when endpointPosition is set.
            return;
        }
        for child in kids {
            self.walk_inner(child, ignored, seen, nodes, leaves);
        }
    }
}

/// VRChat's two Avatar-Dynamics cost metrics, summed over the avatar's PhysBones.
struct PhysBoneDynamics {
    /// Total transforms moved by all PhysBones (each PhysBone's descendant chain under its root,
    /// minus ignored subtrees, plus an endpoint per chain tip when `endpointPosition` is set).
    affected_transforms: u64,
    /// Σ over PhysBones of `affected_transforms × assigned colliders` — the per-frame collision
    /// checks VRChat budgets.
    collision_checks: u64,
    /// PhysBones whose `rootTransform` couldn't be resolved in this file (e.g. a stripped nested-
    /// prefab transform) — they contribute nothing, so both totals are a lower bound.
    unresolved: usize,
}

/// Resolve PhysBone affected-transform and collision-check counts by walking the transform hierarchy
/// under each PhysBone's root. A PhysBone is recognized structurally (like the component tally) by
/// its `endpointPosition`/`multiChildType` fields; its root is `rootTransform` when set, else the
/// transform on the PhysBone's own GameObject.
///
/// This is an **estimate**: it follows VRChat's documented model (root transform is the immovable
/// anchor and is not counted; each remaining descendant is one affected transform; a non-zero
/// `endpointPosition` appends one endpoint bone per chain tip) but does not model every
/// `multiChildType` nuance, and can't see transforms stripped out of a nested-prefab override.
fn resolve_physbone_dynamics(file: &UnityFile) -> PhysBoneDynamics {
    let hierarchy = Hierarchy::of(file);
    let mut affected_transforms = 0u64;
    let mut collision_checks = 0u64;
    let mut unresolved = 0usize;

    for doc in &file.documents {
        if doc.class_id != 114 || !is_physbone(&doc.body) {
            continue;
        }

        // Root: explicit `rootTransform`, else the transform on the PhysBone's own GameObject.
        let root = match field_i64(&doc.body["rootTransform"], "fileID") {
            Some(id) if id != 0 => id,
            _ => {
                let go = field_i64(&doc.body["m_GameObject"], "fileID").unwrap_or(0);
                hierarchy
                    .transform_of_gameobject
                    .get(&go)
                    .copied()
                    .unwrap_or(0)
            }
        };
        if root == 0 || !hierarchy.has(root) {
            unresolved += 1;
            continue;
        }

        let ignored: HashSet<i64> = doc.body["ignoreTransforms"]
            .as_vec()
            .map(|list| {
                list.iter()
                    .filter_map(|t| field_i64(t, "fileID"))
                    .filter(|&id| id != 0)
                    .collect()
            })
            .unwrap_or_default();

        let (mut nodes, mut leaves) = (0u64, 0u64);
        hierarchy.walk(root, &ignored, &mut nodes, &mut leaves);
        // The root is the anchor and isn't simulated; its descendants are the moving bones.
        let mut affected = nodes.saturating_sub(1);
        if has_endpoint(&doc.body) {
            affected += leaves; // one endpoint bone appended to each chain tip.
        }

        affected_transforms += affected;
        collision_checks += affected * collider_count(&doc.body);
    }

    PhysBoneDynamics {
        affected_transforms,
        collision_checks,
        unresolved,
    }
}

/// `true` if this `MonoBehaviour` body is a VRChat PhysBone (same structural test as the count).
fn is_physbone(body: &Yaml) -> bool {
    !body["endpointPosition"].is_badvalue() || !body["multiChildType"].is_badvalue()
}

/// `true` if a PhysBone's `endpointPosition` is non-zero (so VRChat appends an endpoint bone).
fn has_endpoint(body: &Yaml) -> bool {
    let ep = &body["endpointPosition"];
    ["x", "y", "z"]
        .iter()
        .any(|axis| avatar_unity_yaml::field_f64(ep, axis).is_some_and(|v| v != 0.0))
}

/// Number of colliders assigned to a PhysBone (`colliders` entries that reference something).
fn collider_count(body: &Yaml) -> u64 {
    body["colliders"].as_vec().map_or(0, |list| {
        list.iter().filter(|c| AssetRef::parse(c).is_set()).count() as u64
    })
}

/// The estimated total live-particle count across the avatar's particle systems, plus the
/// mesh-particle polygon cost and the trail/collision flag tallies.
struct Particles {
    /// Σ over `ParticleSystem`s of `min(maxParticles, ceil(rate × lifetime))`.
    total: u64,
    /// Particle systems whose emission/init modules couldn't be parsed (no `InitialModule`, etc.) —
    /// they contribute nothing, so `total` is a lower bound.
    unparsed: usize,
    /// Σ over mesh-mode particle renderers of `mesh triangles × the sibling system's live-particle
    /// count` — the active polygons VRChat budgets for mesh particles.
    mesh_particle_triangles: u64,
    /// Particle systems with a `TrailModule.enabled: 1`.
    trail_systems: u64,
    /// Particle systems with a `CollisionModule.enabled: 1`.
    collision_systems: u64,
    /// Mesh-mode particle renderers whose mesh reference didn't resolve to a readable FBX — their
    /// polygons are omitted, so `mesh_particle_triangles` is a lower bound.
    unresolved_mesh_particles: usize,
}

/// Estimate the avatar's total live-particle ceiling. For each `ParticleSystem` (class 198) this
/// reproduces VRChat's own estimate, `min(maxNumParticles, ceil(rate × startLifetime))`, where the
/// rate is the constant scalar from `EmissionModule.rateOverTime` (+ `rateOverDistance`) and the
/// lifetime/cap come from `InitialModule`.
///
/// This is an **estimate**: it reads the *constant* (scalar) value of each curve-able field and so
/// approximates animated curves, bursts, and sub-emitters (which can spawn more) by their constant
/// term — a close ballpark, not an exact ceiling. A system whose modules can't be read at all is
/// **flagged** (counted in `unparsed`) rather than silently assumed to emit nothing.
///
/// It also tallies the **mesh-particle polygon cost** and the **trail/collision flags**. The
/// `ParticleSystem` (class 198) and its `ParticleSystemRenderer` (class 199) sit on the same
/// GameObject; they're paired by GameObject fileID. For each mesh-mode renderer
/// (`m_RenderMode: 4`), the polygon cost is the triangles of its `m_Mesh` (resolved to a source FBX
/// via the `.meta` index, like [`resolve_geometry`]) × the sibling system's live-particle count.
/// `TrailModule`/`CollisionModule` `enabled: 1` increment the flag counters.
fn resolve_particles(file: &UnityFile, guids: &GuidIndex, cache: &mut TriangleCache) -> Particles {
    let mut total = 0u64;
    let mut unparsed = 0usize;
    let mut trail_systems = 0u64;
    let mut collision_systems = 0u64;
    // GameObject fileID → that system's estimated live-particle count (for the renderer pairing).
    let mut particles_on_gameobject: HashMap<i64, u64> = HashMap::new();

    for doc in &file.documents {
        if doc.class_id != 198 {
            continue; // not a ParticleSystem.
        }
        let estimate = estimate_particles(&doc.body);
        match estimate {
            Some(n) => total += n,
            None => unparsed += 1,
        }
        if let Some(go) = field_i64(&doc.body["m_GameObject"], "fileID") {
            // Mesh-particle cost uses the system's live-particle count; 0 when unparseable.
            particles_on_gameobject.insert(go, estimate.unwrap_or(0));
        }
        if module_enabled(&doc.body["TrailModule"]) {
            trail_systems += 1;
        }
        if module_enabled(&doc.body["CollisionModule"]) {
            collision_systems += 1;
        }
    }

    // Mesh-particle polygons: pair each mesh-mode ParticleSystemRenderer (199) with its sibling
    // system (198) by GameObject, resolve the renderer's mesh to an FBX, and weight by particles.
    let mut mesh_particle_triangles = 0u64;
    let mut unresolved_mesh_particles = 0usize;
    for doc in &file.documents {
        if doc.class_id != 199 {
            continue; // not a ParticleSystemRenderer.
        }
        // m_RenderMode is a bare scalar (4 = Mesh); only Mesh mode contributes polygons.
        if doc.body["m_RenderMode"].as_i64() != Some(4) {
            continue;
        }
        // The mesh: `m_Mesh`, falling back to the first non-null `m_Meshes` entry.
        let mut mesh = AssetRef::parse(&doc.body["m_Mesh"]);
        if !mesh.is_set()
            && let Some(list) = doc.body["m_Meshes"].as_vec()
            && let Some(found) = list.iter().map(AssetRef::parse).find(|m| m.is_set())
        {
            mesh = found;
        }
        if !mesh.is_set() {
            continue; // no mesh assigned — nothing to count.
        }
        let particles = field_i64(&doc.body["m_GameObject"], "fileID")
            .and_then(|go| particles_on_gameobject.get(&go).copied())
            .unwrap_or(0);
        match mesh.guid.as_deref().and_then(|g| guids.get(g)) {
            Some(path) if is_fbx(path) => match fbx_triangles(path, cache) {
                Some(tris) => mesh_particle_triangles += tris.saturating_mul(particles),
                None => unresolved_mesh_particles += 1, // FBX failed to load/parse.
            },
            _ => unresolved_mesh_particles += 1, // unknown guid, or a non-FBX (baked/built-in) mesh.
        }
    }

    Particles {
        total,
        unparsed,
        mesh_particle_triangles,
        trail_systems,
        collision_systems,
        unresolved_mesh_particles,
    }
}

/// `true` if a particle sub-module (e.g. `TrailModule`/`CollisionModule`) is present with
/// `enabled: 1`.
fn module_enabled(module: &Yaml) -> bool {
    !module.is_badvalue() && field_i64(module, "enabled") == Some(1)
}

/// The live-particle ceiling of one `ParticleSystem` body, or `None` if its modules are unreadable.
///
/// `min(maxParticles, ceil(rate × lifetime))`. `maxParticles` (`InitialModule.maxNumParticles`)
/// caps any rate; without it the estimate would be unbounded, so a system missing `InitialModule`
/// is treated as unparseable.
fn estimate_particles(body: &Yaml) -> Option<u64> {
    let init = &body["InitialModule"];
    if init.is_badvalue() || init.is_null() {
        return None; // can't bound the system without its initial module.
    }
    // The hard cap Unity enforces on simultaneously-live particles.
    let max_particles = field_i64(init, "maxNumParticles").unwrap_or(1000).max(0) as u64;

    // Start lifetime + emission rate are MinMaxCurves; read their constant scalar term.
    let lifetime = curve_scalar(&init["startLifetime"]).unwrap_or(0.0).max(0.0);
    let emission = &body["EmissionModule"];
    let rate = curve_scalar(&emission["rateOverTime"])
        .unwrap_or(0.0)
        .max(0.0)
        + curve_scalar(&emission["rateOverDistance"])
            .unwrap_or(0.0)
            .max(0.0);

    let by_rate = (rate * lifetime).ceil();
    // Clamp the float to a sane u64 before comparing with the integer cap.
    let by_rate = if by_rate.is_finite() && by_rate >= 0.0 {
        by_rate.min(u64::MAX as f64) as u64
    } else {
        0
    };
    Some(by_rate.min(max_particles))
}

/// Read the constant scalar value of a Unity `MinMaxCurve`. Such a field serializes as
/// `{ scalar: <f>, minScalar: <f>, ... }` (newer) or as a bare number (older/simple). Returns the
/// `scalar` when present, else the node parsed as a plain number, else `None`.
fn curve_scalar(node: &Yaml) -> Option<f64> {
    if node.is_badvalue() || node.is_null() {
        return None;
    }
    if let Some(v) = field_f64(node, "scalar") {
        return Some(v);
    }
    node.as_f64()
        .or_else(|| node.as_i64().map(|i| i as f64))
        .or_else(|| node.as_str().and_then(|s| s.parse().ok()))
}

/// The avatar's constraint tally and the depth of its deepest constraint chain.
struct Constraints {
    /// Total constraints: Unity built-ins (classes 320–325) + VRChat constraint MonoBehaviours.
    count: u64,
    /// Longest chain of constraints that drive one another (a constraint whose source GameObject
    /// also carries a constraint). `None` when the chain couldn't be resolved (e.g. no usable
    /// source references), in which case only the count is reported.
    depth: Option<u64>,
}

/// Unity's built-in constraint class ids: Position/Rotation/Scale/Aim/Parent/LookAt.
fn is_unity_constraint(class_id: u32) -> bool {
    matches!(class_id, 320..=325)
}

/// `true` if a `MonoBehaviour` body is a VRChat constraint, recognized **structurally** (its script
/// guid changes across SDK versions) by VRCConstraintBase's distinctive serialized fields:
/// a `Sources` list of weighted `SourceTransform`s plus the `TargetTransform`/`AffectsPosition…`
/// family. We accept the presence of `Sources` together with a `TargetTransform`.
fn is_vrc_constraint(body: &Yaml) -> bool {
    !body["Sources"].is_badvalue() && !body["TargetTransform"].is_badvalue()
}

/// Tally constraints and estimate the deepest constraint→source chain. Counts Unity built-in
/// constraints (classes 320–325) and VRChat constraint MonoBehaviours (structural). Depth is the
/// longest path through the graph where a constraint's *source* GameObject also carries a
/// constraint — i.e. constraints feeding constraints; computed **conservatively** over the source
/// references resolvable within this file.
///
/// This is an **estimate**: it follows source GameObject references it can see. An empty `Sources`
/// list is a legitimate leaf (depth 1); but a source slot pointing at a transform absent from this
/// file (e.g. a stripped nested-prefab override) means the chain can't be trusted — when any such
/// slot is present the depth is reported as unknown (count only).
fn resolve_constraints(file: &UnityFile) -> Constraints {
    let hierarchy = Hierarchy::of(file);

    // For each constraint, the GameObject it lives on and the source GameObjects it reads from.
    // Keyed by the constraint's own fileID.
    let mut on_gameobject: HashMap<i64, i64> = HashMap::new();
    let mut sources: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut count = 0u64;
    // A non-zero source slot that didn't resolve to a GameObject in this file — its presence means
    // the chain is incomplete and depth can't be trusted.
    let mut unresolved_source = false;

    for doc in &file.documents {
        let is_constraint = is_unity_constraint(doc.class_id)
            || (doc.class_id == 114 && is_vrc_constraint(&doc.body));
        if !is_constraint {
            continue;
        }
        count += 1;

        let go = field_i64(&doc.body["m_GameObject"], "fileID").unwrap_or(0);
        on_gameobject.insert(doc.file_id, go);

        // Source GameObjects: Unity built-ins use `m_Sources[].sourceTransform` (a Transform ref,
        // resolved to its GameObject); VRChat constraints use `Sources[].SourceTransform`.
        let mut src_gos = Vec::new();
        for key in ["m_Sources", "Sources"] {
            let Some(list) = doc.body[key].as_vec() else {
                continue;
            };
            for entry in list {
                for tkey in ["sourceTransform", "SourceTransform"] {
                    let Some(tid) = field_i64(&entry[tkey], "fileID") else {
                        continue;
                    };
                    if tid == 0 {
                        // An empty/unfilled source slot — Unity serializes one per declared source
                        // even before it's wired. Treat as no edge, not as a resolution failure.
                        unresolved_source = true;
                        continue;
                    }
                    match hierarchy.gameobject_of_transform(tid) {
                        Some(src_go) => src_gos.push(src_go),
                        None => unresolved_source = true, // points outside this file.
                    }
                }
            }
        }
        sources.insert(doc.file_id, src_gos);
    }

    if count == 0 {
        return Constraints {
            count: 0,
            depth: Some(0),
        };
    }

    // Constraints attached to each GameObject, so a source GO maps to the constraints feeding it.
    let mut constraints_on_go: HashMap<i64, Vec<i64>> = HashMap::new();
    for (&cid, &go) in &on_gameobject {
        constraints_on_go.entry(go).or_default().push(cid);
    }

    // Depth = longest path following constraint -> source GO's constraints. Computed with memoized
    // DFS over the (small) constraint graph, guarding against cycles.
    let depth = if !unresolved_source {
        let mut memo: HashMap<i64, u64> = HashMap::new();
        let mut on_stack: HashSet<i64> = HashSet::new();
        let max = sources
            .keys()
            .map(|&cid| chain_depth(cid, &sources, &constraints_on_go, &mut memo, &mut on_stack))
            .max()
            .unwrap_or(1);
        Some(max)
    } else {
        // A source slot couldn't be resolved (empty or stripped): the chain is incomplete, so we
        // can't report a trustworthy depth — count only.
        None
    };

    Constraints { count, depth }
}

/// Length (in constraints) of the longest chain starting at constraint `cid` and following its
/// sources to other constraints. A standalone constraint has depth 1. Memoized; cycle-safe.
fn chain_depth(
    cid: i64,
    sources: &HashMap<i64, Vec<i64>>,
    constraints_on_go: &HashMap<i64, Vec<i64>>,
    memo: &mut HashMap<i64, u64>,
    on_stack: &mut HashSet<i64>,
) -> u64 {
    if let Some(&d) = memo.get(&cid) {
        return d;
    }
    if !on_stack.insert(cid) {
        return 1; // cycle: count this node once and stop.
    }

    let mut best = 1u64;
    if let Some(src_gos) = sources.get(&cid) {
        for src_go in src_gos {
            if let Some(feeders) = constraints_on_go.get(src_go) {
                for &feeder in feeders {
                    if feeder == cid {
                        continue;
                    }
                    let d = 1 + chain_depth(feeder, sources, constraints_on_go, memo, on_stack);
                    best = best.max(d);
                }
            }
        }
    }

    on_stack.remove(&cid);
    memo.insert(cid, best);
    best
}

/// The texture-memory side of an avatar (PC + Android), resolved through its materials' textures.
struct TextureMemory {
    /// Estimated PC VRAM (bytes) summed over the avatar's distinct textures.
    pc_bytes: u64,
    /// Estimated Android VRAM (bytes) — same textures, recompressed to ASTC/ETC2.
    android_bytes: u64,
    /// Distinct textures whose VRAM couldn't be estimated (unreadable/unsupported image, missing
    /// guid) — their bytes are omitted, so the totals are a lower bound.
    unresolved: usize,
}

/// Walk renderer → material guid → `.mat` texture refs → texture file, estimating per-platform VRAM
/// for each distinct texture (deduped across materials, so a texture shared by several materials
/// counts once — as VRChat counts unique texture objects).
fn resolve_texture_memory(
    file: &UnityFile,
    guids: &GuidIndex,
    resolver: &mut Resolver,
) -> TextureMemory {
    // Distinct material guids referenced by the avatar's renderers.
    let mut material_guids: HashSet<String> = HashSet::new();
    for doc in &file.documents {
        if !matches!(doc.class_id, 137 | 23) {
            continue;
        }
        if let Some(list) = doc.body["m_Materials"].as_vec() {
            for m in list {
                if let Some(g) = AssetRef::parse(m).guid {
                    material_guids.insert(g);
                }
            }
        }
    }

    // Distinct textures across those materials.
    let mut texture_guids: HashSet<String> = HashSet::new();
    for mat_guid in &material_guids {
        for tex in material_textures(mat_guid, guids, &mut resolver.materials) {
            texture_guids.insert(tex.clone());
        }
    }

    let mut pc_bytes = 0u64;
    let mut android_bytes = 0u64;
    let mut unresolved = 0usize;
    for tex_guid in &texture_guids {
        match texture_bytes(tex_guid, guids, &mut resolver.textures) {
            Some((pc, android)) => {
                pc_bytes += pc;
                android_bytes += android;
            }
            None => unresolved += 1,
        }
    }

    TextureMemory {
        pc_bytes,
        android_bytes,
        unresolved,
    }
}

/// The texture GUIDs a material (by guid) references, via its `m_SavedProperties.m_TexEnvs`.
fn material_textures<'a>(
    mat_guid: &str,
    guids: &GuidIndex,
    cache: &'a mut MaterialCache,
) -> &'a [String] {
    if !cache.contains_key(mat_guid) {
        let textures = guids
            .get(mat_guid)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| UnityFile::parse(&text).ok())
            .map(|file| texture_guids_of_material(&file))
            .unwrap_or_default();
        cache.insert(mat_guid.to_string(), textures);
    }
    &cache[mat_guid]
}

/// Extract every assigned texture's guid from a parsed `.mat` file's `m_TexEnvs`.
fn texture_guids_of_material(file: &UnityFile) -> Vec<String> {
    let mut out = Vec::new();
    for doc in &file.documents {
        if doc.class_id != 21 {
            continue; // not a Material.
        }
        let Some(envs) = doc.body["m_SavedProperties"]["m_TexEnvs"].as_vec() else {
            continue;
        };
        for entry in envs {
            // Each entry is a single-key map `{ _PropName: { m_Texture: {guid}, ... } }`.
            let Some(hash) = entry.as_hash() else {
                continue;
            };
            for value in hash.values() {
                if let Some(guid) =
                    avatar_unity_yaml::ref_guid(value, "m_Texture").filter(|s| !s.is_empty())
                {
                    out.push(guid.to_string());
                }
            }
        }
    }
    out
}

/// Estimated `(PC, Android)` VRAM (bytes) for a texture by guid, computed once and cached.
fn texture_bytes(
    tex_guid: &str,
    guids: &GuidIndex,
    cache: &mut TextureCache,
) -> Option<(u64, u64)> {
    if let Some(cached) = cache.get(tex_guid) {
        return *cached;
    }
    let bytes = guids.get(tex_guid).and_then(|image_path| {
        // The import settings live in the sibling `<image>.meta`; default settings if it's absent.
        let meta_text = std::fs::read_to_string(meta_path(image_path)).unwrap_or_default();
        // Both platforms share the same image read; differ only in compression format.
        let pc = estimate_bytes(image_path, &meta_text, Platform::Pc)?;
        let android = estimate_bytes(image_path, &meta_text, Platform::Android)?;
        Some((pc, android))
    });
    cache.insert(tex_guid.to_string(), bytes);
    bytes
}

/// Component tallies for one avatar.
#[derive(Default)]
struct Counts {
    skinned_meshes: u64,
    basic_meshes: u64,
    material_slots: u64,
    particle_systems: u64,
    lights: u64,
    audio_sources: u64,
    trail_renderers: u64,
    line_renderers: u64,
    cloths: u64,
    rigidbodies: u64,
    physics_colliders: u64,
    animators: u64,
    physbones: u64,
    physbone_colliders: u64,
    contacts: u64,
}

impl Counts {
    fn of(file: &UnityFile) -> Self {
        let mut c = Counts::default();
        for doc in &file.documents {
            // Stable Unity class ids (https://docs.unity3d.com/Manual/ClassIDReference.html).
            match doc.class_id {
                137 => {
                    c.skinned_meshes += 1; // SkinnedMeshRenderer
                    c.material_slots += materials_len(&doc.body);
                }
                23 => {
                    c.basic_meshes += 1; // MeshRenderer
                    c.material_slots += materials_len(&doc.body);
                }
                198 => c.particle_systems += 1, // ParticleSystem
                108 => c.lights += 1,           // Light
                82 => c.audio_sources += 1,     // AudioSource
                96 => c.trail_renderers += 1,   // TrailRenderer
                120 => c.line_renderers += 1,   // LineRenderer
                183 => c.cloths += 1,           // Cloth
                54 => c.rigidbodies += 1,       // Rigidbody
                // BoxCollider / MeshCollider / SphereCollider / CapsuleCollider.
                65 | 64 | 135 | 136 => c.physics_colliders += 1,
                95 => c.animators += 1, // Animator
                114 => c.classify_dynamics(&doc.body),
                _ => {}
            }
        }
        c
    }

    /// Recognize a VRChat dynamics `MonoBehaviour` by its distinctive serialized fields.
    fn classify_dynamics(&mut self, body: &Yaml) {
        if !body["collisionTags"].is_badvalue() {
            self.contacts += 1;
        } else if !body["bonesAsSpheres"].is_badvalue() || !body["insideBounds"].is_badvalue() {
            self.physbone_colliders += 1;
        } else if !body["endpointPosition"].is_badvalue() || !body["multiChildType"].is_badvalue() {
            self.physbones += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn into_report(
        self,
        source: String,
        geometry: &Geometry,
        texture_memory: &TextureMemory,
        bones: u64,
        dynamics: &PhysBoneDynamics,
        particles: &Particles,
        constraints: &Constraints,
    ) -> PerfReport {
        // Geometry first (matching the FBX report), then components.
        let mut stats = vec![
            MetricStat::new(Metric::Triangles, geometry.triangles),
            MetricStat::with_values(
                Metric::TextureMemory,
                texture_memory.pc_bytes,
                texture_memory.android_bytes,
            ),
            MetricStat::new(Metric::SkinnedMeshes, self.skinned_meshes),
            MetricStat::new(Metric::BasicMeshes, self.basic_meshes),
            MetricStat::new(Metric::MaterialSlots, self.material_slots),
            MetricStat::new(Metric::Bones, bones),
            MetricStat::new(Metric::PhysBoneComponents, self.physbones),
            MetricStat::new(Metric::PhysBoneColliders, self.physbone_colliders),
            MetricStat::new(Metric::PhysBoneTransforms, dynamics.affected_transforms),
            MetricStat::new(Metric::PhysBoneCollisionChecks, dynamics.collision_checks),
            MetricStat::new(Metric::Contacts, self.contacts),
            MetricStat::new(Metric::ParticleSystems, self.particle_systems),
            MetricStat::new(Metric::TotalParticles, particles.total),
            MetricStat::new(
                Metric::MeshParticlePolygons,
                particles.mesh_particle_triangles,
            ),
            MetricStat::new(Metric::ParticleTrailsEnabled, particles.trail_systems),
            MetricStat::new(
                Metric::ParticleCollisionEnabled,
                particles.collision_systems,
            ),
            MetricStat::new(Metric::Constraints, constraints.count),
            MetricStat::new(Metric::Lights, self.lights),
            MetricStat::new(Metric::AudioSources, self.audio_sources),
            MetricStat::new(Metric::TrailRenderers, self.trail_renderers),
            MetricStat::new(Metric::LineRenderers, self.line_renderers),
            MetricStat::new(Metric::Cloths, self.cloths),
            MetricStat::new(Metric::PhysicsColliders, self.physics_colliders),
            MetricStat::new(Metric::PhysicsRigidbodies, self.rigidbodies),
            MetricStat::new(Metric::Animators, self.animators),
        ];
        // Constraint depth is only ranked when the constraint graph resolved (otherwise count only).
        if let Some(depth) = constraints.depth {
            stats.push(MetricStat::new(Metric::ConstraintDepth, depth));
        }

        let mut not_evaluated: Vec<String> = PROJECT_NOT_EVALUATED
            .iter()
            .map(|s| s.to_string())
            .collect();
        // Triangles is a lower bound when some meshes didn't resolve to a readable FBX; say so.
        if geometry.unresolved_meshes > 0 {
            not_evaluated.push(format!(
                "Triangles of {} mesh(es) (built-in or non-FBX source)",
                geometry.unresolved_meshes
            ));
        }
        // Texture memory is a lower bound when some textures couldn't be read.
        if texture_memory.unresolved > 0 {
            not_evaluated.push(format!(
                "Texture Memory of {} texture(s) (unreadable image format)",
                texture_memory.unresolved
            ));
        }
        // PhysBone transform/collision counts are a lower bound when a root didn't resolve.
        if dynamics.unresolved > 0 {
            not_evaluated.push(format!(
                "PhysBone transforms of {} component(s) (unresolved root transform)",
                dynamics.unresolved
            ));
        }
        // Total particles is a lower bound when some systems' modules couldn't be parsed.
        if particles.unparsed > 0 {
            not_evaluated.push(format!(
                "Total Particles of {} system(s) (unparseable particle module)",
                particles.unparsed
            ));
        }
        // Mesh-particle polygons are a lower bound when a mesh-mode renderer's mesh didn't resolve.
        if particles.unresolved_mesh_particles > 0 {
            not_evaluated.push(format!(
                "Mesh Particle Polygons of {} renderer(s) (built-in or non-FBX source)",
                particles.unresolved_mesh_particles
            ));
        }
        // Constraint depth couldn't be resolved (no usable source refs) — count is still ranked.
        if constraints.count > 0 && constraints.depth.is_none() {
            not_evaluated.push("Constraint Depth (sources unresolved)".to_string());
        }

        PerfReport::new(source, "avatar", stats, not_evaluated)
    }
}

/// Length of a renderer's `m_Materials` list (its material slots), 0 if absent.
fn materials_len(body: &Yaml) -> u64 {
    body["m_Materials"].as_vec().map_or(0, |v| v.len() as u64)
}

fn is_scene_or_prefab(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("prefab" | "unity")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Platform, Rank};

    // A prefab-ish stream: a descriptor (so it counts as an avatar), two skinned meshes (5 material
    // slots total) sharing a 3-bone skeleton, a particle system, a light, a PhysBone, a PhysBone
    // collider, and one of each contact kind.
    const AVATAR: &str = "\
--- !u!114 &1
MonoBehaviour:
  m_Name: Avatar
  ViewPosition: {x: 0, y: 1.2, z: 0.1}
  baseAnimationLayers:
  - type: 4
    isDefault: 0
--- !u!137 &2
SkinnedMeshRenderer:
  m_Materials:
  - {fileID: 1}
  - {fileID: 2}
  - {fileID: 3}
  m_Bones:
  - {fileID: 100}
  - {fileID: 101}
--- !u!137 &3
SkinnedMeshRenderer:
  m_Materials:
  - {fileID: 4}
  - {fileID: 5}
  m_Bones:
  - {fileID: 101}
  - {fileID: 102}
--- !u!198 &4
ParticleSystem: {}
--- !u!108 &5
Light: {}
--- !u!114 &6
MonoBehaviour:
  rootTransform: {fileID: 0}
  endpointPosition: {x: 0, y: 0.1, z: 0}
  multiChildType: 0
--- !u!114 &7
MonoBehaviour:
  shapeType: 0
  radius: 0.1
  bonesAsSpheres: 0
  insideBounds: 0
--- !u!114 &8
MonoBehaviour:
  shapeType: 0
  radius: 0.1
  collisionTags: []
  receiverType: 1
  parameter: Touch
--- !u!114 &9
MonoBehaviour:
  shapeType: 0
  collisionTags: []
";

    fn report() -> PerfReport {
        let file = UnityFile::parse(AVATAR).unwrap();
        assert!(
            avatar_label(&file).is_some(),
            "should detect the descriptor"
        );
        // No mesh/material guids in this fixture, so geometry and textures resolve to zero.
        let geometry = Geometry {
            triangles: 0,
            unresolved_meshes: 0,
        };
        let texture_memory = TextureMemory {
            pc_bytes: 0,
            android_bytes: 0,
            unresolved: 0,
        };
        let bones = bone_count(&file);
        let dynamics = resolve_physbone_dynamics(&file);
        let particles = resolve_particles(&file, &GuidIndex::new(), &mut TriangleCache::new());
        let constraints = resolve_constraints(&file);
        Counts::of(&file).into_report(
            "Avatar.prefab (Avatar)".into(),
            &geometry,
            &texture_memory,
            bones,
            &dynamics,
            &particles,
            &constraints,
        )
    }

    fn value(r: &PerfReport, name: &str) -> u64 {
        r.stats.iter().find(|s| s.name == name).unwrap().value
    }

    #[test]
    fn counts_renderers_and_material_slots() {
        let r = report();
        assert_eq!(value(&r, "Skinned Meshes"), 2);
        assert_eq!(
            value(&r, "Material Slots"),
            5,
            "3 + 2 across the two renderers"
        );
        assert_eq!(value(&r, "Particle Systems"), 1);
        assert_eq!(value(&r, "Lights"), 1);
    }

    #[test]
    fn recognizes_vrc_dynamics_structurally() {
        let r = report();
        assert_eq!(value(&r, "PhysBone Components"), 1);
        assert_eq!(value(&r, "PhysBone Colliders"), 1);
        assert_eq!(value(&r, "Contacts"), 2, "sender + receiver both count");
    }

    #[test]
    fn counts_distinct_bones_across_skinned_renderers() {
        let r = report();
        // Bones {100, 101} ∪ {101, 102} = {100, 101, 102} -> 3 distinct.
        assert_eq!(value(&r, "Bones"), 3);
    }

    #[test]
    fn overall_rank_reflects_worst_metric() {
        let r = report();
        // One light is already Poor on PC (0/0/0/1), and that's the worst here.
        assert_eq!(r.overall(Platform::Pc), Rank::Poor);
        assert!(
            r.not_evaluated.iter().any(|m| m.contains("PhysBone")),
            "project view should still flag the component counts it can't measure: {:?}",
            r.not_evaluated
        );
    }

    // A 4-transform chain root(10) -> A(11) -> B(12) -> C(13), with a PhysBone whose root is the
    // chain root, an endpoint set, and two colliders.
    const CHAIN: &str = "\
--- !u!4 &10
Transform:
  m_GameObject: {fileID: 100}
  m_Children:
  - {fileID: 11}
--- !u!4 &11
Transform:
  m_GameObject: {fileID: 101}
  m_Children:
  - {fileID: 12}
--- !u!4 &12
Transform:
  m_GameObject: {fileID: 102}
  m_Children:
  - {fileID: 13}
--- !u!4 &13
Transform:
  m_GameObject: {fileID: 103}
  m_Children: []
--- !u!114 &20
MonoBehaviour:
  m_GameObject: {fileID: 100}
  rootTransform: {fileID: 10}
  endpointPosition: {x: 0, y: 0.05, z: 0}
  multiChildType: 0
  ignoreTransforms: []
  colliders:
  - {fileID: 200}
  - {fileID: 201}
";

    #[test]
    fn affected_transforms_excludes_root_and_adds_endpoint() {
        let file = UnityFile::parse(CHAIN).unwrap();
        let d = resolve_physbone_dynamics(&file);
        // Subtree {10,11,12,13}: 3 descendants of the root + 1 endpoint at the single tip = 4.
        assert_eq!(d.affected_transforms, 4);
        // 4 affected × 2 colliders = 8 collision checks.
        assert_eq!(d.collision_checks, 8);
        assert_eq!(d.unresolved, 0);
    }

    #[test]
    fn endpoint_off_drops_the_tip_bone() {
        // Same chain, endpointPosition zeroed.
        let file = UnityFile::parse(&CHAIN.replace("y: 0.05", "y: 0")).unwrap();
        let d = resolve_physbone_dynamics(&file);
        assert_eq!(d.affected_transforms, 3, "3 descendants, no endpoint");
        assert_eq!(d.collision_checks, 6);
    }

    #[test]
    fn ignore_transforms_prunes_the_subtree() {
        // Ignore B(12): B and its child C(13) drop out; A(11) becomes the tip.
        let file = UnityFile::parse(&CHAIN.replace(
            "ignoreTransforms: []",
            "ignoreTransforms:\n  - {fileID: 12}",
        ))
        .unwrap();
        let d = resolve_physbone_dynamics(&file);
        // Subtree {10,11}: 1 descendant + 1 endpoint at the new tip A = 2.
        assert_eq!(d.affected_transforms, 2);
        assert_eq!(d.collision_checks, 4);
    }

    #[test]
    fn rootless_physbone_resolves_via_its_gameobject() {
        // rootTransform unset (fileID 0): fall back to the transform on the PhysBone's GameObject.
        let file = UnityFile::parse(
            &CHAIN.replace("rootTransform: {fileID: 10}", "rootTransform: {fileID: 0}"),
        )
        .unwrap();
        let d = resolve_physbone_dynamics(&file);
        assert_eq!(
            d.affected_transforms, 4,
            "GO 100 -> transform 10, same chain"
        );
        assert_eq!(d.unresolved, 0);
    }

    #[test]
    fn unresolved_root_is_flagged_not_counted() {
        // rootTransform points at a transform absent from the file (a stripped nested prefab).
        let file = UnityFile::parse(&CHAIN.replace(
            "rootTransform: {fileID: 10}",
            "rootTransform: {fileID: 999}",
        ))
        .unwrap();
        let d = resolve_physbone_dynamics(&file);
        assert_eq!(d.affected_transforms, 0);
        assert_eq!(d.unresolved, 1);
    }

    #[test]
    fn file_without_descriptor_is_not_an_avatar() {
        let plain = "--- !u!137 &2\nSkinnedMeshRenderer:\n  m_Materials: []\n";
        let file = UnityFile::parse(plain).unwrap();
        assert!(avatar_label(&file).is_none());
    }

    // Two particle systems: one rate-limited (10/s × 3s = 30, under its 1000 cap → 30), one
    // cap-limited (50/s × 100s = 5000, capped at its 200 maxNumParticles → 200), plus a third
    // system missing InitialModule (unparseable → flagged, contributes 0). Total = 230.
    const PARTICLES: &str = "\
--- !u!198 &1
ParticleSystem:
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 3
  EmissionModule:
    rateOverTime:
      scalar: 10
    rateOverDistance:
      scalar: 0
--- !u!198 &2
ParticleSystem:
  InitialModule:
    maxNumParticles: 200
    startLifetime:
      scalar: 100
  EmissionModule:
    rateOverTime:
      scalar: 50
--- !u!198 &3
ParticleSystem:
  EmissionModule:
    rateOverTime:
      scalar: 5
";

    /// Resolve particles with no project context (empty guid index + fresh cache) — used by the
    /// tests that exercise only the live-particle estimate and the flag tallies.
    fn particles_of(file: &UnityFile) -> Particles {
        resolve_particles(file, &GuidIndex::new(), &mut TriangleCache::new())
    }

    #[test]
    fn total_particles_uses_min_of_cap_and_rate_times_lifetime() {
        let file = UnityFile::parse(PARTICLES).unwrap();
        let p = particles_of(&file);
        assert_eq!(p.total, 230, "30 (rate-limited) + 200 (cap-limited)");
        assert_eq!(p.unparsed, 1, "the system with no InitialModule is flagged");
    }

    #[test]
    fn particle_rate_sums_over_time_and_distance() {
        // 4/s over-time + 6/s over-distance = 10/s × 2s = 20, under the cap.
        let yaml = "\
--- !u!198 &1
ParticleSystem:
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 2
  EmissionModule:
    rateOverTime:
      scalar: 4
    rateOverDistance:
      scalar: 6
";
        let file = UnityFile::parse(yaml).unwrap();
        assert_eq!(particles_of(&file).total, 20);
    }

    #[test]
    fn mesh_particle_polygons_weight_triangles_by_particle_count() {
        // A system on GO 100 emitting 20 live particles, paired with a mesh-mode renderer whose
        // mesh resolves (via the guid index) to an FBX cached at 12 triangles. Cost = 12 × 20 = 240.
        let yaml = "\
--- !u!198 &1
ParticleSystem:
  m_GameObject: {fileID: 100}
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 2
  EmissionModule:
    rateOverTime:
      scalar: 10
--- !u!199 &2
ParticleSystemRenderer:
  m_GameObject: {fileID: 100}
  m_RenderMode: 4
  m_Mesh: {fileID: 4300000, guid: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa, type: 3}
";
        let file = UnityFile::parse(yaml).unwrap();
        // Seed the triangle cache + a one-entry guid index pointing the mesh guid at an FBX path.
        let fbx = PathBuf::from("/Assets/particle.fbx");
        let mut guids = GuidIndex::new();
        guids.insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(), fbx.clone());
        let mut cache = TriangleCache::new();
        cache.insert(fbx, Some(12));

        let p = resolve_particles(&file, &guids, &mut cache);
        assert_eq!(p.total, 20, "10/s × 2s, under the cap");
        assert_eq!(
            p.mesh_particle_triangles, 240,
            "12 triangles × 20 live particles"
        );
        assert_eq!(p.unresolved_mesh_particles, 0);
    }

    #[test]
    fn trail_and_collision_modules_increment_the_flag_counters() {
        let yaml = "\
--- !u!198 &1
ParticleSystem:
  m_GameObject: {fileID: 100}
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 1
  EmissionModule:
    rateOverTime:
      scalar: 5
  TrailModule:
    enabled: 1
  CollisionModule:
    enabled: 1
--- !u!198 &2
ParticleSystem:
  m_GameObject: {fileID: 101}
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 1
  EmissionModule:
    rateOverTime:
      scalar: 5
  TrailModule:
    enabled: 0
";
        let file = UnityFile::parse(yaml).unwrap();
        let p = particles_of(&file);
        assert_eq!(p.trail_systems, 1, "only the system with enabled: 1");
        assert_eq!(p.collision_systems, 1);
    }

    #[test]
    fn unresolved_mesh_particle_is_flagged_not_counted() {
        // A mesh-mode renderer whose mesh guid isn't in the (empty) index: flagged, adds nothing.
        let yaml = "\
--- !u!114 &1
MonoBehaviour:
  m_Name: Avatar
  ViewPosition: {x: 0, y: 1.2, z: 0.1}
  baseAnimationLayers:
  - type: 4
    isDefault: 0
--- !u!198 &2
ParticleSystem:
  m_GameObject: {fileID: 100}
  InitialModule:
    maxNumParticles: 1000
    startLifetime:
      scalar: 2
  EmissionModule:
    rateOverTime:
      scalar: 10
--- !u!199 &3
ParticleSystemRenderer:
  m_GameObject: {fileID: 100}
  m_RenderMode: 4
  m_Mesh: {fileID: 4300000, guid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb, type: 3}
";
        let file = UnityFile::parse(yaml).unwrap();
        let p = particles_of(&file);
        assert_eq!(p.mesh_particle_triangles, 0, "mesh didn't resolve");
        assert_eq!(p.unresolved_mesh_particles, 1);

        // It also surfaces a not_evaluated note via into_report.
        let geometry = Geometry {
            triangles: 0,
            unresolved_meshes: 0,
        };
        let texture_memory = TextureMemory {
            pc_bytes: 0,
            android_bytes: 0,
            unresolved: 0,
        };
        let dynamics = resolve_physbone_dynamics(&file);
        let constraints = resolve_constraints(&file);
        let r = Counts::of(&file).into_report(
            "A.prefab (Avatar)".into(),
            &geometry,
            &texture_memory,
            0,
            &dynamics,
            &p,
            &constraints,
        );
        assert!(
            r.not_evaluated
                .iter()
                .any(|m| m.contains("Mesh Particle Polygons")),
            "unresolved mesh particle flagged: {:?}",
            r.not_evaluated
        );
    }

    // A 3-deep constraint chain: constraint C1 (on GO 1000, transform 10) is driven by C2 (on GO
    // 1001, transform 11), which is driven by C3 (on GO 1002, transform 12). C1 reads its source
    // from transform 11 (GO 1001 → C2); C2 reads from transform 12 (GO 1002 → C3). Depth = 3.
    const CONSTRAINT_CHAIN: &str = "\
--- !u!4 &10
Transform:
  m_GameObject: {fileID: 1000}
--- !u!4 &11
Transform:
  m_GameObject: {fileID: 1001}
--- !u!4 &12
Transform:
  m_GameObject: {fileID: 1002}
--- !u!324 &20
ParentConstraint:
  m_GameObject: {fileID: 1000}
  m_Sources:
  - sourceTransform: {fileID: 11}
    weight: 1
--- !u!324 &21
ParentConstraint:
  m_GameObject: {fileID: 1001}
  m_Sources:
  - sourceTransform: {fileID: 12}
    weight: 1
--- !u!324 &22
ParentConstraint:
  m_GameObject: {fileID: 1002}
  m_Sources: []
";

    #[test]
    fn constraint_count_and_depth_walk_the_source_chain() {
        let file = UnityFile::parse(CONSTRAINT_CHAIN).unwrap();
        let c = resolve_constraints(&file);
        assert_eq!(c.count, 3, "three Unity ParentConstraints");
        assert_eq!(c.depth, Some(3), "C1 <- C2 <- C3");
    }

    #[test]
    fn vrc_constraint_recognized_structurally() {
        // A VRChat constraint MonoBehaviour (no class id 320-325) keyed off Sources + TargetTransform.
        let yaml = "\
--- !u!114 &30
MonoBehaviour:
  m_GameObject: {fileID: 1000}
  TargetTransform: {fileID: 0}
  Sources: []
";
        let file = UnityFile::parse(yaml).unwrap();
        let c = resolve_constraints(&file);
        assert_eq!(c.count, 1, "the VRC constraint is counted");
        // It has no resolvable source refs, so depth is the constraint itself (no chain).
        assert_eq!(c.depth, Some(1));
    }

    #[test]
    fn constraint_with_no_source_refs_reports_count_only() {
        // A standalone constraint whose source is the empty/zero ref: depth unresolved.
        let yaml = "\
--- !u!320 &40
PositionConstraint:
  m_GameObject: {fileID: 1000}
  m_Sources:
  - sourceTransform: {fileID: 0}
    weight: 1
";
        let file = UnityFile::parse(yaml).unwrap();
        let c = resolve_constraints(&file);
        assert_eq!(c.count, 1);
        assert_eq!(c.depth, None, "no usable source ref -> depth unknown");
    }

    #[test]
    fn constraint_count_ranks_and_depth_flagged_when_unresolved() {
        // An avatar with one constraint whose source is unresolvable: count ranked, depth flagged.
        let yaml = "\
--- !u!114 &1
MonoBehaviour:
  m_Name: Avatar
  ViewPosition: {x: 0, y: 1.2, z: 0.1}
  baseAnimationLayers:
  - type: 4
    isDefault: 0
--- !u!320 &40
PositionConstraint:
  m_GameObject: {fileID: 1000}
  m_Sources:
  - sourceTransform: {fileID: 0}
    weight: 1
";
        let file = UnityFile::parse(yaml).unwrap();
        let geometry = Geometry {
            triangles: 0,
            unresolved_meshes: 0,
        };
        let texture_memory = TextureMemory {
            pc_bytes: 0,
            android_bytes: 0,
            unresolved: 0,
        };
        let dynamics = resolve_physbone_dynamics(&file);
        let particles = resolve_particles(&file, &GuidIndex::new(), &mut TriangleCache::new());
        let constraints = resolve_constraints(&file);
        let r = Counts::of(&file).into_report(
            "A.prefab (Avatar)".into(),
            &geometry,
            &texture_memory,
            0,
            &dynamics,
            &particles,
            &constraints,
        );
        assert_eq!(value(&r, "Constraints"), 1);
        assert!(
            !r.stats.iter().any(|s| s.name == "Constraint Depth"),
            "depth not ranked when unresolved"
        );
        assert!(
            r.not_evaluated
                .iter()
                .any(|m| m.contains("Constraint Depth")),
            "depth flagged as unresolved: {:?}",
            r.not_evaluated
        );
    }
}
