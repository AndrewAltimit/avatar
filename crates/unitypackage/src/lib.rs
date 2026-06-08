//! Reading Unity's `.unitypackage` distribution format.
//!
//! A `.unitypackage` is a gzip-compressed tar archive. Every asset is stored as a directory named
//! by its Unity **GUID**, containing:
//!
//! - `pathname` — the project-relative path the asset had when exported (e.g.
//!   `Assets/Avatar/final.fbx`). First line is the path; Unity sometimes appends extra lines.
//! - `asset` — the raw asset bytes (absent for folder entries).
//! - `asset.meta` — the Unity `.meta` sidecar (YAML; carries the same `guid`, import settings).
//! - `preview.png` — an optional thumbnail (ignored here).
//!
//! This crate parses that archive into a GUID-indexed [`UnityPackage`] and can [`extract`] it back
//! into a normal Unity `Assets/` tree (asset bytes at their pathname, `.meta` written alongside).
//! Once extracted, the rest of the toolchain — `avatar lint`, `avatar stats`, FBX/armature tools —
//! operates on it unchanged, since they only need files on disk plus their `.meta` GUID index.
//!
//! References: the format is undocumented but stable; see e.g.
//! <https://docs.unity3d.com/Manual/AssetPackagesImport.html>.

// Regression guard for an ingest crate: an `.unwrap()`/`.expect()` on a parse path turns a malformed
// user file into an opaque panic instead of a structured `anyhow` error an agent can read. Warn on
// them in non-test code — CI runs clippy with `-D warnings`, so a new one fails the build; tests use
// them freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::Serialize;

/// One asset recorded in a `.unitypackage`.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The exporting project's GUID for this asset (the archive directory name, also in the `.meta`).
    pub guid: String,
    /// Project-relative path, e.g. `Assets/Avatar/final.fbx`. `None` for a malformed entry with no
    /// `pathname` (rare; such entries are skipped on extract).
    pub pathname: Option<String>,
    /// Raw asset bytes. `None` for folder entries (a directory has a `pathname` but no `asset`).
    pub asset: Option<Vec<u8>>,
    /// The `.meta` sidecar bytes, if present.
    pub meta: Option<Vec<u8>>,
}

impl Entry {
    /// `true` if this entry carries asset bytes (i.e. is a file, not a folder placeholder).
    pub fn is_file(&self) -> bool {
        self.asset.is_some()
    }

    /// The asset's file extension, lowercased, without the dot (e.g. `"fbx"`). `None` if the
    /// pathname has no extension or the entry is a folder.
    pub fn extension(&self) -> Option<String> {
        if !self.is_file() {
            return None;
        }
        let p = self.pathname.as_deref()?;
        Path::new(p)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    }

    /// Size of the asset bytes, or 0 for a folder entry.
    pub fn size(&self) -> u64 {
        self.asset.as_ref().map_or(0, |b| b.len() as u64)
    }
}

/// A parsed `.unitypackage`, indexed by GUID in archive order.
#[derive(Debug, Clone, Default)]
pub struct UnityPackage {
    entries: BTreeMap<String, Entry>,
}

/// Decompression-bomb / unbounded-allocation guards. A `.unitypackage` is gzip+tar of untrusted
/// origin, so a tiny archive can claim (or actually expand to) an enormous member, and we buffer
/// every member in memory. These caps are far above any legitimate avatar/world export (real assets
/// — FBX, textures — are at most tens of MiB; whole packages a few hundred MiB) and exist only so a
/// pathologically/adversarially crafted archive bails with a clean error instead of exhausting RAM,
/// mirroring the `MAX_NODE_DEPTH` pattern in `avatar-fbx`.
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024; // 512 MiB per single archive member
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB across all retained members

impl UnityPackage {
    /// Open and fully parse a `.unitypackage` file from disk.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::read(file).with_context(|| format!("reading unitypackage {}", path.display()))
    }

    /// Parse a `.unitypackage` from any reader yielding the gzip stream.
    pub fn read<R: Read>(reader: R) -> Result<Self> {
        Self::read_capped(reader, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES)
    }

    /// Parse with explicit decompression caps. Internal; the public [`read`](Self::read) uses the
    /// production [`MAX_ENTRY_BYTES`]/[`MAX_TOTAL_BYTES`] values, while tests inject tiny caps to
    /// exercise the bomb guard without crafting multi-hundred-MiB fixtures.
    fn read_capped<R: Read>(reader: R, max_entry: u64, max_total: u64) -> Result<Self> {
        let gz = GzDecoder::new(reader);
        let mut archive = tar::Archive::new(gz);
        let mut entries: BTreeMap<String, Entry> = BTreeMap::new();
        let mut total: u64 = 0;

        for entry in archive.entries().context("iterating tar entries")? {
            let mut entry = entry.context("reading tar entry")?;
            let header_path = entry
                .path()
                .context("decoding tar entry path")?
                .into_owned();

            // Each archive member is `<guid>/<kind>`; directories themselves are skipped.
            let mut comps = header_path.components().filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            });
            let (Some(guid), Some(kind)) = (comps.next(), comps.next()) else {
                continue; // top-level dir entry or unexpected shape
            };
            if comps.next().is_some() {
                continue; // deeper than expected; not part of the format
            }

            // Read at most `max_entry + 1` bytes so we can detect an overrun without trusting the
            // tar header's declared size (a bomb can lie); `bail!` if the member exceeds the cap.
            let mut bytes = Vec::new();
            entry
                .by_ref()
                .take(max_entry + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("reading {}", header_path.display()))?;
            if bytes.len() as u64 > max_entry {
                bail!(
                    "archive member {} exceeds the per-entry size cap ({max_entry} bytes); \
                     refusing to process a possible decompression bomb",
                    header_path.display()
                );
            }
            total = total.saturating_add(bytes.len() as u64);
            if total > max_total {
                bail!(
                    "decompressed contents exceed the total size cap ({max_total} bytes); \
                     refusing to process a possible decompression bomb"
                );
            }

            let slot = entries.entry(guid.clone()).or_insert_with(|| Entry {
                guid: guid.clone(),
                pathname: None,
                asset: None,
                meta: None,
            });
            match kind.as_str() {
                "pathname" => slot.pathname = Some(parse_pathname(&bytes)),
                "asset" => slot.asset = Some(bytes),
                "asset.meta" => slot.meta = Some(bytes),
                "preview.png" => {} // thumbnail; not retained
                _ => {}             // unknown member; ignore for forward-compat
            }
        }

        if entries.is_empty() {
            bail!("no entries found; not a valid .unitypackage (expected <guid>/pathname members)");
        }
        Ok(UnityPackage { entries })
    }

    /// All entries, ordered by GUID.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// File entries only (those carrying asset bytes), ordered by GUID.
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values().filter(|e| e.is_file())
    }

    /// Look up an entry by GUID.
    pub fn get(&self, guid: &str) -> Option<&Entry> {
        self.entries.get(guid)
    }

    /// Find a file entry by its project-relative pathname (exact match).
    pub fn find_by_path(&self, pathname: &str) -> Option<&Entry> {
        self.entries
            .values()
            .find(|e| e.pathname.as_deref() == Some(pathname))
    }

    /// Total number of entries (files + folders).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A structured summary: counts, total size, breakdown by extension, and detected traits
    /// (SDK markers, avatar/world hints).
    pub fn summary(&self) -> PackageSummary {
        let mut by_extension: BTreeMap<String, ExtensionStat> = BTreeMap::new();
        let mut total_asset_bytes = 0u64;
        let mut file_count = 0usize;
        let mut folder_count = 0usize;
        for e in self.entries.values() {
            if e.is_file() {
                file_count += 1;
                total_asset_bytes += e.size();
                let ext = e.extension().unwrap_or_else(|| "(none)".to_string());
                let stat = by_extension.entry(ext).or_default();
                stat.count += 1;
                stat.bytes += e.size();
            } else {
                folder_count += 1;
            }
        }
        PackageSummary {
            entry_count: self.entries.len(),
            file_count,
            folder_count,
            total_asset_bytes,
            by_extension,
            traits: self.detect_traits(),
        }
    }

    /// Heuristic content detection from pathnames and the VRChat SDK markers.
    ///
    /// SDK2 vs SDK3 is read from the plugin DLLs and VPM package paths, NOT from
    /// `VRCSDK/version.txt` — that file is date-based (e.g. `2021.04.21...`), and the 2021-era
    /// bundled VRCSDK shipped *both* SDK2 and SDK3 editor scripts in one folder, so the source
    /// tree alone is ambiguous. The runtime plugin (`VRCSDK2.dll` vs `VRCSDK3*.dll`) and the VPM
    /// package (`com.vrchat.avatars`/`com.vrchat.worlds`) are the unambiguous signals.
    fn detect_traits(&self) -> PackageTraits {
        let mut traits = PackageTraits::default();
        let mut has_avatars_pkg = false;
        let mut has_worlds_pkg = false;
        let mut has_sdk2_dll = false;
        let mut has_sdk3_dll = false;

        for e in self.entries.values() {
            let Some(path) = e.pathname.as_deref() else {
                continue;
            };
            if path.contains("com.vrchat.avatars") {
                has_avatars_pkg = true;
            }
            if path.contains("com.vrchat.worlds") {
                has_worlds_pkg = true;
            }
            let file = path.rsplit('/').next().unwrap_or(path);
            // Match only the runtime plugin DLLs, not editor scripts that happen to mention SDK3.
            if let Some(stem) = file
                .strip_suffix(".dll")
                .or_else(|| file.strip_suffix(".DLL"))
            {
                let up = stem.to_ascii_uppercase();
                if up == "VRCSDK2" || up == "VRCSDK2-EDITOR" {
                    has_sdk2_dll = true;
                }
                // VRCSDK3, VRCSDK3A, VRCSDK3-Editor, etc.
                if up.starts_with("VRCSDK3") {
                    has_sdk3_dll = true;
                }
            }
            if path.ends_with("VRCSDK/version.txt")
                && let Some(bytes) = e.asset.as_deref()
            {
                traits.sdk_version_txt = Some(String::from_utf8_lossy(bytes).trim().to_string());
            }
            let lower = path.to_ascii_lowercase();
            if lower.ends_with(".prefab") {
                traits.prefab_count += 1;
            }
            if lower.ends_with(".unity") {
                traits.scene_count += 1;
            }
            // VRChat avatar prefabs exported from the SDK are named `prefab-id-v1_avtr_...`.
            if lower.contains("avtr_") && lower.ends_with(".prefab") {
                traits.looks_like_avatar = true;
            }
        }

        // Precedence: explicit VPM package > SDK3 plugin > SDK2 plugin > legacy folder present.
        traits.vrc_sdk = if has_worlds_pkg {
            Some(VrcSdk::Sdk3Worlds)
        } else if has_avatars_pkg || has_sdk3_dll {
            Some(VrcSdk::Sdk3Avatars)
        } else if has_sdk2_dll {
            Some(VrcSdk::Sdk2)
        } else if traits.sdk_version_txt.is_some() {
            Some(VrcSdk::Unknown)
        } else {
            None
        };

        // A package with scenes and no avatar prefab is most likely a world/map.
        if traits.scene_count > 0 && !traits.looks_like_avatar {
            traits.looks_like_world = true;
        }
        traits
    }

    /// Analyze what would happen if this package and `other` were imported into the *same* Unity
    /// project — the file-level "test this avatar inside this world" question.
    ///
    /// Two failure modes matter:
    /// - **GUID collisions**: the same GUID in both packages. If the asset bytes differ, Unity keeps
    ///   whichever was imported last, silently changing one package's asset (e.g. two SDK or shader
    ///   versions fighting). Identical bytes are a harmless duplicate.
    /// - **Path collisions**: different GUIDs claiming the same `Assets/...` path. The second import
    ///   overwrites the first file on disk while keeping a different GUID — references can dangle.
    pub fn overlap(&self, other: &UnityPackage) -> OverlapReport {
        let mut guid_collisions = Vec::new();
        for (guid, a) in &self.entries {
            let Some(b) = other.entries.get(guid) else {
                continue;
            };
            // Compare only file entries; folder-vs-folder GUID reuse is benign.
            if !a.is_file() && !b.is_file() {
                continue;
            }
            let identical = a.asset == b.asset;
            guid_collisions.push(GuidCollision {
                guid: guid.clone(),
                path_a: a.pathname.clone(),
                path_b: b.pathname.clone(),
                identical,
            });
        }

        // path -> guid, files only, for cross-package path conflicts.
        let path_index = |p: &UnityPackage| {
            let mut m: BTreeMap<String, String> = BTreeMap::new();
            for e in p.files() {
                if let Some(path) = e.pathname.as_deref() {
                    m.insert(path.to_string(), e.guid.clone());
                }
            }
            m
        };
        let a_paths = path_index(self);
        let b_paths = path_index(other);
        let mut path_collisions = Vec::new();
        for (path, guid_a) in &a_paths {
            if let Some(guid_b) = b_paths.get(path)
                && guid_a != guid_b
            {
                path_collisions.push(PathCollision {
                    path: path.clone(),
                    guid_a: guid_a.clone(),
                    guid_b: guid_b.clone(),
                });
            }
        }

        OverlapReport {
            guid_collisions,
            path_collisions,
        }
    }

    /// Extract every file entry into `dest`, reconstructing the project tree: each asset at
    /// `dest/<pathname>` with its `.meta` written as `dest/<pathname>.meta`. Returns the extraction
    /// report (counts, any skipped/unsafe entries). Existing files are overwritten.
    pub fn extract(&self, dest: &Path) -> Result<ExtractReport> {
        let mut report = ExtractReport::default();
        for e in self.entries.values() {
            let Some(rel) = e.pathname.as_deref() else {
                report.skipped_no_pathname += 1;
                continue;
            };
            let Some(target) = safe_join(dest, rel) else {
                report.skipped_unsafe.push(rel.to_string());
                continue;
            };
            if let Some(asset) = e.asset.as_deref() {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .with_context(|| format!("creating {}", parent.display()))?;
                }
                std::fs::write(&target, asset)
                    .with_context(|| format!("writing {}", target.display()))?;
                report.files_written += 1;
                report.bytes_written += asset.len() as u64;

                if let Some(meta) = e.meta.as_deref() {
                    let meta_path = append_ext(&target, "meta");
                    std::fs::write(&meta_path, meta)
                        .with_context(|| format!("writing {}", meta_path.display()))?;
                    report.meta_written += 1;
                }
            } else {
                // Folder entry: materialize the directory so empty folders survive.
                std::fs::create_dir_all(&target)
                    .with_context(|| format!("creating {}", target.display()))?;
                report.folders_created += 1;
            }
        }
        Ok(report)
    }
}

/// Per-extension tally in a [`PackageSummary`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtensionStat {
    pub count: usize,
    pub bytes: u64,
}

/// Summary statistics for a `.unitypackage`.
#[derive(Debug, Clone, Serialize)]
pub struct PackageSummary {
    pub entry_count: usize,
    pub file_count: usize,
    pub folder_count: usize,
    pub total_asset_bytes: u64,
    pub by_extension: BTreeMap<String, ExtensionStat>,
    pub traits: PackageTraits,
}

/// Which VRChat SDK a package appears to target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VrcSdk {
    /// Legacy SDK2 (`VRCSDK/version.txt` with a 2.x line).
    Sdk2,
    /// SDK3 Avatars (`com.vrchat.avatars` or an SDK3 `VRCSDK/version.txt`).
    Sdk3Avatars,
    /// SDK3 Worlds (`com.vrchat.worlds`).
    Sdk3Worlds,
    /// VRChat SDK present but version not classifiable.
    Unknown,
}

/// Heuristic content traits detected from a package's contents.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PackageTraits {
    pub vrc_sdk: Option<VrcSdk>,
    /// Raw contents of `VRCSDK/version.txt` if found.
    pub sdk_version_txt: Option<String>,
    pub prefab_count: usize,
    pub scene_count: usize,
    /// A VRChat avatar prefab (`avtr_*`) was found.
    pub looks_like_avatar: bool,
    /// Scenes present and no avatar prefab — most likely a world/map.
    pub looks_like_world: bool,
}

/// A GUID present in both packages of an [`UnityPackage::overlap`].
#[derive(Debug, Clone, Serialize)]
pub struct GuidCollision {
    pub guid: String,
    pub path_a: Option<String>,
    pub path_b: Option<String>,
    /// `true` if both packages carry byte-identical asset content (a harmless duplicate).
    pub identical: bool,
}

/// Two different GUIDs claiming the same project path across packages.
#[derive(Debug, Clone, Serialize)]
pub struct PathCollision {
    pub path: String,
    pub guid_a: String,
    pub guid_b: String,
}

/// Result of comparing two packages for co-import conflicts ([`UnityPackage::overlap`]).
#[derive(Debug, Clone, Serialize)]
pub struct OverlapReport {
    pub guid_collisions: Vec<GuidCollision>,
    pub path_collisions: Vec<PathCollision>,
}

impl OverlapReport {
    /// GUID collisions whose asset bytes differ — the ones that silently mutate an asset on import.
    pub fn conflicting(&self) -> impl Iterator<Item = &GuidCollision> {
        self.guid_collisions.iter().filter(|c| !c.identical)
    }

    /// `true` if importing both packages together is fully safe (no GUID/path conflicts at all).
    pub fn is_clean(&self) -> bool {
        self.guid_collisions.is_empty() && self.path_collisions.is_empty()
    }
}

/// Outcome of [`UnityPackage::extract`].
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExtractReport {
    pub files_written: usize,
    pub meta_written: usize,
    pub folders_created: usize,
    pub bytes_written: u64,
    pub skipped_no_pathname: usize,
    /// Pathnames rejected as not cleanly project-relative: absolute POSIX paths, Windows drive /
    /// UNC roots, or `..` traversal. These are never legitimate project assets.
    pub skipped_unsafe: Vec<String>,
}

/// Extract the asset path from a `pathname` member: the first non-empty line, trimmed. Unity
/// occasionally appends trailing metadata lines; we keep only the path.
fn parse_pathname(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Join `rel` onto `base`, refusing anything that is not a clean project-relative path: absolute
/// POSIX paths, Windows drive letters (`C:/…`) or UNC roots (`//…`), and `..` traversal. Returns
/// `None` if unsafe. Normalizes Windows-style `\` separators. Drive-letter rejection matters on
/// non-Windows hosts, where `Path::components` treats `C:` as a normal segment — old SDK exports
/// sometimes leak absolute paths to bundled editor DLLs that are not project assets at all.
fn safe_join(base: &Path, rel: &str) -> Option<PathBuf> {
    let normalized = rel.replace('\\', "/");
    // Drive-letter prefix (`C:` / `c:/...`) — reject regardless of host OS path parsing.
    let first_seg = normalized.split('/').next().unwrap_or("");
    if first_seg.len() == 2
        && first_seg.as_bytes()[0].is_ascii_alphabetic()
        && first_seg.as_bytes()[1] == b':'
    {
        return None;
    }
    let candidate = Path::new(&normalized);
    let mut out = base.to_path_buf();
    let mut pushed_any = false;
    for comp in candidate.components() {
        match comp {
            Component::Normal(s) => {
                out.push(s);
                pushed_any = true;
            }
            Component::CurDir => {}
            // Absolute roots, prefixes, and parent-dir traversal are all rejected.
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => return None,
        }
    }
    if pushed_any { Some(out) } else { None }
}

/// Return `path` with `ext` appended after its existing extension, e.g. `final.fbx` -> `final.fbx.meta`.
fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests;
