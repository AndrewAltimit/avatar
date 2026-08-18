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
            fs::copy(&path, &dst)
                .with_context(|| format!("copying {} -> {}", path.display(), dst.display()))?;
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

/// `Packages/vpm-manifest.json` declaring the VRChat avatar SDK (VCC resolves it on open).
pub fn vpm_manifest(sdk_version: &str) -> String {
    format!(
        "{{\n  \"dependencies\": {{\n    \"com.vrchat.avatars\": {{\n      \"version\": \"{sdk_version}\"\n    }},\n    \"com.vrchat.base\": {{\n      \"version\": \"{sdk_version}\"\n    }}\n  }},\n  \"locked\": {{\n    \"com.vrchat.avatars\": {{\n      \"version\": \"{sdk_version}\",\n      \"dependencies\": {{\n        \"com.vrchat.base\": \"{sdk_version}\"\n      }}\n    }},\n    \"com.vrchat.base\": {{\n      \"version\": \"{sdk_version}\",\n      \"dependencies\": {{}}\n    }}\n  }}\n}}\n"
    )
}

/// `Packages/manifest.json` — Unity's own manifest; VCC adds the VRChat entries, but the file
/// must exist for Unity to treat the folder as a project.
pub fn unity_manifest() -> String {
    "{\n  \"dependencies\": {\n    \"com.unity.modules.animation\": \"1.0.0\",\n    \"com.unity.modules.imageconversion\": \"1.0.0\",\n    \"com.unity.modules.jsonserialize\": \"1.0.0\",\n    \"com.unity.modules.physics\": \"1.0.0\",\n    \"com.unity.modules.ui\": \"1.0.0\",\n    \"com.unity.modules.uielements\": \"1.0.0\",\n    \"com.unity.modules.unitywebrequest\": \"1.0.0\",\n    \"com.unity.modules.video\": \"1.0.0\",\n    \"com.unity.modules.audio\": \"1.0.0\",\n    \"com.unity.modules.cloth\": \"1.0.0\",\n    \"com.unity.modules.particlesystem\": \"1.0.0\",\n    \"com.unity.modules.physics2d\": \"1.0.0\",\n    \"com.unity.modules.terrain\": \"1.0.0\",\n    \"com.unity.modules.ai\": \"1.0.0\",\n    \"com.unity.modules.androidjni\": \"1.0.0\",\n    \"com.unity.modules.assetbundle\": \"1.0.0\",\n    \"com.unity.modules.director\": \"1.0.0\",\n    \"com.unity.modules.imgui\": \"1.0.0\",\n    \"com.unity.modules.screencapture\": \"1.0.0\",\n    \"com.unity.modules.unityanalytics\": \"1.0.0\",\n    \"com.unity.modules.unitywebrequestassetbundle\": \"1.0.0\",\n    \"com.unity.modules.unitywebrequestaudio\": \"1.0.0\",\n    \"com.unity.modules.unitywebrequesttexture\": \"1.0.0\",\n    \"com.unity.modules.unitywebrequestwww\": \"1.0.0\",\n    \"com.unity.modules.vehicles\": \"1.0.0\",\n    \"com.unity.modules.vr\": \"1.0.0\",\n    \"com.unity.modules.wind\": \"1.0.0\",\n    \"com.unity.modules.xr\": \"1.0.0\"\n  }\n}\n".to_string()
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
