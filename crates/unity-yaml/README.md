# avatar-unity-yaml

A reader for Unity's YAML serialization format. Package `avatar-unity-yaml` · library
`avatar_unity_yaml`. Part of the [avatar](../../README.md) monorepo.

## What it does

Reads Unity files (`.asset`, `.prefab`, `.unity`, `.meta`), which are a multi-document YAML stream
where each document is introduced by a header line `--- !u!<classID> &<fileID>` (optionally trailed
by `stripped`). The class id and file id live on that header line; the body below it is ordinary
YAML. This crate splits on the header lines itself to recover the class/file ids, then parses each
body with [`yaml-rust2`](https://crates.io/crates/yaml-rust2).

This is a **reader** — it does not attempt byte-stable round-trip writing (see
[`PLAN.md`](../../PLAN.md) §8); asset generation will be a separate concern.

## Key API

- `UnityFile::parse(text) -> Result<UnityFile>` — split + parse a Unity YAML stream;
  `monobehaviours()` iterates the MonoBehaviour documents.
- `UnityDocument` — one document: class id, file id, and helpers `is_monobehaviour()`,
  `script_guid()`, `name()`.
- `meta_guid(meta_text)` — pull the GUID out of a `.meta` file.
- `field_i64` / `field_f64` / `field_bool` / `field_str` — tolerant accessors over parsed YAML nodes.

> **Gotcha:** a Unity GUID is 32 hex chars and must contain letters; an all-digit "guid" is parsed
> as a *number*, so `as_str()` returns `None` and guid resolution silently breaks. Use hex-with-letters
> guids in fixtures.

## Status

Built: **M2**.
