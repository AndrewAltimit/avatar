# avatar-unity-yaml

A reader **and surgical editor** for Unity's YAML serialization format. Package `avatar-unity-yaml` ·
library `avatar_unity_yaml`. Part of the [avatar](../../README.md) monorepo.

## What it does

Reads Unity files (`.asset`, `.prefab`, `.unity`, `.meta`), which are a multi-document YAML stream
where each document is introduced by a header line `--- !u!<classID> &<fileID>` (optionally trailed
by `stripped`). The class id and file id live on that header line; the body below it is ordinary
YAML. This crate splits on the header lines itself to recover the class/file ids, then parses each
body with [`yaml-rust2`](https://crates.io/crates/yaml-rust2).

Reading does not round-trip through `yaml-rust2` (which drops formatting, key order, and the `--- !u!`
headers). For *editing* an existing asset, `EditableUnityFile` instead **span-splices the raw text** —
it replaces only the bytes of the value you change, leaving every `&fileID`, `{fileID, guid}`
reference, key order, and byte of formatting untouched, then re-parses to validate. Generating *whole*
new assets is a separate concern (`avatar-anim-gen`). Full detail:
[`docs/reference/unity-yaml-edit.md`](../../docs/reference/unity-yaml-edit.md).

## Key API

- `UnityFile::parse(text) -> Result<UnityFile>` — split + parse a Unity YAML stream;
  `monobehaviours()` iterates the MonoBehaviour documents.
- `UnityDocument` — one document: class id, file id, and helpers `is_monobehaviour()`,
  `script_guid()`, `name()`.
- `meta_guid(meta_text)` — pull the GUID out of a `.meta` file.
- `field_i64` / `field_f64` / `field_bool` / `field_str` — tolerant accessors over parsed YAML nodes.
- `UnityFile::parse_lossy(text)` — same split, but **skips** any document body `yaml-rust2` rejects
  instead of failing the file (large scenes occasionally serialize scalars it can't parse; used by the
  world renderer, which only needs Transforms/MeshFilters).
- `parse_meta(text)` — parse a single-document file (e.g. a `.meta`) into its root `Yaml` node.
- `EditableUnityFile::parse(text)` — load for surgical editing. `set_scalar(doc, path, Scalar)` and
  `set_reference(doc, path, fileID, guid, type)` splice a single value; `doc_by_file_id`,
  `documents()`, `into_string()`. Paths are `parse_path("parameters/0/saved")` /
  `parse_path("m_Script/guid")`. Structural edits: `remove_document` / `replace_document_body` /
  `retag_document` / `append_document`, and `append_sequence_item` / `remove_sequence_item` /
  `sequence_items` on block lists (`m_Component`, `m_Children`). See the
  [reference doc](../../docs/reference/unity-yaml-edit.md).
- `script_file_id(namespace, class)` — Unity's `m_Script` fileID for a class inside a DLL (MD4 class
  hash), pinned against the VRChat SDK's serialized assets.

## Usage

```rust
use avatar_unity_yaml::{UnityFile, field_i64, field_str};

let text = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!114 &11400000
MonoBehaviour:
  m_Name: Parameters
  parameters:
  - name: VRCEmote
    valueType: 0
";
let file = UnityFile::parse(text)?;
let doc = &file.documents[0];
assert_eq!(doc.class_id, 114);
assert!(doc.is_monobehaviour());
assert_eq!(doc.name(), Some("Parameters"));

let params = doc.body["parameters"].as_vec().unwrap();
assert_eq!(field_str(&params[0], "name"), Some("VRCEmote"));
assert_eq!(field_i64(&params[0], "valueType"), Some(0));
# anyhow::Ok(())
```

> **Gotcha:** a Unity GUID is 32 hex chars and must contain letters; an all-digit "guid" is parsed
> as a *number*, so `as_str()` returns `None` and guid resolution silently breaks. Use hex-with-letters
> guids in fixtures.

## Status

Built: **M2** (reader) + surgical editor (`EditableUnityFile` / `avatar asset set`).
