# Surgical Unity-YAML editing — `EditableUnityFile` / `avatar asset set`

The *modify* counterpart to the read-only `lint`/`stats`/`describe` surface. It edits a value inside
an **existing** Unity YAML asset (`.asset`, `.controller`, `.prefab`, `.anim`, `.unity`) — and makes
the document-level **structural** edits a prefab rewrite needs (remove/replace/append whole
documents, add/remove block-sequence items) — while preserving everything it doesn't touch —
byte-for-byte.

Library: [`avatar_unity_yaml::edit`](../../crates/unity-yaml/src/edit.rs)
(`EditableUnityFile`, `Scalar`, `Seg`, `parse_path`). CLI: `avatar asset set`.

## Why span-splicing, not parse-and-re-emit

A Unity asset is a multi-document YAML stream where each document carries an anchor —
`--- !u!114 &11400000`. Every cross-asset reference (`{fileID: 11400000, guid: …}`) points at one of
those anchors. The reader (`UnityFile`) parses each body with `yaml-rust2`, which **discards**
formatting, key order, and the non-standard `--- !u!… &…` headers. Re-emitting from that parsed
model would rewrite the whole file: reordered keys, dropped/renumbered anchors, a churned diff — and
any of those silently breaks references pointing into the file. `yaml-rust2` 0.11 also exposes no
source spans, so you can't recover where a node lived in the original text.

`EditableUnityFile` keeps the file as **raw text** and edits by **span-splicing**:

1. A small indentation-aware scanner locates the exact byte range of the value to change.
2. That range — and only that range — is replaced.
3. The result is re-parsed with `UnityFile::parse`; a splice that produced malformed YAML fails
   loudly (and legibly, for an agent) instead of writing a broken asset.

Everything else — every `&fileID`, every `{fileID, guid}` reference, indentation, key order, the
`%YAML`/`%TAG` preamble, trailing whitespace, even CRLF line endings — survives untouched because it
is never rewritten. The round-trip tests assert exactly this against the real fixture corpus
(`crates/unity-yaml/tests/round_trip.rs`): a no-op load→emit is the identity, and a single-value edit
changes exactly one line and leaves the anchor set unchanged.

## Paths

A path is `/`-separated and addresses a value within one document's body:

- a **key** descends into a mapping (`m_Name`, `m_Script`);
- a **numeric** segment indexes a sequence (`parameters/0`); Unity mapping keys are never bare
  integers, so the heuristic is unambiguous;
- a final segment may name a **subfield of an inline reference** (`m_Script/guid`,
  `m_Script/fileID`).

Examples: `m_Name` · `parameters/2/saved` · `m_Script/guid` · `m_ChildStates/0/m_State`.

Unity writes a key's sequence value at the key's *own* indent (the `-` is not indented an extra
level), and a sequence element's first field inline on the `- ` line (`- m_Name: GestureLeft`); the
scanner handles both, plus flow-map (`- {fileID: N}`) sequence elements.

## Library API

```rust
use avatar_unity_yaml::{EditableUnityFile, Scalar, parse_path};

let mut file = EditableUnityFile::parse(&text)?;
let doc = file.doc_by_file_id(110200002).unwrap();   // select a document by its &fileID anchor

// Set a scalar (int / float / bool / string). Float/bool render Unity-style (0 not 0.0; 1/0 bools).
file.set_scalar(doc, &parse_path("m_WriteDefaultValues"), Scalar::Int(1))?;

// Re-point a reference at a clip in another asset (the canonical "swap an animation" edit).
file.set_reference(doc, &parse_path("m_Motion"), 7400000, Some("…32 hex…"), 2)?;

let edited = file.into_string();
```

`set_scalar` also reaches subfields inside an inline reference (`m_Script/guid`), since the scanner
descends into flow maps. `set_reference` replaces the whole `{…}` (and can add a `guid` a local
reference lacked). Errors are structured `anyhow` messages ("mapping key 'x' not found", "sequence
index N out of range", "path resolves to a reference; use set_reference") — never panics.

## CLI — `avatar asset set`

```sh
# Rename (single-doc file → --doc optional). Default output is stdout: a pure preview.
avatar asset set Parameters.asset --path m_Name --value Hands

# Edit a nested sequence field; write in place (guarded: refuses to clobber without --force).
avatar asset set Parameters.asset --path parameters/2/saved --value 1 -o Parameters.asset --force

# Multi-document file: pick the document by its fileID anchor.
avatar asset set Hands.controller --doc 110600000 --path m_BlendParameter --value GestureLeftWeight

# Re-target a reference (cross-asset). Omit --ref-guid/--ref-type for a local {fileID: N}.
avatar asset set Hands.controller --doc 110200002 --path m_Motion \
  --ref 7400000 --ref-guid 1234567890abcdef1234567890abcdef --ref-type 2

# Machine-readable report (edit summary + the edited asset text) for an agent host.
avatar asset set Parameters.asset --path m_Name --value Hands --json
```

`--value` type is inferred (int → float → bool → string); force it with `--type int|float|bool|string`.
Writes go through the shared `WriteGuard`: stdout by default, `-o <file>` to write (no clobber
without `--force`), `--dry-run` to report without touching the filesystem. Mutation stays on the CLI
behind the guard — it is deliberately *not* exposed over the read-only MCP server.

## Scope

In scope: editing the **value** at a path — scalars, reference re-targets, flow-map subfields — and
the **document-level structural** edits a prefab rewrite needs, still span-based and still leaving
every untouched byte alone:

| Method | What it does |
|---|---|
| `remove_document(doc)` | Drop a whole `--- !u!… &id` document (header + body). References to it elsewhere are left as-is (Unity reads a dangling local ref as null), so pair it with `remove_sequence_item` on the owner's list. |
| `replace_document_body(doc, body)` | Swap the body while keeping the header — the object's fileID and every reference to it stay valid. This is how a component is **retyped** at the same slot (`DynamicBone` → `VRCPhysBone`, SDK2 descriptor → SDK3). |
| `retag_document(doc, class_id, file_id)` | Rewrite the header (for a class change, e.g. `CapsuleCollider` 136 → MonoBehaviour 114). |
| `append_document(class_id, file_id, body)` | Add a new document at the end (fileID must be unused). |
| `append_sequence_item(doc, path, item)` / `remove_sequence_item(doc, path, i)` | Add to / remove from a **block sequence** such as `m_Component` or `m_Children` (converting `[]` ↔ block form; multi-line items re-indented under their `- `). `sequence_items` / `sequence_len` read them. |

Still out of scope: adding or removing *mapping keys* inside a body — a body that needs a different
key set is regenerated whole (the generators in [`avatar-anim-gen`](anim-gen.md), the component
emitters in [`avatar-migrate`](migrate.md)) and swapped in with `replace_document_body`. A value edit
can only change a value that already exists.
