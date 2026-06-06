//! Offline computation of VRChat's **avatar performance ranking** — the Excellent → Very Poor
//! grade VRChat assigns each avatar — from the files an avatar is made of, with no Unity.
//!
//! VRChat ranks an avatar per *metric* (triangles, material slots, PhysBone components, …) against
//! a fixed table of per-tier limits, and the avatar's overall rank is the **worst** single metric.
//! This crate reproduces that math:
//!
//! - [`analyze_fbx`] reads the geometry side from an FBX (triangles, skinned/basic meshes, material
//!   slots, bones) — the "is my mesh too heavy before I even open Unity?" check.
//! - [`analyze_project`] reads the component side from a Unity project's prefabs/scenes (PhysBones,
//!   colliders, contacts, particle systems, lights, …).
//!
//! Each source measures only part of the full rank, so a [`PerfReport`] also lists the rank-
//! affecting metrics it could **not** evaluate ([`PerfReport::not_evaluated`]) — a clean rank from a
//! bare FBX does not account for the component side, and vice versa.
//!
//! Thresholds are encoded as data ([`Metric::limits`]) rather than scattered constants, so a VRChat
//! limit change is a one-line edit. Source of the numbers:
//! <https://creators.vrchat.com/avatars/avatar-performance-ranking-system/>.

use std::collections::HashSet;
use std::path::Path;

use anyhow::Result;
use avatar_fbx::{FbxDocument, FbxScene};
use avatar_mesh::RawMesh;
use serde::Serialize;

mod project;
mod texture;
pub use project::analyze_project;

/// One mebibyte, the unit VRChat's Texture Memory limits are quoted in.
const MIB: u64 = 1024 * 1024;

/// The platform an avatar is ranked against. VRChat publishes separate (stricter) Android limits,
/// and some metrics (Lights, Audio Sources, Cloth, physics) are simply not ranked on Android.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Platform {
    Pc,
    Android,
}

impl Platform {
    pub fn label(self) -> &'static str {
        match self {
            Platform::Pc => "PC",
            Platform::Android => "Android",
        }
    }

    /// Both platforms, in display order.
    pub const ALL: [Platform; 2] = [Platform::Pc, Platform::Android];
}

/// One of VRChat's five performance tiers. Declared in **ascending severity** so the worst of a set
/// is `max` ([`Rank::worst`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Rank {
    Excellent,
    Good,
    Medium,
    Poor,
    VeryPoor,
}

impl Rank {
    /// Human-readable label (note the space in "Very Poor").
    pub fn label(self) -> &'static str {
        match self {
            Rank::Excellent => "Excellent",
            Rank::Good => "Good",
            Rank::Medium => "Medium",
            Rank::Poor => "Poor",
            Rank::VeryPoor => "Very Poor",
        }
    }

    /// The worse (higher-severity) of two ranks.
    pub fn worst(self, other: Rank) -> Rank {
        self.max(other)
    }
}

/// Per-tier upper bounds for a metric. A measured value earns the first tier whose bound it does
/// not exceed (checked Excellent → Poor); anything above the Poor bound is Very Poor. The bounds are
/// inclusive, which is how VRChat's `0/0/0/1` rows (e.g. Lights) work: `0` is Excellent, `1` is Poor.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub excellent: u64,
    pub good: u64,
    pub medium: u64,
    pub poor: u64,
}

const fn lim(excellent: u64, good: u64, medium: u64, poor: u64) -> Limits {
    Limits {
        excellent,
        good,
        medium,
        poor,
    }
}

impl Limits {
    /// The rank a measured `value` earns under these limits.
    pub fn rank(&self, value: u64) -> Rank {
        if value <= self.excellent {
            Rank::Excellent
        } else if value <= self.good {
            Rank::Good
        } else if value <= self.medium {
            Rank::Medium
        } else if value <= self.poor {
            Rank::Poor
        } else {
            Rank::VeryPoor
        }
    }
}

/// A performance metric this crate can measure and rank. (VRChat tracks a few more — mesh-particle
/// active polygons, particle trail/collision flags — that we still surface via
/// [`PerfReport::not_evaluated`] rather than rank.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Metric {
    Triangles,
    TextureMemory,
    SkinnedMeshes,
    BasicMeshes,
    MaterialSlots,
    Bones,
    PhysBoneComponents,
    PhysBoneColliders,
    PhysBoneTransforms,
    PhysBoneCollisionChecks,
    Contacts,
    ParticleSystems,
    TotalParticles,
    Constraints,
    ConstraintDepth,
    Lights,
    AudioSources,
    TrailRenderers,
    LineRenderers,
    Cloths,
    PhysicsColliders,
    PhysicsRigidbodies,
    Animators,
}

impl Metric {
    /// Display name, matching VRChat's performance panel wording.
    pub fn name(self) -> &'static str {
        use Metric::*;
        match self {
            Triangles => "Triangles",
            TextureMemory => "Texture Memory",
            SkinnedMeshes => "Skinned Meshes",
            BasicMeshes => "Basic Meshes",
            MaterialSlots => "Material Slots",
            Bones => "Bones",
            PhysBoneComponents => "PhysBone Components",
            PhysBoneColliders => "PhysBone Colliders",
            PhysBoneTransforms => "PhysBone Affected Transforms",
            PhysBoneCollisionChecks => "PhysBone Collision Check Count",
            Contacts => "Contacts",
            ParticleSystems => "Particle Systems",
            TotalParticles => "Total Particles",
            Constraints => "Constraints",
            ConstraintDepth => "Constraint Depth",
            Lights => "Lights",
            AudioSources => "Audio Sources",
            TrailRenderers => "Trail Renderers",
            LineRenderers => "Line Renderers",
            Cloths => "Cloths",
            PhysicsColliders => "Physics Colliders",
            PhysicsRigidbodies => "Physics Rigidbodies",
            Animators => "Animators",
        }
    }

    /// The VRChat limits table, as `(PC, Android)`. `Android` is `None` for metrics VRChat does not
    /// rank on Android (its mobile table omits Lights / Audio Sources / Cloth / physics colliders &
    /// rigidbodies — those component types are stripped on mobile).
    fn defs(self) -> (Limits, Option<Limits>) {
        use Metric::*;
        match self {
            Triangles => (
                lim(32_000, 70_000, 70_000, 70_000),
                Some(lim(7_500, 10_000, 15_000, 20_000)),
            ),
            // Limits are in bytes (VRChat quotes them in MB).
            TextureMemory => (
                lim(40 * MIB, 75 * MIB, 110 * MIB, 150 * MIB),
                Some(lim(10 * MIB, 18 * MIB, 25 * MIB, 40 * MIB)),
            ),
            SkinnedMeshes => (lim(1, 2, 8, 16), Some(lim(1, 1, 2, 2))),
            BasicMeshes => (lim(4, 8, 16, 24), Some(lim(1, 1, 2, 2))),
            MaterialSlots => (lim(4, 8, 16, 32), Some(lim(1, 1, 2, 4))),
            Bones => (lim(75, 150, 256, 400), Some(lim(75, 90, 150, 150))),
            PhysBoneComponents => (lim(4, 8, 16, 32), Some(lim(0, 4, 6, 8))),
            PhysBoneColliders => (lim(4, 8, 16, 32), Some(lim(0, 4, 8, 16))),
            PhysBoneTransforms => (lim(16, 32, 64, 128), Some(lim(0, 16, 32, 64))),
            PhysBoneCollisionChecks => (lim(8, 16, 32, 64), Some(lim(0, 8, 16, 32))),
            Contacts => (lim(8, 16, 24, 32), Some(lim(2, 4, 8, 16))),
            ParticleSystems => (lim(0, 4, 8, 16), Some(lim(0, 0, 0, 2))),
            TotalParticles => (lim(0, 300, 1_000, 2_500), Some(lim(0, 0, 0, 200))),
            Constraints => (lim(100, 250, 300, 350), Some(lim(30, 60, 120, 150))),
            ConstraintDepth => (lim(20, 50, 80, 100), Some(lim(5, 15, 35, 50))),
            Lights => (lim(0, 0, 0, 1), None),
            AudioSources => (lim(1, 4, 8, 8), None),
            TrailRenderers => (lim(1, 2, 4, 8), Some(lim(0, 0, 0, 1))),
            LineRenderers => (lim(1, 2, 4, 8), Some(lim(0, 0, 0, 1))),
            Cloths => (lim(0, 1, 1, 1), None),
            PhysicsColliders => (lim(0, 1, 8, 8), None),
            PhysicsRigidbodies => (lim(0, 1, 8, 8), None),
            Animators => (lim(1, 4, 16, 32), Some(lim(1, 1, 1, 2))),
        }
    }

    /// The limits for `platform`, or `None` if the metric is not ranked there.
    pub fn limits(self, platform: Platform) -> Option<Limits> {
        let (pc, android) = self.defs();
        match platform {
            Platform::Pc => Some(pc),
            Platform::Android => android,
        }
    }
}

/// One measured metric and the rank it earns on each platform.
///
/// `value` is the PC measurement; `android_value` is the Android one. They're equal for almost every
/// metric (a triangle count is the same on both — only the *limits* differ), and differ only when
/// the measured quantity itself is platform-dependent: **texture memory**, where the textures are
/// recompressed to a different format (ASTC/ETC2 vs DXT/BC) for each platform.
#[derive(Debug, Clone, Serialize)]
pub struct MetricStat {
    #[serde(skip)]
    pub metric: Metric,
    pub name: &'static str,
    /// The PC measurement.
    pub value: u64,
    /// The Android measurement (equal to `value` unless the quantity is platform-dependent).
    pub android_value: u64,
    /// The PC rank (always present — every metric we measure has PC limits).
    pub pc: Option<Rank>,
    /// The Android rank, or `None` if the metric is not ranked on Android.
    pub android: Option<Rank>,
}

impl MetricStat {
    /// A metric whose measured value is the same on both platforms (the common case).
    pub(crate) fn new(metric: Metric, value: u64) -> Self {
        Self::with_values(metric, value, value)
    }

    /// A metric whose measured value differs per platform (e.g. texture memory).
    pub(crate) fn with_values(metric: Metric, pc_value: u64, android_value: u64) -> Self {
        MetricStat {
            metric,
            name: metric.name(),
            value: pc_value,
            android_value,
            pc: metric.limits(Platform::Pc).map(|l| l.rank(pc_value)),
            android: metric
                .limits(Platform::Android)
                .map(|l| l.rank(android_value)),
        }
    }

    /// The rank this metric earns on `platform`, if it is ranked there.
    pub fn rank(&self, platform: Platform) -> Option<Rank> {
        match platform {
            Platform::Pc => self.pc,
            Platform::Android => self.android,
        }
    }

    /// The measured value on `platform`.
    pub fn value(&self, platform: Platform) -> u64 {
        match platform {
            Platform::Pc => self.value,
            Platform::Android => self.android_value,
        }
    }
}

/// A performance report for one source (an FBX file, or one avatar in a project).
#[derive(Debug, Clone, Serialize)]
pub struct PerfReport {
    /// What was measured (a file path, or `path (AvatarName)` for a project avatar).
    pub source: String,
    /// `"fbx"` (geometry only) or `"avatar"` (project components only).
    pub kind: &'static str,
    pub stats: Vec<MetricStat>,
    /// Rank-affecting metrics this source could **not** measure. The overall rank is the worst of
    /// the *measured* metrics only, so these are the categories that could still drag it down.
    pub not_evaluated: Vec<String>,
    /// Worst measured PC rank.
    pub pc_overall: Rank,
    /// Worst measured Android rank.
    pub android_overall: Rank,
}

impl PerfReport {
    pub(crate) fn new(
        source: String,
        kind: &'static str,
        stats: Vec<MetricStat>,
        not_evaluated: Vec<String>,
    ) -> Self {
        PerfReport {
            pc_overall: worst_measured(&stats, Platform::Pc),
            android_overall: worst_measured(&stats, Platform::Android),
            source,
            kind,
            stats,
            not_evaluated,
        }
    }

    /// The worst measured rank on `platform` (the avatar's overall rank, modulo unmeasured metrics).
    pub fn overall(&self, platform: Platform) -> Rank {
        match platform {
            Platform::Pc => self.pc_overall,
            Platform::Android => self.android_overall,
        }
    }
}

/// The worst rank across the measured metrics on `platform` (Excellent if there are none).
fn worst_measured(stats: &[MetricStat], platform: Platform) -> Rank {
    stats
        .iter()
        .filter_map(|s| s.rank(platform))
        .fold(Rank::Excellent, Rank::worst)
}

/// Rank-affecting metrics an FBX file cannot reveal (they live in the Unity project / components).
const FBX_NOT_EVALUATED: &[&str] = &[
    "Texture Memory",
    "PhysBone Components",
    "PhysBone Colliders",
    "Contacts",
    "Particle Systems",
    "Lights",
    "Audio Sources",
    "Constraints",
    "Animators",
];

/// Compute the geometry-side performance stats for an FBX file.
///
/// Covers Triangles, Skinned/Basic Meshes, Material Slots, and Bones — everything a rank depends on
/// that lives in the model itself. The component side (PhysBones, particles, …) is a Unity-project
/// concern; see [`analyze_project`] and [`PerfReport::not_evaluated`].
pub fn analyze_fbx(path: &Path) -> Result<PerfReport> {
    let doc = FbxDocument::load(path)?;
    analyze_fbx_doc(&doc, &source_label(path))
}

/// Like [`analyze_fbx`] but from an in-memory FBX byte buffer (used by tests).
pub fn analyze_fbx_bytes(bytes: &[u8], source: &str) -> Result<PerfReport> {
    let doc = FbxDocument::from_bytes(bytes)?;
    analyze_fbx_doc(&doc, source)
}

fn analyze_fbx_doc(doc: &FbxDocument, source: &str) -> Result<PerfReport> {
    let scene = doc.scene();
    let meshes = doc.meshes()?;

    let triangles: u64 = meshes.iter().map(|m| (m.indices.len() / 3) as u64).sum();
    let skinned = meshes.iter().filter(|m| m.is_skinned()).count() as u64;
    let basic = (meshes.len() as u64).saturating_sub(skinned);
    let material_slots = material_slot_count(&scene);
    let bones = skinned_bone_count(&meshes, &scene);

    let stats = vec![
        MetricStat::new(Metric::Triangles, triangles),
        MetricStat::new(Metric::SkinnedMeshes, skinned),
        MetricStat::new(Metric::BasicMeshes, basic),
        MetricStat::new(Metric::MaterialSlots, material_slots),
        MetricStat::new(Metric::Bones, bones),
    ];
    Ok(PerfReport::new(
        source.to_string(),
        "fbx",
        stats,
        FBX_NOT_EVALUATED.iter().map(|s| s.to_string()).collect(),
    ))
}

/// Material slots ≈ the number of `Material → Model` connections (one slot per attachment, so a
/// material shared across two meshes counts twice, as Unity counts slots). Falls back to the
/// material object count if no connection wiring is present.
fn material_slot_count(scene: &FbxScene) -> u64 {
    let materials: HashSet<i64> = scene
        .objects
        .iter()
        .filter(|o| o.node_name == "Material")
        .map(|o| o.id)
        .collect();
    if materials.is_empty() {
        return 0;
    }
    let slots = scene
        .connections
        .iter()
        .filter(|c| c.kind == "OO" && materials.contains(&c.child))
        .count() as u64;
    if slots > 0 {
        slots
    } else {
        materials.len() as u64
    }
}

/// Bone count = distinct bones actually driving skin clusters (what VRChat counts for skinned mesh
/// renderers). Falls back to the bone-like model count for a rig with no skin extracted.
fn skinned_bone_count(meshes: &[RawMesh], scene: &FbxScene) -> u64 {
    let mut bones: HashSet<i64> = HashSet::new();
    for mesh in meshes {
        if let Some(skin) = &mesh.skin {
            for cluster in &skin.clusters {
                bones.insert(cluster.bone_id);
            }
        }
    }
    if !bones.is_empty() {
        bones.len() as u64
    } else {
        scene.objects.iter().filter(|o| o.is_bone_like()).count() as u64
    }
}

/// The display label for an FBX source: its file name, or the full path if that fails.
fn source_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_boundaries_are_inclusive_upper_bounds() {
        let l = lim(4, 8, 16, 32); // PhysBone components, PC.
        assert_eq!(l.rank(0), Rank::Excellent);
        assert_eq!(l.rank(4), Rank::Excellent);
        assert_eq!(l.rank(5), Rank::Good);
        assert_eq!(l.rank(8), Rank::Good);
        assert_eq!(l.rank(16), Rank::Medium);
        assert_eq!(l.rank(32), Rank::Poor);
        assert_eq!(l.rank(33), Rank::VeryPoor);
    }

    #[test]
    fn lights_zero_is_excellent_one_is_poor() {
        // VRChat's 0/0/0/1 row: anything but zero is already Poor, two is Very Poor.
        let l = Metric::Lights.limits(Platform::Pc).unwrap();
        assert_eq!(l.rank(0), Rank::Excellent);
        assert_eq!(l.rank(1), Rank::Poor);
        assert_eq!(l.rank(2), Rank::VeryPoor);
        assert!(
            Metric::Lights.limits(Platform::Android).is_none(),
            "Lights are not ranked on Android"
        );
    }

    #[test]
    fn overall_is_worst_measured_metric() {
        let stats = vec![
            MetricStat::new(Metric::Triangles, 1_000), // PC Excellent
            MetricStat::new(Metric::Bones, 300),       // PC Poor (256 < 300 <= 400)
        ];
        let report = PerfReport::new("x".into(), "fbx", stats, vec![]);
        assert_eq!(report.overall(Platform::Pc), Rank::Poor);
        // 300 bones on Android exceeds the Poor bound (150) -> Very Poor.
        assert_eq!(report.overall(Platform::Android), Rank::VeryPoor);
    }

    #[test]
    fn worst_helper_picks_higher_severity() {
        assert_eq!(Rank::Good.worst(Rank::Poor), Rank::Poor);
        assert_eq!(Rank::VeryPoor.worst(Rank::Excellent), Rank::VeryPoor);
    }
}
