//! In-browser FBX analysis for the docs site's Analyzer page.
//!
//! A thin wasm-bindgen shim over the same diagnose graph the CLI uses:
//! [`avatar_fbx`] (parse), [`avatar_armature`] (humanoid rig check), and
//! [`avatar_stats`] (performance rank). Everything runs client-side on the
//! bytes of a dropped file — nothing is uploaded anywhere.
//!
//! The exported surface is one function, `analyze_fbx(bytes, name)`, returning
//! a JSON string; [`analyze`] is the pure core so the report shape is testable
//! off-wasm. Built for the site with
//! `wasm-pack build crates/web-analyzer --target web`.

use anyhow::Result;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// The full report the Analyzer page renders. `armature` and `stats` are the
/// same serde shapes the CLI's `--json` output uses.
#[derive(Serialize)]
pub struct Report {
    pub fbx: FbxSummary,
    pub armature: avatar_armature::ArmatureReport,
    pub stats: avatar_stats::PerfReport,
    pub blendshapes: Vec<Blendshape>,
}

/// Coarse object counts so the page can show what the file contains at a glance.
#[derive(Serialize)]
pub struct FbxSummary {
    /// FBX format version as reported by the parser, e.g. `7400`.
    pub version: u32,
    pub models: usize,
    pub geometries: usize,
    pub materials: usize,
    pub deformers: usize,
    pub bone_like: usize,
}

/// One blendshape channel (`avatar_fbx::BlendshapeChannel` isn't serde-derived).
#[derive(Serialize)]
pub struct Blendshape {
    pub name: String,
    pub mesh: Option<String>,
}

/// Parse + analyze one binary FBX. Pure (no fs, no JS types), so unit tests can
/// pin the report against the synthetic testkit corpus.
pub fn analyze(bytes: &[u8], name: &str) -> Result<Report> {
    let doc = avatar_fbx::FbxDocument::from_bytes(bytes)?;
    let scene = doc.scene();

    let count = |node: &str| scene.objects.iter().filter(|o| o.node_name == node).count();
    let fbx = FbxSummary {
        version: scene.version,
        models: count("Model"),
        geometries: count("Geometry"),
        materials: count("Material"),
        deformers: count("Deformer"),
        bone_like: scene.objects.iter().filter(|o| o.is_bone_like()).count(),
    };

    let armature = avatar_armature::analyze(&scene);
    let stats = avatar_stats::analyze_fbx_bytes(bytes, name)?;
    let blendshapes = scene
        .blendshape_channels()
        .into_iter()
        .map(|c| Blendshape {
            name: c.name,
            mesh: c.mesh_model_name,
        })
        .collect();

    Ok(Report {
        fbx,
        armature,
        stats,
        blendshapes,
    })
}

/// The wasm entry point: bytes + display name in, JSON out. Errors surface to
/// JS as a thrown exception carrying the anyhow message.
#[wasm_bindgen]
pub fn analyze_fbx(bytes: &[u8], name: &str) -> Result<String, JsError> {
    let report = analyze(bytes, name).map_err(|e| JsError::new(&format!("{e:#}")))?;
    serde_json::to_string(&report).map_err(|e| JsError::new(&e.to_string()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn analyzes_synthetic_humanoid() {
        let bytes = avatar_testkit::fbx::humanoid_skeleton();
        let report = super::analyze(&bytes, "humanoid.fbx").expect("analyze");
        assert!(report.fbx.models > 0);
        assert!(
            report.armature.is_humanoid_ready(),
            "synthetic humanoid must map clean"
        );
        // The report must round-trip to JSON with the keys the Analyzer page reads.
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        for key in ["fbx", "armature", "stats", "blendshapes"] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        assert!(json["stats"]["pc_overall"].is_string());
    }
}
