//! Bundling VPM packages (VRChat's Creator-Companion package format) into the output project, and
//! relinking **locked** shader materials to their original shader.
//!
//! # VPM packages
//!
//! A VPM package is a plain directory (or `.zip` of one) with a `package.json` at its root — Unity
//! treats it as an *embedded* package when it sits under `Packages/<name>/`, and the Creator
//! Companion records it in `Packages/vpm-manifest.json`. Bundling one here (`--vpm-package`) means
//! the migrated project opens with, say, the shader package it needs already present, instead of a
//! wall of "Couldn't open include file" errors. The package's `legacyFolders` (what VCC deletes on
//! install, e.g. `Assets/_PoiyomiShaders`) are excluded from the asset copy, and any source asset
//! whose GUID the package already provides is skipped, so nothing collides.
//!
//! # Locked-shader relink
//!
//! Shader lockers (Poiyomi/Thry's optimizer, Kaj's) replace a material's shader with a generated,
//! per-material `Hidden/…` copy — and remember where it came from in the material's
//! `stringTagMap.OriginalShader`. Exports frequently carry the generated copies without their
//! `#include`s, which cannot compile anywhere else. Relinking finds a shader whose `Shader "<name>"`
//! matches `OriginalShader` (exactly, else ignoring decorative bullets/whitespace) among the source
//! assets and bundled packages, re-points the material at it, and turns the locker's
//! `_ShaderOptimizerEnabled` flag off; the generated `OptimizedShaders/<folder>` copy is then
//! excluded from the copy. Property values are kept — lockers preserve property names, and the
//! shader's own upgrade logic runs when the material is next inspected in Unity.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use avatar_unity_yaml::{
    EditableUnityFile, Scalar, UnityFile, build_guid_index, parse_path, walk_assets,
};
use serde::Serialize;

/// A VPM package staged for bundling.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct VpmPackage {
    pub name: String,
    pub version: String,
    /// Where the package's files are on disk (an extracted directory).
    #[serde(skip)]
    pub root: PathBuf,
    /// `legacyFolders` keys (Assets-relative, `Assets/` stripped) — folders VCC removes on install.
    pub legacy_folders: Vec<String>,
    /// GUIDs the package provides (from its `.meta` files).
    #[serde(skip)]
    pub guids: HashMap<String, PathBuf>,
    /// If the package was given as a `.zip`, where it was extracted (a temp dir the caller owns).
    #[serde(skip)]
    pub extracted_from_zip: Option<PathBuf>,
}

impl VpmPackage {
    /// Load a package from a directory containing `package.json`, or a `.zip` of one (extracted
    /// under `scratch`).
    pub fn load(path: &Path, scratch: &Path) -> Result<Self> {
        let (root, extracted) = if path.is_dir() {
            (path.to_path_buf(), None)
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            let dest = scratch.join(
                path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "vpm-package".into()),
            );
            let _ = fs::remove_dir_all(&dest);
            unzip(path, &dest)?;
            // Some zips wrap the package in one top-level directory.
            let root = if dest.join("package.json").exists() {
                dest.clone()
            } else {
                let mut inner = None;
                for e in fs::read_dir(&dest)? {
                    let e = e?;
                    if e.path().is_dir() && e.path().join("package.json").exists() {
                        inner = Some(e.path());
                        break;
                    }
                }
                inner.with_context(|| format!("{}: no package.json in the zip", path.display()))?
            };
            (root, Some(dest))
        } else {
            bail!(
                "{}: a VPM package is a directory with package.json or a .zip of one",
                path.display()
            );
        };
        let manifest = fs::read_to_string(root.join("package.json"))
            .with_context(|| format!("reading {}", root.join("package.json").display()))?;
        let json: serde_json::Value = serde_json::from_str(&manifest)
            .with_context(|| format!("parsing {}", root.join("package.json").display()))?;
        let name = json["name"]
            .as_str()
            .context("package.json has no name")?
            .to_string();
        let version = json["version"]
            .as_str()
            .context("package.json has no version")?
            .to_string();
        let legacy_folders: Vec<String> = json["legacyFolders"]
            .as_object()
            .map(|m| {
                m.keys()
                    .map(|k| {
                        k.replace('\\', "/")
                            .trim_start_matches("Assets/")
                            .trim_matches('/')
                            .to_string()
                    })
                    .filter(|k| !k.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let files = walk_assets(&root);
        let guids = build_guid_index(&files);
        Ok(VpmPackage {
            name,
            version,
            root,
            legacy_folders,
            guids,
            extracted_from_zip: extracted,
        })
    }

    /// Copy the package into `<out>/Packages/<name>/`. Returns files copied.
    pub fn install(&self, out: &Path) -> Result<usize> {
        let dest = out.join("Packages").join(&self.name);
        copy_dir(&self.root, &dest)
    }
}

/// `Shader "<name>"` of a `.shader` file, if it has one.
pub fn shader_name(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines().take(50) {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("Shader")
            && let Some(start) = rest.find('"')
            && let Some(end) = rest[start + 1..].find('"')
        {
            return Some(rest[start + 1..start + 1 + end].to_string());
        }
    }
    None
}

/// Normalise a shader name for fuzzy matching: drop decorative bullets/dots and *all* whitespace,
/// lower-case — so `.poiyomi/• Poiyomi Toon •` and `.poiyomi/Poiyomi Toon` compare equal.
pub fn normalize_shader_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '•' | '·' | '●' | '◆' | '★'))
        .collect::<String>()
        .to_lowercase()
}

/// An index of shader name → (guid, path) over a set of `.shader` files.
#[derive(Debug, Default)]
pub struct ShaderIndex {
    exact: HashMap<String, (String, PathBuf)>,
    normalized: HashMap<String, (String, PathBuf)>,
}

impl ShaderIndex {
    /// Index every `.shader` in `guid_index` (guid → path).
    pub fn add(&mut self, guid_index: &HashMap<String, PathBuf>) {
        for (guid, path) in guid_index {
            if path.extension().is_none_or(|e| e != "shader") {
                continue;
            }
            let Some(name) = shader_name(path) else {
                continue;
            };
            // Never relink onto a locked/hidden shader.
            if name.starts_with("Hidden/") {
                continue;
            }
            self.exact
                .entry(name.clone())
                .or_insert((guid.clone(), path.clone()));
            self.normalized
                .entry(normalize_shader_name(&name))
                .or_insert((guid.clone(), path.clone()));
        }
    }

    /// Find a shader by name — exact first, then normalised.
    pub fn find(&self, name: &str) -> Option<&(String, PathBuf)> {
        self.exact
            .get(name)
            .or_else(|| self.normalized.get(&normalize_shader_name(name)))
    }
}

/// One material relinked from a locked shader.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RelinkedMaterial {
    /// Assets-relative material path.
    pub material: String,
    pub original_shader: String,
    /// The shader it now points at (name) and where it lives.
    pub relinked_to: String,
    pub shader_path: String,
    /// The generated shader folder (Assets-relative) that is now excluded from the copy.
    pub dropped_folder: Option<String>,
}

/// The result of a relink pass: rewritten material texts + folders to exclude.
#[derive(Debug, Default)]
pub struct RelinkResult {
    /// Assets-relative material path → new file content.
    pub overrides: HashMap<String, String>,
    pub relinked: Vec<RelinkedMaterial>,
    /// Assets-relative directories (locked shader copies) to exclude from the copy.
    pub exclude_dirs: Vec<String>,
    /// Materials with an `OriginalShader` tag whose shader could not be found.
    pub unresolved: Vec<(String, String)>,
}

/// Relink every material under `assets_root` that carries an `OriginalShader` tag.
pub fn relink_locked_materials(
    assets_root: &Path,
    files: &[PathBuf],
    shaders: &ShaderIndex,
) -> RelinkResult {
    let mut out = RelinkResult::default();
    for mat in files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "mat"))
    {
        let Ok(text) = fs::read_to_string(mat) else {
            continue;
        };
        let rel = mat
            .strip_prefix(assets_root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let Ok(parsed) = UnityFile::parse(&text) else {
            continue;
        };
        let Some(doc) = parsed.documents.iter().find(|d| d.class_id == 21) else {
            continue;
        };
        let tags = &doc.body["stringTagMap"];
        let Some(original) = tags["OriginalShader"].as_str() else {
            continue;
        };
        let Some((guid, shader_path)) = shaders.find(original) else {
            out.unresolved.push((rel, original.to_string()));
            continue;
        };
        let Ok(mut edit) = EditableUnityFile::parse(&text) else {
            continue;
        };
        let Some(idx) = edit.doc_by_file_id(doc.file_id) else {
            continue;
        };
        if edit
            .set_reference(idx, &parse_path("m_Shader"), 4800000, Some(guid), 3)
            .is_err()
        {
            continue;
        }
        // Turn the locker's flag off so the shader's inspector treats the material as unlocked.
        if let Ok(items) = edit.sequence_items(idx, &parse_path("m_SavedProperties/m_Floats"))
            && let Some(pos) = items
                .iter()
                .position(|it| it.trim_start().starts_with("- _ShaderOptimizerEnabled:"))
        {
            let _ = edit.set_scalar(
                idx,
                &parse_path(&format!(
                    "m_SavedProperties/m_Floats/{pos}/_ShaderOptimizerEnabled"
                )),
                Scalar::Int(0),
            );
        }
        let dropped = tags["OptimizedShaderFolder"].as_str().map(|folder| {
            let dir = Path::new(&rel)
                .parent()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if dir.is_empty() {
                format!("OptimizedShaders/{folder}")
            } else {
                format!("{dir}/OptimizedShaders/{folder}")
            }
        });
        if let Some(d) = &dropped
            && !out.exclude_dirs.contains(d)
        {
            out.exclude_dirs.push(d.clone());
        }
        out.relinked.push(RelinkedMaterial {
            material: rel.clone(),
            original_shader: original.to_string(),
            relinked_to: shader_name(shader_path).unwrap_or_default(),
            shader_path: shader_path.to_string_lossy().replace('\\', "/"),
            dropped_folder: dropped,
        });
        out.overrides.insert(rel, edit.into_string());
    }
    out.relinked.sort_by(|a, b| a.material.cmp(&b.material));
    out.exclude_dirs.sort();
    out.unresolved.sort();
    out
}

fn unzip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file =
        fs::File::open(zip_path).with_context(|| format!("opening {}", zip_path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("reading {}", zip_path.display()))?;
    fs::create_dir_all(dest)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue; // path traversal guard
        };
        let target = dest.join(name);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        fs::write(&target, buf).with_context(|| format!("writing {}", target.display()))?;
    }
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<usize> {
    let mut n = 0;
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let e = e?;
            let p = e.path();
            let rel = p.strip_prefix(src).unwrap_or(&p);
            let target = dst.join(rel);
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&p, &target).with_context(|| format!("copying {}", p.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_name_normalisation_bridges_bullets() {
        assert_eq!(
            normalize_shader_name(".poiyomi/• Poiyomi Toon •"),
            normalize_shader_name(".poiyomi/Poiyomi Toon")
        );
        assert_ne!(
            normalize_shader_name(".poiyomi/Poiyomi Toon"),
            normalize_shader_name(".poiyomi/Poiyomi Toon Two Pass")
        );
    }

    #[test]
    fn relink_rewrites_shader_ref_and_optimizer_flag() {
        let dir = std::env::temp_dir().join(format!("avatar-relink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let assets = dir.join("Assets");
        fs::create_dir_all(assets.join("Mat/OptimizedShaders/Body-abc")).unwrap();
        fs::create_dir_all(assets.join("Shaders")).unwrap();
        fs::write(
            assets.join("Shaders/Toon.shader"),
            "Shader \".poiyomi/Poiyomi Toon\" { }\n",
        )
        .unwrap();
        fs::write(
            assets.join("Shaders/Toon.shader.meta"),
            "fileFormatVersion: 2\nguid: abcdefabcdefabcdefabcdefabcdefab\n",
        )
        .unwrap();
        let mat = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!21 &2100000\nMaterial:\n  m_Name: Body\n  m_Shader: {fileID: 4800000, guid: 11111111111111111111111111111aaa, type: 3}\n  stringTagMap:\n    OptimizedShaderFolder: Body-abc\n    OriginalShader: \".poiyomi/\\u2022 Poiyomi Toon \\u2022\"\n  m_SavedProperties:\n    m_Floats:\n    - _Cull: 2\n    - _ShaderOptimizerEnabled: 1\n";
        fs::write(assets.join("Mat/Body.mat"), mat).unwrap();
        fs::write(
            assets.join("Mat/Body.mat.meta"),
            "fileFormatVersion: 2\nguid: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n",
        )
        .unwrap();
        let files = walk_assets(&assets);
        let mut idx = ShaderIndex::default();
        idx.add(&build_guid_index(&files));
        let r = relink_locked_materials(&assets, &files, &idx);
        assert_eq!(r.relinked.len(), 1, "{:?}", r.unresolved);
        let new = &r.overrides["Mat/Body.mat"];
        assert!(new.contains(
            "m_Shader: {fileID: 4800000, guid: abcdefabcdefabcdefabcdefabcdefab, type: 3}"
        ));
        assert!(new.contains("- _ShaderOptimizerEnabled: 0"));
        assert!(new.contains("- _Cull: 2"));
        assert_eq!(
            r.exclude_dirs,
            vec!["Mat/OptimizedShaders/Body-abc".to_string()]
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
