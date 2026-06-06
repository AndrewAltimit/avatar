//! Parsing of VRChat's per-avatar **OSC config JSON** (a.k.a. the OSCQuery avatar node tree).
//!
//! When VRChat loads an avatar it writes a JSON file to
//! `…/VRChat/VRChat/OSC/<usr_id>/Avatars/<avtr_id>.json` describing every parameter the avatar
//! exposes over OSC — its full address, its OSC type tag, and whether the game reads/writes it.
//! Parsing that file lets a daemon know an avatar's parameter schema **offline**, without probing
//! the live OSCQuery HTTP endpoint.
//!
//! The file is an OSCQuery *host info* root whose `CONTENTS` is a recursive tree of nodes. Each node
//! may carry `FULL_PATH`, `TYPE` (an OSC type-tag string like `"f"`), `ACCESS` (a bitmask:
//! 1 = read, 2 = write, 3 = read/write), `VALUE` (the current value), and further `CONTENTS`. The
//! parameters we care about are the leaves under `/avatar/parameters`.
//!
//! Reference: <https://github.com/Vidvox/OSCQueryProposal> and VRChat's OSC overview.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Read/write access a node declares, mirroring OSCQuery's `ACCESS` bitmask
/// (1 = read-only, 2 = write-only, 3 = read/write, 0 = no value / container).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    None,
    Read,
    Write,
    ReadWrite,
}

impl Access {
    fn from_bits(bits: u8) -> Access {
        match bits & 0b11 {
            0 => Access::None,
            1 => Access::Read,
            2 => Access::Write,
            _ => Access::ReadWrite,
        }
    }

    /// Whether VRChat will *send* this parameter's value to us (the daemon can read it).
    pub fn is_readable(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }

    /// Whether VRChat will *accept* our writes to this parameter.
    pub fn is_writable(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }
}

/// One avatar parameter, flattened out of the OSCQuery node tree.
#[derive(Debug, Clone, PartialEq)]
pub struct AvatarParam {
    /// Full OSC address, e.g. `/avatar/parameters/VRCEmote`.
    pub full_path: String,
    /// The parameter name (the bit after `/avatar/parameters/`), if the path is a parameter address.
    pub name: Option<String>,
    /// OSC type tag string: `"f"`, `"i"`, `"T"`/`"F"` (bool), etc. Empty for containers.
    pub type_tag: String,
    /// Declared read/write access.
    pub access: Access,
}

impl AvatarParam {
    /// Whether this leaf is an `/avatar/parameters/<Name>` entry (vs. a structural container or a
    /// non-parameter node such as `/avatar/change`).
    pub fn is_avatar_parameter(&self) -> bool {
        self.name.is_some()
    }
}

/// An avatar's full OSC parameter schema, parsed from its OSCQuery config JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct AvatarConfig {
    /// The node tree's display name (`name` at the root — usually the avatar's name).
    pub name: String,
    /// Every value-bearing leaf in the tree, in document order.
    pub params: Vec<AvatarParam>,
}

impl AvatarConfig {
    /// Parse from the JSON text of an avatar OSC config file.
    pub fn from_json(text: &str) -> Result<AvatarConfig> {
        let root: Node =
            serde_json::from_str(text).context("parsing avatar OSC config (OSCQuery) JSON")?;
        let mut params = Vec::new();
        collect(&root, &mut params);
        Ok(AvatarConfig {
            name: root.name.unwrap_or_default(),
            params,
        })
    }

    /// Read and parse an avatar OSC config file from disk.
    pub fn from_path(path: impl AsRef<Path>) -> Result<AvatarConfig> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading avatar OSC config {}", path.display()))?;
        AvatarConfig::from_json(&text)
    }

    /// Look up a parameter by its name (the bit after `/avatar/parameters/`).
    pub fn param(&self, name: &str) -> Option<&AvatarParam> {
        self.params.iter().find(|p| p.name.as_deref() == Some(name))
    }

    /// Just the `/avatar/parameters/*` leaves (filtering out containers and `/avatar/change`).
    pub fn avatar_parameters(&self) -> impl Iterator<Item = &AvatarParam> {
        self.params.iter().filter(|p| p.is_avatar_parameter())
    }
}

/// Raw OSCQuery node as it appears in the JSON. Field names are SCREAMING_SNAKE_CASE in the file;
/// `serde(rename)` maps them. `VALUE` is intentionally ignored — schemas, not live values, are the
/// point of parsing the file offline.
#[derive(Debug, Deserialize)]
struct Node {
    #[serde(rename = "FULL_PATH")]
    full_path: Option<String>,
    #[serde(rename = "TYPE")]
    type_tag: Option<String>,
    #[serde(rename = "ACCESS")]
    access: Option<u8>,
    #[serde(rename = "CONTENTS")]
    contents: Option<std::collections::BTreeMap<String, Node>>,
    // Present only at the root; harmless elsewhere.
    name: Option<String>,
}

/// The address prefix avatar parameters live under in the config tree.
const PARAM_PREFIX: &str = "/avatar/parameters/";

/// Depth-first flatten: emit a leaf for any node that declares a `TYPE` (i.e. carries a value), and
/// recurse into `CONTENTS`. Container-only nodes (no `TYPE`) are skipped as parameters but still
/// walked. `BTreeMap` gives a stable (alphabetical) order so output is deterministic.
fn collect(node: &Node, out: &mut Vec<AvatarParam>) {
    if let (Some(full_path), Some(type_tag)) = (&node.full_path, &node.type_tag) {
        let name = full_path
            .strip_prefix(PARAM_PREFIX)
            .map(str::to_string)
            .filter(|n| !n.is_empty());
        out.push(AvatarParam {
            full_path: full_path.clone(),
            name,
            type_tag: type_tag.clone(),
            access: Access::from_bits(node.access.unwrap_or(0)),
        });
    }
    if let Some(contents) = &node.contents {
        for child in contents.values() {
            collect(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed but structurally faithful avatar OSC config: a root container → `avatar` →
    /// `change` (string) + `parameters` container → three typed leaves.
    const SAMPLE: &str = r#"{
        "name": "TestAvatar",
        "FULL_PATH": "/",
        "ACCESS": 0,
        "CONTENTS": {
            "avatar": {
                "FULL_PATH": "/avatar",
                "ACCESS": 0,
                "CONTENTS": {
                    "change": {
                        "FULL_PATH": "/avatar/change",
                        "TYPE": "s",
                        "ACCESS": 1,
                        "VALUE": ["avtr_0000"]
                    },
                    "parameters": {
                        "FULL_PATH": "/avatar/parameters",
                        "ACCESS": 0,
                        "CONTENTS": {
                            "VRCEmote": {
                                "FULL_PATH": "/avatar/parameters/VRCEmote",
                                "TYPE": "i",
                                "ACCESS": 3,
                                "VALUE": [0]
                            },
                            "GestureLeftWeight": {
                                "FULL_PATH": "/avatar/parameters/GestureLeftWeight",
                                "TYPE": "f",
                                "ACCESS": 1,
                                "VALUE": [0.0]
                            },
                            "Grounded": {
                                "FULL_PATH": "/avatar/parameters/Grounded",
                                "TYPE": "T",
                                "ACCESS": 1,
                                "VALUE": [true]
                            }
                        }
                    }
                }
            }
        }
    }"#;

    #[test]
    fn parses_name_and_param_count() {
        let cfg = AvatarConfig::from_json(SAMPLE).unwrap();
        assert_eq!(cfg.name, "TestAvatar");
        // change + 3 parameter leaves carry a TYPE; containers don't.
        assert_eq!(cfg.params.len(), 4);
        assert_eq!(cfg.avatar_parameters().count(), 3);
    }

    #[test]
    fn resolves_param_types_and_access() {
        let cfg = AvatarConfig::from_json(SAMPLE).unwrap();
        let emote = cfg.param("VRCEmote").unwrap();
        assert_eq!(emote.full_path, "/avatar/parameters/VRCEmote");
        assert_eq!(emote.type_tag, "i");
        assert_eq!(emote.access, Access::ReadWrite);
        assert!(emote.access.is_readable() && emote.access.is_writable());

        let weight = cfg.param("GestureLeftWeight").unwrap();
        assert_eq!(weight.type_tag, "f");
        assert_eq!(weight.access, Access::Read);
        assert!(weight.access.is_readable() && !weight.access.is_writable());
    }

    #[test]
    fn change_node_is_not_an_avatar_parameter() {
        let cfg = AvatarConfig::from_json(SAMPLE).unwrap();
        let change = cfg
            .params
            .iter()
            .find(|p| p.full_path == "/avatar/change")
            .unwrap();
        assert!(change.name.is_none());
        assert!(!change.is_avatar_parameter());
        assert!(cfg.param("change").is_none());
    }

    #[test]
    fn access_bitmask_decodes() {
        assert_eq!(Access::from_bits(0), Access::None);
        assert_eq!(Access::from_bits(1), Access::Read);
        assert_eq!(Access::from_bits(2), Access::Write);
        assert_eq!(Access::from_bits(3), Access::ReadWrite);
    }

    #[test]
    fn malformed_json_errors() {
        assert!(AvatarConfig::from_json("{ not json").is_err());
    }

    #[test]
    fn empty_tree_yields_no_params() {
        let cfg = AvatarConfig::from_json(r#"{"name":"X","FULL_PATH":"/","ACCESS":0}"#).unwrap();
        assert_eq!(cfg.name, "X");
        assert!(cfg.params.is_empty());
    }
}
