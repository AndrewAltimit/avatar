//! Discovery and parsing of a VRChat/Unity project: the VPM manifest
//! (`Packages/vpm-manifest.json`), the editor version (`ProjectSettings/ProjectVersion.txt`),
//! and the locations that matter for linting.
//!
//! References: <https://vcc.docs.vrchat.com/vpm/>.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The VRChat avatar SDK package id.
pub const AVATAR_SDK: &str = "com.vrchat.avatars";
/// The VRChat base SDK package id.
pub const BASE_SDK: &str = "com.vrchat.base";

/// A package recorded in the VPM manifest.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    /// `true` if it appears in `locked` (a resolved version), vs only in `dependencies`.
    pub locked: bool,
}

/// A discovered Unity project.
#[derive(Debug, Clone)]
pub struct UnityProject {
    pub root: PathBuf,
    /// Editor version from `ProjectSettings/ProjectVersion.txt`, e.g. `"2022.3.22f1"`.
    pub unity_version: Option<String>,
    pub packages: Vec<Package>,
}

impl UnityProject {
    /// Discover a project by walking up from `start` looking for `Packages/vpm-manifest.json`.
    /// Falls back to treating `start` as the root if it directly contains an `Assets` directory.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use avatar_vpm::UnityProject;
    /// use std::path::Path;
    ///
    /// let project = UnityProject::discover(Path::new("./MyAvatarProject"))?;
    /// if project.has_avatar_sdk() {
    ///     println!("avatars SDK {:?}", project.package_version(avatar_vpm::AVATAR_SDK));
    /// }
    /// # anyhow::Ok(())
    /// ```
    pub fn discover(start: &Path) -> Result<Self> {
        let start = start
            .canonicalize()
            .with_context(|| format!("resolving path {}", start.display()))?;

        let mut dir: Option<&Path> = Some(&start);
        while let Some(d) = dir {
            if d.join("Packages/vpm-manifest.json").is_file() {
                return Self::load(d);
            }
            dir = d.parent();
        }

        // No VPM manifest found; accept a bare Unity project (has Assets/).
        if start.join("Assets").is_dir() {
            return Self::load(&start);
        }

        bail!(
            "no Unity/VPM project found at or above {} (looked for Packages/vpm-manifest.json or an Assets/ directory)",
            start.display()
        )
    }

    fn load(root: &Path) -> Result<Self> {
        let unity_version = read_unity_version(root);
        let packages = read_manifest(root)?;
        Ok(UnityProject {
            root: root.to_path_buf(),
            unity_version,
            packages,
        })
    }

    /// The project's `Assets/` directory.
    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("Assets")
    }

    /// The resolved version of a package, if present.
    pub fn package_version(&self, name: &str) -> Option<&str> {
        self.packages
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.version.as_str())
    }

    /// `true` if the VRChat avatar SDK is present.
    pub fn has_avatar_sdk(&self) -> bool {
        self.package_version(AVATAR_SDK).is_some()
    }
}

fn read_unity_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("ProjectSettings/ProjectVersion.txt")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("m_EditorVersion:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    dependencies: BTreeMap<String, RawVersion>,
    #[serde(default)]
    locked: BTreeMap<String, RawVersion>,
}

#[derive(Deserialize)]
struct RawVersion {
    version: Option<String>,
}

fn read_manifest(root: &Path) -> Result<Vec<Package>> {
    let path = root.join("Packages/vpm-manifest.json");
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: RawManifest =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;

    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    for (name, v) in manifest.dependencies {
        packages.insert(
            name.clone(),
            Package {
                name,
                version: v.version.unwrap_or_default(),
                locked: false,
            },
        );
    }
    // `locked` versions are authoritative; overwrite the dependency entry.
    for (name, v) in manifest.locked {
        if let Some(version) = v.version {
            packages.insert(
                name.clone(),
                Package {
                    name,
                    version,
                    locked: true,
                },
            );
        }
    }

    Ok(packages.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_with_locked_versions() {
        let json = r#"{
            "dependencies": { "com.vrchat.avatars": { "version": "3.7.0" } },
            "locked": {
                "com.vrchat.avatars": { "version": "3.7.0" },
                "com.vrchat.base": { "version": "3.7.0" },
                "nadena.dev.modular-avatar": { "version": "1.10.0" }
            }
        }"#;
        let manifest: RawManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.locked.len(), 3);
        assert_eq!(
            manifest.locked["nadena.dev.modular-avatar"]
                .version
                .as_deref(),
            Some("1.10.0")
        );
    }
}
