//! A reader for Unity's YAML serialization format (`.asset`, `.prefab`, `.unity`, `.meta`).
//!
//! Unity files are a multi-document YAML stream where each document is introduced by a header
//! line of the form `--- !u!<classID> &<fileID>` (optionally trailed by `stripped`). The class
//! id and file id live on that header line; the document *body* below it is ordinary YAML that a
//! standard parser can read. We therefore split on the header lines ourselves to recover the
//! class id / file id, then parse each body with `yaml-rust2`.
//!
//! This is a *reader*. It does not attempt byte-stable round-trip writing (see PLAN §8) — asset
//! generation will be a separate concern.

use anyhow::{Context, Result};

pub use yaml_rust2::Yaml;
use yaml_rust2::YamlLoader;

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
    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_inner(text, true)
    }

    /// Parse a Unity YAML file, **skipping** any document whose body fails to parse instead of
    /// failing the whole file. Unity occasionally serializes scalars (e.g. embedded scripts or
    /// odd quoting in large scenes) that `yaml-rust2` rejects; when a caller only needs a subset
    /// of object types (e.g. Transforms/MeshFilters for rendering), this keeps the rest usable.
    pub fn parse_lossy(text: &str) -> Self {
        Self::parse_inner(text, false).expect("lossy parse never errors")
    }

    fn parse_inner(text: &str, strict: bool) -> Result<Self> {
        let mut documents = Vec::new();

        for raw in split_documents(text) {
            let (header, body_text) = raw;
            let Some((class_id, file_id, stripped)) = parse_header(header) else {
                continue;
            };

            // A stripped header often has an empty body; skip parsing if so.
            let trimmed = body_text.trim();
            if trimmed.is_empty() {
                documents.push(UnityDocument {
                    class_id,
                    file_id,
                    stripped,
                    type_name: String::new(),
                    body: Yaml::Null,
                });
                continue;
            }

            let docs = match YamlLoader::load_from_str(&body_text) {
                Ok(docs) => docs,
                Err(e) if strict => {
                    return Err(e)
                        .with_context(|| format!("parsing Unity document (class {class_id})"));
                }
                // Lossy: drop the unparseable document and continue.
                Err(_) => continue,
            };
            let Some(doc) = docs.into_iter().next() else {
                continue;
            };

            let (type_name, body) = match doc.as_hash().and_then(|h| h.front()) {
                Some((k, v)) => (k.as_str().unwrap_or_default().to_string(), v.clone()),
                None => (String::new(), doc),
            };

            documents.push(UnityDocument {
                class_id,
                file_id,
                stripped,
                type_name,
                body,
            });
        }

        Ok(UnityFile { documents })
    }

    /// Iterate documents that are MonoBehaviours.
    pub fn monobehaviours(&self) -> impl Iterator<Item = &UnityDocument> {
        self.documents.iter().filter(|d| d.is_monobehaviour())
    }
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
fn parse_header(header: &str) -> Option<(u32, i64, bool)> {
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
}
