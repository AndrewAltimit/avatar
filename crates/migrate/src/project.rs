//! Assembling the output Unity project: copy the source `Assets/` (minus exclusions), drop the
//! migrated prefab + generated assets in, and write the VPM manifest / Unity version stamps a
//! VRChat Creator Companion project needs so the user can "Add Existing Project" and open it.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Copy `src_assets/**` into `dst_assets/**`, skipping any path whose Assets-relative form starts
/// with one of `exclude` (directory prefixes such as `VRCSDK` or `Avatar/DynamicBone`) and any
/// exact file in `skip_files` (Assets-relative). Returns `(files copied, files skipped)`.
pub fn copy_assets(
    src_assets: &Path,
    dst_assets: &Path,
    exclude: &[String],
    skip_files: &[String],
    overrides: &std::collections::HashMap<String, String>,
) -> Result<(usize, usize)> {
    let mut copied = 0;
    let mut skipped = 0;
    let mut stack = vec![src_assets.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let rel = path
                .strip_prefix(src_assets)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let excluded = exclude.iter().any(|e| {
                let e = e.trim_matches('/');
                rel == e || rel.starts_with(&format!("{e}/"))
            });
            if excluded {
                skipped += 1;
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if skip_files.iter().any(|f| f.trim_matches('/') == rel) {
                skipped += 1;
                continue;
            }
            let dst = dst_assets.join(&rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Some(content) = overrides.get(&rel) {
                fs::write(&dst, content).with_context(|| format!("writing {}", dst.display()))?;
            } else {
                fs::copy(&path, &dst)
                    .with_context(|| format!("copying {} -> {}", path.display(), dst.display()))?;
            }
            copied += 1;
        }
    }
    Ok((copied, skipped))
}

/// Write a text file, creating parent directories.
pub fn write_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// `Packages/vpm-manifest.json` declaring the VRChat avatar SDK plus any bundled packages (VCC
/// resolves the SDK on open; bundled packages are already present as embedded packages).
pub fn vpm_manifest(sdk_version: &str, bundled: &[(String, String)]) -> String {
    let mut deps = serde_json::Map::new();
    let mut locked = serde_json::Map::new();
    deps.insert(
        "com.vrchat.avatars".into(),
        serde_json::json!({ "version": sdk_version }),
    );
    deps.insert(
        "com.vrchat.base".into(),
        serde_json::json!({ "version": sdk_version }),
    );
    locked.insert(
        "com.vrchat.avatars".into(),
        serde_json::json!({ "version": sdk_version, "dependencies": { "com.vrchat.base": sdk_version } }),
    );
    locked.insert(
        "com.vrchat.base".into(),
        serde_json::json!({ "version": sdk_version, "dependencies": {} }),
    );
    for (name, version) in bundled {
        deps.insert(name.clone(), serde_json::json!({ "version": version }));
        locked.insert(
            name.clone(),
            serde_json::json!({ "version": version, "dependencies": {} }),
        );
    }
    let root = serde_json::json!({ "dependencies": deps, "locked": locked });
    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    text
}

/// `Packages/manifest.json` — Unity's own manifest, matching what a Creator-Companion avatar
/// template project (Unity 2022.3) declares. `com.unity.test-framework` is load-bearing: the
/// VRChat SDK's editor assembly ships NUnit tests and fails to compile without it, which puts the
/// whole project into a broken state; TextMeshPro/uGUI back the SDK's control panel UI.
pub fn unity_manifest() -> String {
    let registry: &[(&str, &str)] = &[
        ("com.unity.ide.visualstudio", "2.0.22"),
        ("com.unity.test-framework", "1.1.33"),
        ("com.unity.textmeshpro", "3.0.6"),
        ("com.unity.timeline", "1.7.6"),
        ("com.unity.ugui", "1.0.0"),
        ("com.unity.visualscripting", "1.9.1"),
    ];
    let modules: &[&str] = &[
        "ai",
        "androidjni",
        "animation",
        "assetbundle",
        "audio",
        "cloth",
        "director",
        "imageconversion",
        "imgui",
        "jsonserialize",
        "particlesystem",
        "physics",
        "physics2d",
        "screencapture",
        "terrain",
        "terrainphysics",
        "tilemap",
        "ui",
        "uielements",
        "umbra",
        "unityanalytics",
        "unitywebrequest",
        "unitywebrequestassetbundle",
        "unitywebrequestaudio",
        "unitywebrequesttexture",
        "unitywebrequestwww",
        "vehicles",
        "video",
        "vr",
        "wind",
        "xr",
    ];
    let mut deps = serde_json::Map::new();
    for (k, v) in registry {
        deps.insert((*k).into(), serde_json::Value::String((*v).into()));
    }
    for m in modules {
        deps.insert(
            format!("com.unity.modules.{m}"),
            serde_json::Value::String("1.0.0".into()),
        );
    }
    let root = serde_json::json!({ "dependencies": deps });
    let mut text = serde_json::to_string_pretty(&root).unwrap_or_default();
    text.push('\n');
    text
}

/// `ProjectSettings/ProjectVersion.txt`.
pub fn project_version(unity_version: &str) -> String {
    format!("m_EditorVersion: {unity_version}\n")
}

/// A `.meta` for a `.prefab` (PrefabImporter).
pub fn prefab_meta(guid: &str) -> String {
    format!(
        "fileFormatVersion: 2\nguid: {guid}\nPrefabImporter:\n  externalObjects: {{}}\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n"
    )
}

/// A `.meta` for a folder.
pub fn folder_meta(guid: &str) -> String {
    format!(
        "fileFormatVersion: 2\nguid: {guid}\nfolderAsset: yes\nDefaultImporter:\n  externalObjects: {{}}\n  userData: \n  assetBundleName: \n  assetBundleVariant: \n"
    )
}

/// The output project's `Assets/` directory.
pub fn assets_dir(out: &Path) -> PathBuf {
    out.join("Assets")
}
