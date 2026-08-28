//! A reader for Unity's YAML serialization format (`.asset`, `.prefab`, `.unity`, `.meta`).
//!
//! Unity files are a multi-document YAML stream where each document is introduced by a header
//! line of the form `--- !u!<classID> &<fileID>` (optionally trailed by `stripped`). The class
//! id and file id live on that header line; the document *body* below it is ordinary YAML that a
//! standard parser can read. We therefore split on the header lines ourselves to recover the
//! class id / file id, then parse each body with `yaml-rust2`.
//!
//! This crate reads Unity YAML; surgical, round-trip-safe *editing* lives in [`edit`]
//! ([`EditableUnityFile`]). Generating whole new assets is a separate concern (`avatar-anim-gen`).

// Regression guard for an ingest crate: an `.unwrap()`/`.expect()` on a parse path turns a malformed
// user file into an opaque panic instead of a structured `anyhow` error an agent can read. Warn on
// them in non-test code — CI runs clippy with `-D warnings`, so a new one fails the build; tests use
// them freely.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use yaml_rust2::Yaml;
use yaml_rust2::YamlLoader;

pub mod edit;
pub use edit::{EditableUnityFile, Scalar, Seg, parse_path};

/// One document within a Unity YAML file.
#[derive(Debug, Clone)]
pub struct UnityDocument {
    /// Unity class id from the `!u!<classID>` tag (e.g. `114` = MonoBehaviour, `1` = GameObject).
    pub class_id: u32,
    /// The `&<fileID>` anchor identifying this object within the file.
    pub file_id: i64,
    /// `true` if the header was marked `stripped` (a prefab-instance placeholder).
    pub stripped: bool,
    /// The top-level type key of the body, e.g. `"MonoBehaviour"`, `"GameObject"`, `"Transform"`.
    pub type_name: String,
    /// The document body (the mapping under `type_name`).
    pub body: Yaml,
}

impl UnityDocument {
    /// True if this document is a `MonoBehaviour` (Unity class id 114).
    pub fn is_monobehaviour(&self) -> bool {
        self.class_id == 114
    }

    /// The `m_Script` GUID for a MonoBehaviour, if present.
    pub fn script_guid(&self) -> Option<&str> {
        self.body["m_Script"]["guid"].as_str()
    }

    /// The `m_Name` of the object, if present.
    pub fn name(&self) -> Option<&str> {
        self.body["m_Name"].as_str()
    }
}

/// A parsed Unity YAML file.
#[derive(Debug, Clone)]
pub struct UnityFile {
    pub documents: Vec<UnityDocument>,
}

impl UnityFile {
    /// Parse a Unity YAML file from text. Fails if any document body is not valid YAML.
    ///
    /// # Example
    ///
    /// ```
    /// use avatar_unity_yaml::UnityFile;
    ///
    /// let text = "\
    /// %YAML 1.1
    /// %TAG !u! tag:unity3d.com,2011:
    /// --- !u!114 &11400000
    /// MonoBehaviour:
    ///   m_Name: Parameters
    /// ";
    /// let file = UnityFile::parse(text)?;
    /// let doc = &file.documents[0];
    /// assert_eq!(doc.class_id, 114);
    /// assert_eq!(doc.file_id, 11400000);
    /// assert!(doc.is_monobehaviour());
    /// assert_eq!(doc.name(), Some("Parameters"));
    /// # anyhow::Ok(())
    /// ```
    pub fn parse(text: &str) -> Result<Self> {
        let mut documents = Vec::new();
        for (header, body_text) in split_documents(text) {
            // Strict: a body `yaml-rust2` rejects aborts the whole file with context.
            if let Some(doc) = parse_one(header, &body_text)? {
                documents.push(doc);
            }
        }
        Ok(UnityFile { documents })
    }

    /// Parse a Unity YAML file, **skipping** any document whose body fails to parse instead of
    /// failing the whole file. Unity occasionally serializes scalars (e.g. embedded scripts or
    /// odd quoting in large scenes) that `yaml-rust2` rejects; when a caller only needs a subset
    /// of object types (e.g. Transforms/MeshFilters for rendering), this keeps the rest usable.
    ///
    /// Infallible by construction: the per-document parse error is simply dropped, so there is no
    /// panic path (the previous `.expect(...)` is gone).
    pub fn parse_lossy(text: &str) -> Self {
        let mut documents = Vec::new();
        for (header, body_text) in split_documents(text) {
            // Lossy: an unparseable document yields `Err`, which we drop and continue.
            if let Ok(Some(doc)) = parse_one(header, &body_text) {
                documents.push(doc);
            }
        }
        UnityFile { documents }
    }

    /// Iterate documents that are MonoBehaviours.
    pub fn monobehaviours(&self) -> impl Iterator<Item = &UnityDocument> {
        self.documents.iter().filter(|d| d.is_monobehaviour())
    }
}

/// Parse one `(header, body)` pair into a [`UnityDocument`]. Returns:
/// - `Ok(Some(doc))` for a recognised document (including stripped/empty bodies),
/// - `Ok(None)` to skip (the line wasn't a `--- !u!` header, or the body parsed to nothing),
/// - `Err(_)` if `yaml-rust2` rejects the body.
///
/// [`UnityFile::parse`] propagates the `Err`; [`UnityFile::parse_lossy`] drops it. Pulling the
/// fallible work into this helper is what lets the lossy path be infallible by construction.
fn parse_one(header: &str, body_text: &str) -> Result<Option<UnityDocument>> {
    let Some((class_id, file_id, stripped)) = parse_header(header) else {
        return Ok(None);
    };

    // A stripped header often has an empty body; skip parsing if so.
    if body_text.trim().is_empty() {
        return Ok(Some(UnityDocument {
            class_id,
            file_id,
            stripped,
            type_name: String::new(),
            body: Yaml::Null,
        }));
    }

    let docs = YamlLoader::load_from_str(body_text)
        .with_context(|| format!("parsing Unity document (class {class_id})"))?;
    let Some(doc) = docs.into_iter().next() else {
        return Ok(None);
    };

    let (type_name, body) = match doc.as_hash().and_then(|h| h.front()) {
        Some((k, v)) => (k.as_str().unwrap_or_default().to_string(), v.clone()),
        None => (String::new(), doc),
    };

    Ok(Some(UnityDocument {
        class_id,
        file_id,
        stripped,
        type_name,
        body,
    }))
}

/// Read the `guid` from a Unity `.meta` file's text, if present.
pub fn meta_guid(meta_text: &str) -> Option<String> {
    let docs = YamlLoader::load_from_str(meta_text).ok()?;
    docs.first()?["guid"].as_str().map(str::to_string)
}

/// Parse a single-document YAML file (e.g. a `.meta`, which is *not* the multi-document `--- !u!`
/// stream [`UnityFile`] reads) into its root node. Returns `None` if it doesn't parse.
pub fn parse_meta(text: &str) -> Option<Yaml> {
    YamlLoader::load_from_str(text).ok()?.into_iter().next()
}

/// Split a Unity YAML stream into `(header_line, body_text)` pairs, one per document. Content
/// before the first `---` header (the `%YAML` / `%TAG` directives) is dropped.
fn split_documents(text: &str) -> Vec<(&str, String)> {
    let mut out = Vec::new();
    let mut current_header: Option<&str> = None;
    let mut body = String::new();

    for line in text.lines() {
        if line.starts_with("---") {
            if let Some(h) = current_header.take() {
                out.push((h, std::mem::take(&mut body)));
            }
            current_header = Some(line);
        } else if current_header.is_some() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(h) = current_header.take() {
        out.push((h, body));
    }
    out
}

/// Parse a `--- !u!<classID> &<fileID> [stripped]` header into `(class_id, file_id, stripped)`.
pub(crate) fn parse_header(header: &str) -> Option<(u32, i64, bool)> {
    let stripped = header.contains("stripped");
    let class_id = header
        .split("!u!")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<u32>()
        .ok()?;
    let file_id = header
        .split('&')
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse::<i64>()
        .ok()?;
    Some((class_id, file_id, stripped))
}

/// Convenience: read an `i64` field from a Yaml mapping (accepts integer or numeric string).
pub fn field_i64(node: &Yaml, key: &str) -> Option<i64> {
    let v = &node[key];
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Convenience: read an `f64` field (accepts real, integer, or numeric string).
pub fn field_f64(node: &Yaml, key: &str) -> Option<f64> {
    let v = &node[key];
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Convenience: read a `bool`-ish field. Unity serializes bools as `0`/`1`.
pub fn field_bool(node: &Yaml, key: &str) -> Option<bool> {
    let v = &node[key];
    v.as_bool()
        .or_else(|| v.as_i64().map(|i| i != 0))
        .or_else(|| match v.as_str()? {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        })
}

/// Convenience: read a string field.
pub fn field_str<'a>(node: &'a Yaml, key: &str) -> Option<&'a str> {
    node[key].as_str()
}

/// Read the `guid` of a `{fileID, guid, type}` reference stored under `key` (a cross-asset
/// reference). Returns `None` for in-file (`fileID`-only) references.
pub fn ref_guid<'a>(node: &'a Yaml, key: &str) -> Option<&'a str> {
    field_str(&node[key], "guid")
}

/// Read the `fileID` of a `{fileID, guid, type}` reference stored under `key`.
pub fn ref_fileid(node: &Yaml, key: &str) -> Option<i64> {
    field_i64(&node[key], "fileID")
}

/// 64-bit FNV-1a hash. Stable across platforms and runs (unlike `DefaultHasher`, which is *not*
/// guaranteed stable), so derived values (e.g. generated fileIDs, texture fingerprints) are
/// reproducible.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Unity's `m_Script` fileID for a class compiled into a DLL: the first four bytes (little-endian
/// `i32`) of MD4 over `"s\0\0\0" + namespace + class_name`. This is how every `{fileID: N, guid:
/// <dll guid>, type: 3}` reference to a plugin script is derived (a loose `.cs` script instead
/// gets the fixed `11500000`). Verified against the VRChat SDK's own serialized assets — e.g.
/// `VRC.SDK3.Avatars.Components.VRCAvatarDescriptor` → `542108242`.
pub fn script_file_id(namespace: &str, class_name: &str) -> i32 {
    let mut input = b"s\0\0\0".to_vec();
    input.extend_from_slice(namespace.as_bytes());
    input.extend_from_slice(class_name.as_bytes());
    let digest = md4(&input);
    i32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// MD4 (RFC 1320). Only used for [`script_file_id`]; not a general-purpose hash.
fn md4(data: &[u8]) -> [u8; 16] {
    fn f(x: u32, y: u32, z: u32) -> u32 {
        (x & y) | (!x & z)
    }
    fn g(x: u32, y: u32, z: u32) -> u32 {
        (x & y) | (x & z) | (y & z)
    }
    fn h(x: u32, y: u32, z: u32) -> u32 {
        x ^ y ^ z
    }
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) = (
        0x6745_2301u32,
        0xefcd_ab89u32,
        0x98ba_dcfeu32,
        0x1032_5476u32,
    );
    for block in msg.as_chunks::<64>().0 {
        let mut x = [0u32; 16];
        for (i, w) in block.as_chunks::<4>().0.iter().enumerate() {
            x[i] = u32::from_le_bytes(*w);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        const S1: [u32; 4] = [3, 7, 11, 19];
        for i in 0..16 {
            let t = a
                .wrapping_add(f(b, c, d))
                .wrapping_add(x[i])
                .rotate_left(S1[i % 4]);
            (a, b, c, d) = (d, t, b, c);
        }
        const K2: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
        const S2: [u32; 4] = [3, 5, 9, 13];
        for i in 0..16 {
            let t = a
                .wrapping_add(g(b, c, d))
                .wrapping_add(x[K2[i]])
                .wrapping_add(0x5a82_7999)
                .rotate_left(S2[i % 4]);
            (a, b, c, d) = (d, t, b, c);
        }
        const K3: [usize; 16] = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15];
        const S3: [u32; 4] = [3, 9, 11, 15];
        for i in 0..16 {
            let t = a
                .wrapping_add(h(b, c, d))
                .wrapping_add(x[K3[i]])
                .wrapping_add(0x6ed9_eba1)
                .rotate_left(S3[i % 4]);
            (a, b, c, d) = (d, t, b, c);
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Recursively collect every file under `root` (directories are descended, not emitted). Returns
/// an empty vec if `root` is unreadable.
pub fn walk_assets(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_into(root, &mut out);
    out
}

fn walk_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_into(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Build a `guid -> asset path` index from a set of files: every `.meta` whose `guid` parses maps
/// to the asset it describes (the `.meta` path with the trailing `.meta` stripped). A
/// `Foo.fbx.meta` describes `Foo.fbx`; references elsewhere point at it by this guid.
pub fn build_guid_index(files: &[PathBuf]) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    for path in files {
        if path.extension().is_none_or(|e| e != "meta") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(guid) = meta_guid(&text) {
            index.insert(guid, path.with_extension("")); // strip ".meta"
        }
    }
    index
}

/// A `path` rendered relative to `root` (falling back to the full path if it isn't under `root`).
pub fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The `.meta` sidecar path for an asset (`Foo.png` → `Foo.png.meta`).
pub fn meta_path(path: &Path) -> PathBuf {
    let mut s = path.to_path_buf().into_os_string();
    s.push(".meta");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!114 &11400000
MonoBehaviour:
  m_ObjectHideFlags: 0
  m_Script: {fileID: 11500000, guid: abcdef0123456789abcdef0123456789, type: 3}
  m_Name: Parameters
  parameters:
  - name: VRCEmote
    valueType: 0
    saved: 1
    defaultValue: 0
    networkSynced: 1
";

    // A second doc whose body has a quoted scalar with indentation yaml-rust2 rejects, between two
    // well-formed Transform docs — mirrors what large Unity scenes contain.
    const WITH_BAD_DOC: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!4 &100
Transform:
  m_GameObject: {fileID: 1}
--- !u!114 &200
MonoBehaviour:
  m_Name: Bad
  data: \"unterminated quote
--- !u!4 &300
Transform:
  m_GameObject: {fileID: 3}
";

    #[test]
    fn parse_strict_fails_on_bad_doc_but_lossy_skips_it() {
        assert!(UnityFile::parse(WITH_BAD_DOC).is_err());

        let file = UnityFile::parse_lossy(WITH_BAD_DOC);
        let ids: Vec<i64> = file.documents.iter().map(|d| d.file_id).collect();
        // Both Transforms survive; the unparseable MonoBehaviour is dropped.
        assert!(ids.contains(&100), "first Transform kept");
        assert!(ids.contains(&300), "Transform after the bad doc kept");
        assert!(!ids.contains(&200), "unparseable doc dropped");
        assert_eq!(file.documents.iter().filter(|d| d.class_id == 4).count(), 2);
    }

    #[test]
    fn parse_lossy_does_not_panic_on_garbage() {
        // Inputs that previously risked the `.expect(...)` panic: pure garbage, headers with
        // wildly malformed bodies, and empty text. None must panic; all return a `UnityFile`.
        for text in [
            "",
            "not yaml at all\n\t\0 \x07 ::: ][}{",
            "--- !u!114 &200\n\tMonoBehaviour:\n  : : : broken\n   - - mixed\n\tbad indent: [",
            "--- !u!1 &1\n--- garbage --- !u! &&&\n\u{feff}",
        ] {
            let file = UnityFile::parse_lossy(text);
            // documents.len() is always defined; just touch it so the call isn't optimized away.
            let _ = file.documents.len();
        }
    }

    #[test]
    fn parses_header_and_body() {
        let file = UnityFile::parse(SAMPLE).unwrap();
        assert_eq!(file.documents.len(), 1);
        let d = &file.documents[0];
        assert_eq!(d.class_id, 114);
        assert_eq!(d.file_id, 11400000);
        assert_eq!(d.type_name, "MonoBehaviour");
        assert!(d.is_monobehaviour());
        assert_eq!(d.name(), Some("Parameters"));
        assert_eq!(d.script_guid(), Some("abcdef0123456789abcdef0123456789"));
    }

    #[test]
    fn reads_nested_parameter_fields() {
        let file = UnityFile::parse(SAMPLE).unwrap();
        let body = &file.documents[0].body;
        let params = body["parameters"].as_vec().unwrap();
        assert_eq!(params.len(), 1);
        assert_eq!(field_str(&params[0], "name"), Some("VRCEmote"));
        assert_eq!(field_i64(&params[0], "valueType"), Some(0));
        assert_eq!(field_bool(&params[0], "networkSynced"), Some(true));
    }

    #[test]
    fn reads_meta_guid() {
        let meta = "fileFormatVersion: 2\nguid: 0123456789abcdef0123456789abcdef\n";
        assert_eq!(
            meta_guid(meta).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn script_file_id_matches_vrchat_sdk_serialized_values() {
        // Read off com.vrchat.avatars / com.vrchat.base 3.10.4 sample assets.
        assert_eq!(
            script_file_id("VRC.SDK3.Avatars.Components", "VRCAvatarDescriptor"),
            542108242
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Dynamics.PhysBone.Components", "VRCPhysBone"),
            1661641543
        );
        assert_eq!(
            script_file_id(
                "VRC.SDK3.Dynamics.PhysBone.Components",
                "VRCPhysBoneCollider"
            ),
            -1631200402
        );
        assert_eq!(
            script_file_id(
                "VRC.SDK3.Avatars.ScriptableObjects",
                "VRCExpressionParameters"
            ),
            -1506855854
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Avatars.ScriptableObjects", "VRCExpressionsMenu"),
            -340790334
        );
        assert_eq!(
            script_file_id("VRC.SDK3.Dynamics.Contact.Components", "VRCContactReceiver"),
            -1450912254
        );
    }
}
