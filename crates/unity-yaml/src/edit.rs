//! Surgical, round-trip-safe editing of Unity YAML assets.
//!
//! [`UnityFile`](crate::UnityFile) is a *reader*: it parses each document body with `yaml-rust2`,
//! which discards formatting, key order, and the non-standard `--- !u!<class> &<fileID>` headers —
//! so re-emitting from the parsed model would rewrite the whole file and churn every byte. That is
//! exactly what you must **not** do to a Unity asset: a rewrite that reorders keys or drops a
//! `&fileID` anchor silently breaks every cross-asset reference (`{fileID: N, guid: G}`) pointing at
//! it, and produces a noisy, unreviewable diff.
//!
//! [`EditableUnityFile`] instead holds the file as raw text and edits it by **span-splicing**: it
//! locates the exact byte range of the value a caller wants to change and replaces *only* that
//! range. Everything else — fileIDs, GUID references, indentation, key order, the `%YAML`/`%TAG`
//! preamble, trailing whitespace, line endings — is preserved byte-for-byte, because it is never
//! touched. After every edit the result is re-parsed with [`UnityFile::parse`] so a splice that
//! produced malformed YAML fails loudly (and legibly, for an agent) rather than writing a broken
//! asset.
//!
//! `yaml-rust2` 0.11 exposes no source spans on parsed nodes, so the locator is a small
//! indentation-aware scanner over the raw text rather than a reuse of the parser. Unity's
//! serialization is regular enough (two-space indents, `key: value` lines, inline *flow* maps for
//! references, `- ` sequence members at the parent indent) that this stays compact.
//!
//! # Scope
//!
//! Supported edits: replacing a **scalar** value at a path (including a subfield *inside* a flow
//! map, e.g. a reference's `guid`/`fileID`), and replacing a whole **reference** (`{fileID, …}`).
//! Paths descend through nested mappings (`Key`) and sequences (`Index`).
//!
//! On top of the value edits sit a small set of **structural** edits, still span-based and still
//! leaving every untouched byte alone: removing a whole document ([`remove_document`]),
//! swapping a document's body for new text while keeping its `&fileID` header — and therefore
//! every reference to it — intact ([`replace_document_body`]), appending a new document
//! ([`append_document`]), and appending to / removing from a **block sequence** such as a
//! `GameObject`'s `m_Component` list or a `Transform`'s `m_Children`
//! ([`append_sequence_item`], [`remove_sequence_item`]). These are what a prefab-level rewrite
//! (strip a subtree, swap one component type for another, bolt on a new component) needs; adding
//! or removing *mapping keys* is still out of scope — a body that needs a different key set is
//! regenerated whole and swapped in with `replace_document_body`.
//!
//! [`remove_document`]: EditableUnityFile::remove_document
//! [`replace_document_body`]: EditableUnityFile::replace_document_body
//! [`append_document`]: EditableUnityFile::append_document
//! [`append_sequence_item`]: EditableUnityFile::append_sequence_item
//! [`remove_sequence_item`]: EditableUnityFile::remove_sequence_item
//!
//! ```
//! use avatar_unity_yaml::{EditableUnityFile, Scalar, parse_path};
//!
//! let text = "\
//! %YAML 1.1
//! %TAG !u! tag:unity3d.com,2011:
//! --- !u!114 &11400000
//! MonoBehaviour:
//!   m_Script: {fileID: 11500000, guid: abcdef0123456789abcdef0123456789, type: 3}
//!   m_Name: Parameters
//! ";
//! let mut file = EditableUnityFile::parse(text)?;
//! let doc = file.doc_by_file_id(11400000).unwrap();
//! file.set_scalar(doc, &parse_path("m_Name"), Scalar::Str("Params2"))?;
//! // The m_Script line — fileID, guid and all — is untouched, byte-for-byte.
//! assert!(file.text().contains("m_Name: Params2"));
//! assert!(file.text().contains("guid: abcdef0123456789abcdef0123456789"));
//! # anyhow::Ok(())
//! ```

use std::ops::Range;

use anyhow::{Context, Result, bail};

use crate::{UnityFile, parse_header};

/// A scalar value to write, rendered the way Unity serializes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar<'a> {
    /// An integer (`m_Type: 2`).
    Int(i64),
    /// A float. Integral values print bare (`0`, not `0.0`), matching Unity.
    Float(f64),
    /// A boolean. Unity serializes bools as `1`/`0`, so that is what is written.
    Bool(bool),
    /// A string, written unquoted (the common case for `m_Name`, GUIDs, hierarchy paths). Must not
    /// contain a newline.
    Str(&'a str),
}

impl Scalar<'_> {
    /// Render to the exact token Unity would write.
    fn render(&self) -> Result<String> {
        Ok(match self {
            Scalar::Int(i) => i.to_string(),
            Scalar::Float(f) => fmt_unity_f64(*f),
            Scalar::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            Scalar::Str(s) => {
                if s.contains('\n') {
                    bail!("string value contains a newline; a scalar must be a single line");
                }
                (*s).to_string()
            }
        })
    }
}

/// Render an `f64` the way Unity renders floats: an integral value as a bare integer (`0`, `-2`),
/// otherwise the shortest decimal that round-trips. Mirrors `avatar_anim_gen::yaml_emit::fmt_f32`
/// (kept here so this crate stays dependency-free of the generator).
fn fmt_unity_f64(v: f64) -> String {
    if v == v.trunc() && v.is_finite() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// One segment of an edit path: a mapping key or a sequence index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    /// Descend into a mapping by key (`m_Script`, `parameters`).
    Key(String),
    /// Descend into a sequence by zero-based index.
    Index(usize),
}

/// Parse a `/`-separated path into [`Seg`]s. A purely numeric segment is an [`Index`](Seg::Index);
/// anything else is a [`Key`](Seg::Key). Empty segments (leading/trailing/double `/`) are dropped.
///
/// Unity mapping keys are never bare integers, so the numeric-is-index heuristic is unambiguous in
/// practice: `parameters/0/defaultValue` selects field `defaultValue` of the first `parameters`
/// element; `m_Script/guid` selects the `guid` inside the `m_Script` reference.
pub fn parse_path(s: &str) -> Vec<Seg> {
    s.split('/')
        .filter(|p| !p.is_empty())
        .map(|p| match p.parse::<usize>() {
            Ok(n) => Seg::Index(n),
            Err(_) => Seg::Key(p.to_string()),
        })
        .collect()
}

/// A located document within the file: its identity plus the byte ranges of its header line and
/// body. The byte ranges are private (they shift on every edit); identity is public.
#[derive(Debug, Clone)]
pub struct DocSpan {
    /// Unity class id from the `!u!<classID>` tag.
    pub class_id: u32,
    /// The `&<fileID>` anchor identifying this object within the file.
    pub file_id: i64,
    /// `true` if the header was marked `stripped`.
    pub stripped: bool,
    /// The top-level type key of the body (`MonoBehaviour`, `Transform`, …).
    pub type_name: String,
    /// The `--- !u!… &…` header line, including its trailing newline.
    header: Range<usize>,
    /// The document body: everything after the header line up to the next header (or EOF).
    body: Range<usize>,
}

impl DocSpan {
    /// The byte range of this document's header line in the file text.
    pub fn header_range(&self) -> Range<usize> {
        self.header.clone()
    }
    /// The byte range of this document's body in the file text.
    pub fn body_range(&self) -> Range<usize> {
        self.body.clone()
    }
}

/// A Unity YAML stream held as raw text for surgical, round-trip-safe edits. See the module docs.
#[derive(Debug, Clone)]
pub struct EditableUnityFile {
    text: String,
    docs: Vec<DocSpan>,
}

/// One physical line within a body, with byte offsets into the file text. `content_start` is the
/// offset of the first non-space byte; `end` excludes the trailing newline (and a `\r` before it).
#[derive(Debug, Clone, Copy)]
struct Line {
    content_start: usize,
    end: usize,
    indent: usize,
}

impl Line {
    fn nonblank(&self) -> bool {
        self.content_start < self.end
    }
}

/// A located block sequence: the key's (empty or `[]`) value range, the indent its `- ` elements
/// sit at, each element's byte range (from its `- ` line start through its last continuation
/// line's newline), and the offset just past the block.
struct SeqLoc {
    key_value: Range<usize>,
    base: usize,
    elems: Vec<Range<usize>>,
    block_end: usize,
}

impl EditableUnityFile {
    /// Parse a Unity YAML stream for editing. Fails if the text is not a parseable Unity file (so a
    /// caller never edits something it has misread); the strict parse mirrors [`UnityFile::parse`].
    pub fn parse(text: &str) -> Result<Self> {
        // Validate up front: an editor that can't read the file shouldn't pretend to edit it.
        UnityFile::parse(text).context("parsing Unity YAML for editing")?;
        let docs = Self::compute_docs(text);
        Ok(EditableUnityFile {
            text: text.to_string(),
            docs,
        })
    }

    /// The located documents, in file order.
    pub fn documents(&self) -> &[DocSpan] {
        &self.docs
    }

    /// The index of the document with the given `&fileID`, if present.
    pub fn doc_by_file_id(&self, file_id: i64) -> Option<usize> {
        self.docs.iter().position(|d| d.file_id == file_id)
    }

    /// The current file text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Consume the editor, returning the edited text.
    pub fn into_string(self) -> String {
        self.text
    }

    /// Set the scalar value at `path` within document `doc` to `value`.
    ///
    /// `path` may end at a plain scalar (`m_Name`), a sequence-element field (`parameters/0/saved`),
    /// or a subfield inside an inline reference (`m_Script/guid`, `m_Script/fileID`). It is an error
    /// if the path resolves to a whole reference (`{…}`) or a nested block — use [`set_reference`]
    /// for the former, and a deeper path for the latter.
    ///
    /// [`set_reference`]: Self::set_reference
    pub fn set_scalar(&mut self, doc: usize, path: &[Seg], value: Scalar) -> Result<()> {
        let body = self.doc_body(doc)?;
        let lines = self.body_lines(&body);
        let vr = self.resolve_path(&lines, path)?;
        if self.text[vr.clone()].starts_with('{') {
            bail!(
                "path resolves to a reference ({{...}}); use set_reference to rewrite it, or append \
                 a subfield key (e.g. '.../guid')"
            );
        }
        let rendered = value.render()?;
        self.replace(vr, &rendered)
            .context("scalar edit produced malformed Unity YAML")
    }

    /// Replace the whole reference at `path` (an inline `{fileID: …}` or `{fileID: …, guid: …,
    /// type: …}` map) with a new one. `guid: None` writes a local reference (`{fileID: N}`); `Some`
    /// writes a cross-asset reference. This is the way to *re-target* a reference — re-point a
    /// `m_Script`, a mesh, a material, or an animation clip — and to add a `guid` a local reference
    /// did not have.
    pub fn set_reference(
        &mut self,
        doc: usize,
        path: &[Seg],
        file_id: i64,
        guid: Option<&str>,
        asset_type: i64,
    ) -> Result<()> {
        let body = self.doc_body(doc)?;
        let lines = self.body_lines(&body);
        let vr = self.resolve_path(&lines, path)?;
        if !self.text[vr.clone()].starts_with('{') {
            bail!(
                "target at path is not an inline reference ({{...}}); use set_scalar for scalars"
            );
        }
        let rendered = match guid {
            Some(g) => format!("{{fileID: {file_id}, guid: {g}, type: {asset_type}}}"),
            None => format!("{{fileID: {file_id}}}"),
        };
        self.replace(vr, &rendered)
            .context("reference edit produced malformed Unity YAML")
    }

    // ---- structural edits ---------------------------------------------------

    /// Remove document `doc` entirely (its `--- !u!… &…` header line and body). Nothing else
    /// moves; any *reference* to the removed fileID elsewhere in the file is left as-is (Unity
    /// treats a dangling local reference as `{fileID: 0}`), so callers stripping a component
    /// should also [`remove_sequence_item`](Self::remove_sequence_item) it from its owner's list.
    pub fn remove_document(&mut self, doc: usize) -> Result<()> {
        let d = self
            .docs
            .get(doc)
            .with_context(|| format!("document index {doc} out of range"))?;
        let range = d.header.start..d.body.end;
        self.replace(range, "")
            .context("document removal produced malformed Unity YAML")
    }

    /// Replace the **body** of document `doc` (everything after its header line) with `body`,
    /// keeping the `--- !u!<class> &<fileID>` header — and so the object's identity and every
    /// reference to it — exactly as it was. `body` must start with the top-level `Type:` line and
    /// end with a newline (one is added if missing). This is how a component is swapped for a
    /// different type at the same fileID (e.g. a `DynamicBone` MonoBehaviour re-typed as a
    /// `VRCPhysBone` one: same `&id`, same slot in the owning `GameObject`'s `m_Component`).
    ///
    /// The header's *class id* is not changed here; when the new body's class differs, pass the
    /// new class via [`retag_document`](Self::retag_document).
    pub fn replace_document_body(&mut self, doc: usize, body: &str) -> Result<()> {
        let d = self
            .docs
            .get(doc)
            .with_context(|| format!("document index {doc} out of range"))?;
        let mut new_body = body.to_string();
        if !new_body.ends_with('\n') {
            new_body.push('\n');
        }
        let range = d.body.clone();
        self.replace(range, &new_body)
            .context("document body replacement produced malformed Unity YAML")
    }

    /// Rewrite document `doc`'s header to `--- !u!<class_id> &<file_id>` (keeping a `stripped`
    /// marker if present). Use with [`replace_document_body`](Self::replace_document_body) when a
    /// document changes Unity class (rare — a MonoBehaviour swapped for another MonoBehaviour
    /// keeps class 114 and needs no retag).
    pub fn retag_document(&mut self, doc: usize, class_id: u32, file_id: i64) -> Result<()> {
        let d = self
            .docs
            .get(doc)
            .with_context(|| format!("document index {doc} out of range"))?;
        let stripped = if d.stripped { " stripped" } else { "" };
        let header = format!("--- !u!{class_id} &{file_id}{stripped}\n");
        let range = d.header.clone();
        self.replace(range, &header)
            .context("document retag produced malformed Unity YAML")
    }

    /// Append a new document `--- !u!<class_id> &<file_id>` with `body` (which must start with the
    /// top-level `Type:` line) at the end of the file. Returns the new document's index. Fails if
    /// `file_id` is already used by another document in the file.
    pub fn append_document(&mut self, class_id: u32, file_id: i64, body: &str) -> Result<usize> {
        if self.doc_by_file_id(file_id).is_some() {
            bail!("fileID {file_id} already exists in this file");
        }
        let mut chunk = String::new();
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            chunk.push('\n');
        }
        chunk.push_str(&format!("--- !u!{class_id} &{file_id}\n"));
        chunk.push_str(body);
        if !body.ends_with('\n') {
            chunk.push('\n');
        }
        let at = self.text.len();
        self.replace(at..at, &chunk)
            .context("document append produced malformed Unity YAML")?;
        self.doc_by_file_id(file_id)
            .context("appended document not found after re-parse")
    }

    /// Append one element to the block sequence at `path` in document `doc` (e.g.
    /// `m_Component`, `m_Children`, `colliders`). `item` is the element's text **without** the
    /// leading `- `; a multi-line item's continuation lines are re-indented under the element.
    /// An empty flow sequence (`key: []`) is converted to block form. Elements are written at the
    /// key's own indent, the way Unity serializes them.
    pub fn append_sequence_item(&mut self, doc: usize, path: &[Seg], item: &str) -> Result<()> {
        let loc = self.locate_sequence(doc, path)?;
        let pad = " ".repeat(loc.base);
        let mut rendered = String::new();
        for (i, line) in item.lines().enumerate() {
            if i == 0 {
                rendered.push_str(&format!("{pad}- {line}\n"));
            } else {
                rendered.push_str(&format!("{pad}  {line}\n"));
            }
        }
        if loc.elems.is_empty() {
            // `key: []` -> `key:` + block. Insert the block first (the later offset), then blank
            // the `[]` (and the single space Unity puts before it) at the earlier, still-valid one.
            let insert_at = loc.block_end;
            let needs_nl = insert_at > 0 && self.text.as_bytes()[insert_at - 1] != b'\n';
            let chunk = if needs_nl {
                format!("\n{rendered}")
            } else {
                rendered
            };
            self.text.replace_range(insert_at..insert_at, &chunk);
            let mut vs = loc.key_value.start;
            let ve = loc.key_value.end;
            if vs > 0 && self.text.as_bytes()[vs - 1] == b' ' {
                vs -= 1;
            }
            self.text.replace_range(vs..ve, "");
            self.docs = Self::compute_docs(&self.text);
            return UnityFile::parse(&self.text)
                .map(|_| ())
                .context("sequence append produced malformed Unity YAML");
        }
        let at = loc.block_end;
        let needs_nl = at > 0 && self.text.as_bytes()[at - 1] != b'\n';
        let chunk = if needs_nl {
            format!("\n{rendered}")
        } else {
            rendered
        };
        self.replace(at..at, &chunk)
            .context("sequence append produced malformed Unity YAML")
    }

    /// Remove element `index` from the block sequence at `path` in document `doc`. Removing the
    /// last remaining element leaves `key: []`.
    pub fn remove_sequence_item(&mut self, doc: usize, path: &[Seg], index: usize) -> Result<()> {
        let loc = self.locate_sequence(doc, path)?;
        let Some(range) = loc.elems.get(index).cloned() else {
            bail!(
                "sequence index {index} out of range ({} element(s))",
                loc.elems.len()
            );
        };
        self.text.replace_range(range, "");
        if loc.elems.len() == 1 {
            // Element came after the key line, so the key's value offset is unchanged.
            let vs = loc.key_value.start;
            let lead = if vs > 0 && self.text.as_bytes()[vs - 1] == b':' {
                " []"
            } else {
                "[]"
            };
            self.text.replace_range(vs..vs, lead);
        }
        self.docs = Self::compute_docs(&self.text);
        UnityFile::parse(&self.text)
            .map(|_| ())
            .context("sequence removal produced malformed Unity YAML")
    }

    /// The number of elements in the block (or empty flow) sequence at `path`.
    pub fn sequence_len(&self, doc: usize, path: &[Seg]) -> Result<usize> {
        Ok(self.locate_sequence(doc, path)?.elems.len())
    }

    /// Byte ranges of the sequence's elements are private; this returns each element's text
    /// (the `- ` line and its continuation lines) so callers can find the one to remove.
    pub fn sequence_items(&self, doc: usize, path: &[Seg]) -> Result<Vec<String>> {
        let loc = self.locate_sequence(doc, path)?;
        Ok(loc
            .elems
            .iter()
            .map(|r| self.text[r.clone()].to_string())
            .collect())
    }

    /// Locate the block sequence introduced by the mapping key at `path`.
    fn locate_sequence(&self, doc: usize, path: &[Seg]) -> Result<SeqLoc> {
        let body = self.doc_body(doc)?;
        let lines = self.body_lines(&body);
        let key_value = self.resolve_path(&lines, path)?;
        let vtext = &self.text[key_value.clone()];
        if !(vtext.is_empty() || vtext == "[]") {
            bail!("path does not resolve to a block sequence (value is `{vtext}`)");
        }
        // The key line is the one containing the value range.
        let key_idx = lines
            .iter()
            .position(|l| {
                let start = l.content_start - l.indent;
                start <= key_value.start && key_value.start <= l.end
            })
            .context("could not locate the sequence key line")?;
        let key_line = lines[key_idx];
        let key_indent = key_line.indent;
        let after_key = key_line.end + 1; // past the newline (or == body.end)
        if vtext == "[]" {
            return Ok(SeqLoc {
                key_value,
                base: key_indent,
                elems: Vec::new(),
                block_end: after_key.min(body.end),
            });
        }
        // First non-blank line after the key decides the element indent (Unity: the key's own).
        let Some(first) = ((key_idx + 1)..lines.len()).find(|&j| lines[j].nonblank()) else {
            bail!("sequence key has no following lines");
        };
        let fl = lines[first];
        if fl.indent < key_indent || !self.text[fl.content_start..fl.end].starts_with('-') {
            bail!("sequence key is not followed by `- ` elements");
        }
        let base = fl.indent;
        let mut elems: Vec<Range<usize>> = Vec::new();
        let mut cur_start: Option<usize> = None;
        let mut block_end = fl.content_start - fl.indent;
        for lj in lines.iter().skip(first) {
            let line_start = lj.content_start - lj.indent;
            if !lj.nonblank() {
                continue;
            }
            let is_dash = self.text[lj.content_start..lj.end].starts_with('-');
            if lj.indent == base && is_dash {
                if let Some(s) = cur_start {
                    elems.push(s..line_start);
                }
                cur_start = Some(line_start);
                block_end = (lj.end + 1).min(body.end);
                continue;
            }
            if lj.indent > base {
                block_end = (lj.end + 1).min(body.end);
                continue;
            }
            // Same-or-shallower non-dash line: block over.
            break;
        }
        if let Some(s) = cur_start {
            elems.push(s..block_end);
        }
        Ok(SeqLoc {
            key_value,
            base,
            elems,
            block_end,
        })
    }

    // ---- internals ---------------------------------------------------------

    fn doc_body(&self, doc: usize) -> Result<Range<usize>> {
        self.docs.get(doc).map(|d| d.body.clone()).with_context(|| {
            format!(
                "document index {doc} out of range ({} docs)",
                self.docs.len()
            )
        })
    }

    /// Replace `range` with `with`, then re-derive document spans and re-validate the result.
    fn replace(&mut self, range: Range<usize>, with: &str) -> Result<()> {
        self.text.replace_range(range, with);
        self.docs = Self::compute_docs(&self.text);
        UnityFile::parse(&self.text).map(|_| ())
    }

    /// Compute the document spans (identity + header/body byte ranges) for a Unity stream.
    fn compute_docs(text: &str) -> Vec<DocSpan> {
        let len = text.len();
        // Byte offset of the start of each physical line.
        let mut line_starts = vec![0usize];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        // A trailing '\n' yields a phantom empty line at `len`; drop it.
        if line_starts.last() == Some(&len) && len > 0 {
            line_starts.pop();
        }

        let header_lines: Vec<usize> = line_starts
            .iter()
            .copied()
            .filter(|&s| text[s..].starts_with("---"))
            .collect();

        let mut docs = Vec::new();
        for (h, &header_start) in header_lines.iter().enumerate() {
            // Header line runs to the start of the next line (or EOF).
            let next_line = line_starts
                .iter()
                .copied()
                .find(|&s| s > header_start)
                .unwrap_or(len);
            let header_end = next_line;
            let body_start = header_end;
            let body_end = header_lines.get(h + 1).copied().unwrap_or(len);

            let Some((class_id, file_id, stripped)) = parse_header(&text[header_start..header_end])
            else {
                continue;
            };
            let type_name = Self::first_body_key(text, body_start..body_end);
            docs.push(DocSpan {
                class_id,
                file_id,
                stripped,
                type_name,
                header: header_start..header_end,
                body: body_start..body_end,
            });
        }
        docs
    }

    /// The top-level type key of a body region (the text before the first `:` on the first
    /// non-blank line), or empty if there is none.
    fn first_body_key(text: &str, body: Range<usize>) -> String {
        for line in text[body].lines() {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            return t.split(':').next().unwrap_or("").to_string();
        }
        String::new()
    }

    /// Build the line model for a body region.
    fn body_lines(&self, body: &Range<usize>) -> Vec<Line> {
        let text = &self.text;
        let mut out = Vec::new();
        let mut pos = body.start;
        while pos < body.end {
            let rel = text[pos..body.end].find('\n');
            let raw_end = rel.map(|i| pos + i).unwrap_or(body.end);
            let mut end = raw_end;
            if end > pos && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
            let line = &text[pos..end];
            let indent = line.len() - line.trim_start_matches(' ').len();
            out.push(Line {
                content_start: pos + indent,
                end,
                indent,
            });
            if rel.is_none() {
                break;
            }
            pos = raw_end + 1;
        }
        out
    }

    /// Resolve `path` to the byte range of the target value, starting from the fields of the body's
    /// single top-level `Type:` mapping.
    fn resolve_path(&self, lines: &[Line], path: &[Seg]) -> Result<Range<usize>> {
        if path.is_empty() {
            bail!("empty edit path");
        }
        let first = lines
            .iter()
            .position(|l| l.nonblank())
            .context("document body is empty")?;
        let type_indent = lines[first].indent;
        let lo = first + 1;
        let hi = lines.len();
        let base = lines[lo..hi]
            .iter()
            .find(|l| l.nonblank())
            .map(|l| l.indent)
            .filter(|&i| i > type_indent)
            .context("document body has no nested fields to edit")?;
        self.resolve_in(lines, lo, hi, base, path)
    }

    /// Resolve `path` within the block `lines[lo..hi]` whose direct members sit at indent `base`.
    fn resolve_in(
        &self,
        lines: &[Line],
        lo: usize,
        hi: usize,
        base: usize,
        path: &[Seg],
    ) -> Result<Range<usize>> {
        match &path[0] {
            Seg::Key(k) => {
                for i in lo..hi {
                    let l = lines[i];
                    if !l.nonblank() || l.indent != base {
                        continue;
                    }
                    // Unity writes a key's sequence value at the key's own indent, so `-` lines at
                    // `base` belong to a *previous* key's list — skip them and keep scanning for the
                    // next mapping key (e.g. `m_DefaultState` after an `m_ChildStates` list).
                    let content = &self.text[l.content_start..l.end];
                    if content.starts_with('-') {
                        continue;
                    }
                    if let Some((key, vr)) = self.parse_kv(l.content_start, l.end)
                        && key == k
                    {
                        return self.finish(vr, base, &path[1..], lines, i + 1, hi);
                    }
                }
                bail!("mapping key '{k}' not found");
            }
            Seg::Index(n) => {
                let mut count = 0;
                for i in lo..hi {
                    let l = lines[i];
                    if !l.nonblank() || l.indent != base {
                        continue;
                    }
                    let content = &self.text[l.content_start..l.end];
                    if !content.starts_with('-') {
                        // A non-`-` member at this indent ends the sequence (the block bound should
                        // already exclude it, but stop defensively rather than mis-count).
                        break;
                    }
                    if count == *n {
                        return self.resolve_seq_element(lines, i, hi, base, &path[1..]);
                    }
                    count += 1;
                }
                bail!("sequence index {n} out of range ({count} element(s) at this level)");
            }
        }
    }

    /// Resolve the remaining `rest` of a path within the sequence element that begins on line
    /// `elem` (a `- …` line at indent `base`).
    fn resolve_seq_element(
        &self,
        lines: &[Line],
        elem: usize,
        hi: usize,
        base: usize,
        rest: &[Seg],
    ) -> Result<Range<usize>> {
        let l = lines[elem];
        let content = &self.text[l.content_start..l.end];
        let after = if content.starts_with("- ") {
            2
        } else if content.starts_with('-') {
            1
        } else {
            0
        };
        let inline_start = l.content_start + after;
        // The element owns the following lines that are indented deeper than `base`.
        let mut child_hi = hi;
        for (j, lj) in lines.iter().enumerate().take(hi).skip(elem + 1) {
            if lj.nonblank() && lj.indent <= base {
                child_hi = j;
                break;
            }
        }

        let inline = &self.text[inline_start..l.end];

        // A flow-map element (`- {fileID: 110100000}`): the whole `{…}` is the element value. This
        // is how reference lists (transitions, child motions) are written.
        if inline.starts_with('{') {
            let vr = self.trim_value(inline_start, l.end);
            if rest.is_empty() {
                return Ok(vr);
            }
            return self.resolve_flow(vr, rest);
        }

        if rest.is_empty() {
            // Scalar element (`- VRCEmote`): the value is the inline text. A `key: value` element
            // with no field selector is ambiguous, so require one.
            if inline.contains(':') {
                bail!("sequence element is a mapping; append a field key to the path");
            }
            return Ok(self.trim_value(inline_start, l.end));
        }

        let field_indent = l.indent + 2; // column of the element's fields
        match &rest[0] {
            Seg::Key(k) => {
                // The first field is inline on the `- ` line (`- name: VRCEmote`).
                if let Some((ikey, ivr)) = self.parse_kv(inline_start, l.end)
                    && ikey == k
                {
                    return self.finish(ivr, field_indent, &rest[1..], lines, elem + 1, child_hi);
                }
                // Remaining fields are on following lines at `field_indent`.
                for j in (elem + 1)..child_hi {
                    let cl = lines[j];
                    if !cl.nonblank() || cl.indent != field_indent {
                        continue;
                    }
                    let cc = &self.text[cl.content_start..cl.end];
                    if cc.starts_with('-') {
                        continue;
                    }
                    if let Some((key, vr)) = self.parse_kv(cl.content_start, cl.end)
                        && key == k
                    {
                        return self.finish(vr, field_indent, &rest[1..], lines, j + 1, child_hi);
                    }
                }
                bail!("field '{k}' not found in sequence element");
            }
            Seg::Index(_) => {
                bail!("nested sequence indexing inside a sequence element is not supported")
            }
        }
    }

    /// Given a member's value range and remaining path, either return the value (path ends here),
    /// descend into an inline flow map, or descend into the member's nested block.
    fn finish(
        &self,
        value: Range<usize>,
        member_indent: usize,
        rest: &[Seg],
        lines: &[Line],
        child_lo: usize,
        child_hi: usize,
    ) -> Result<Range<usize>> {
        if rest.is_empty() {
            return Ok(value);
        }
        let valtext = &self.text[value.clone()];
        if valtext.starts_with('{') {
            return self.resolve_flow(value, rest);
        }
        if !valtext.is_empty() {
            bail!("path continues past a scalar value");
        }
        // Empty value: descend into the member's nested block. Two shapes are possible:
        //   - a nested *mapping*, whose keys are indented deeper than this member; or
        //   - a nested *sequence*, whose `- ` members Unity writes at this member's *own* indent.
        let Some(start) = (child_lo..child_hi).find(|&j| lines[j].nonblank()) else {
            bail!("no nested block to descend into");
        };
        let first = lines[start];
        let is_seq = first.indent == member_indent
            && self.text[first.content_start..first.end].starts_with('-');
        if !is_seq && first.indent <= member_indent {
            bail!("no nested block to descend into");
        }
        let base = first.indent;
        // The block ends at the first line that is neither deeper than the member nor a same-indent
        // sequence member of it.
        let mut endw = child_hi;
        for (j, lj) in lines.iter().enumerate().take(child_hi).skip(start) {
            if !lj.nonblank() {
                continue;
            }
            if lj.indent < member_indent {
                endw = j;
                break;
            }
            if lj.indent == member_indent && !self.text[lj.content_start..lj.end].starts_with('-') {
                endw = j;
                break;
            }
        }
        self.resolve_in(lines, start, endw, base, rest)
    }

    /// Resolve a single subfield key inside an inline flow map (`{fileID: N, guid: G, type: T}`).
    /// Unity references are flat (no nested braces), so a comma split is sufficient.
    fn resolve_flow(&self, value: Range<usize>, rest: &[Seg]) -> Result<Range<usize>> {
        if rest.len() != 1 {
            bail!("descent into a flow map supports exactly one subfield key");
        }
        let Seg::Key(k) = &rest[0] else {
            bail!("a flow-map subfield must be a key, not an index");
        };
        let s = &self.text[value.clone()];
        let open = s.find('{').context("value is not a flow map")?;
        let close = s.rfind('}').context("flow map is not closed")?;
        let inner_start = value.start + open + 1;
        let inner = &self.text[inner_start..value.start + close];

        let mut off = inner_start;
        for part in inner.split(',') {
            let plen = part.len();
            if let Some(colon) = part.find(':') {
                let key = part[..colon].trim();
                if key == k {
                    let mut vs = off + colon + 1;
                    if self.text.as_bytes().get(vs) == Some(&b' ') {
                        vs += 1;
                    }
                    return Ok(self.trim_value(vs, off + plen));
                }
            }
            off += plen + 1; // +1 for the comma consumed by split
        }
        bail!("subfield '{k}' not found in flow map");
    }

    /// Parse a `key: value` line into `(key, value_range)`. The value range may be empty (a `key:`
    /// that introduces a nested block). Trailing spaces are excluded from the value range.
    fn parse_kv(&self, content_start: usize, end: usize) -> Option<(&str, Range<usize>)> {
        let content = &self.text[content_start..end];
        let colon = content.find(':')?;
        let key = &content[..colon];
        let mut vs = content_start + colon + 1;
        if self.text.as_bytes().get(vs) == Some(&b' ') {
            vs += 1;
        }
        Some((key, self.trim_value(vs, end)))
    }

    /// `start..end` with trailing spaces trimmed (line ends already exclude `\n`/`\r`).
    fn trim_value(&self, start: usize, end: usize) -> Range<usize> {
        let mut e = end;
        while e > start && self.text.as_bytes()[e - 1] == b' ' {
            e -= 1;
        }
        start..e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: &str = "\
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
  - name: Toggle1
    valueType: 2
    saved: 1
    defaultValue: 0
";

    // Two documents: a controller with an inline reference list and nested sequence elements.
    const CTRL: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1102 &110200002
AnimatorState:
  m_Name: Fist
  m_WriteDefaultValues: 0
  m_Motion: {fileID: 110600000}
  m_Transitions:
  - {fileID: 110100000}
--- !u!1107 &110700000
AnimatorStateMachine:
  m_ChildStates:
  - serializedVersion: 1
    m_State: {fileID: 110200001}
  m_DefaultState: {fileID: 0}
";

    fn edited(src: &str, f: impl FnOnce(&mut EditableUnityFile, usize)) -> String {
        let mut file = EditableUnityFile::parse(src).expect("parse");
        let doc = 0;
        f(&mut file, doc);
        file.into_string()
    }

    #[test]
    fn set_top_level_scalar_string_leaves_everything_else_byte_for_byte() {
        let out = edited(PARAMS, |f, d| {
            f.set_scalar(d, &parse_path("m_Name"), Scalar::Str("Renamed"))
                .unwrap();
        });
        assert!(out.contains("m_Name: Renamed\n"));
        // The reference, its guid, and every other line are untouched.
        assert!(out.contains(
            "m_Script: {fileID: 11500000, guid: abcdef0123456789abcdef0123456789, type: 3}"
        ));
        // Exactly one line differs from the original.
        let diffs = PARAMS
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(diffs, 1, "only m_Name should change");
        assert_eq!(PARAMS.lines().count(), out.lines().count());
    }

    #[test]
    fn set_int_float_bool_scalars() {
        let out = edited(PARAMS, |f, d| {
            f.set_scalar(d, &parse_path("m_ObjectHideFlags"), Scalar::Int(1))
                .unwrap();
        });
        assert!(out.contains("m_ObjectHideFlags: 1\n"));

        // Float renders Unity-style: integral → bare, fractional → shortest.
        assert_eq!(Scalar::Float(0.0).render().unwrap(), "0");
        assert_eq!(Scalar::Float(0.25).render().unwrap(), "0.25");
        assert_eq!(Scalar::Bool(true).render().unwrap(), "1");
        assert_eq!(Scalar::Bool(false).render().unwrap(), "0");
    }

    #[test]
    fn edit_sequence_element_field_by_index() {
        // parameters/1/saved is the `saved:` of the SECOND element (Toggle1), not the first.
        let out = edited(PARAMS, |f, d| {
            f.set_scalar(d, &parse_path("parameters/1/saved"), Scalar::Int(0))
                .unwrap();
        });
        // First element's saved stays 1; second flips to 0.
        let toggle = out.split("- name: Toggle1").nth(1).unwrap();
        assert!(toggle.contains("saved: 0"));
        assert!(
            out.split("- name: VRCEmote")
                .nth(1)
                .unwrap()
                .contains("saved: 1")
        );
    }

    #[test]
    fn edit_inline_first_field_of_sequence_element() {
        // `name` is inline on the `- ` line itself.
        let out = edited(PARAMS, |f, d| {
            f.set_scalar(d, &parse_path("parameters/0/name"), Scalar::Str("Renamed"))
                .unwrap();
        });
        assert!(out.contains("- name: Renamed\n"));
        assert!(out.contains("- name: Toggle1\n"));
    }

    #[test]
    fn edit_flow_map_subfield_guid_and_fileid() {
        let out = edited(PARAMS, |f, d| {
            f.set_scalar(
                d,
                &parse_path("m_Script/guid"),
                Scalar::Str("00000000000000000000000000000000"),
            )
            .unwrap();
            f.set_scalar(d, &parse_path("m_Script/fileID"), Scalar::Int(99))
                .unwrap();
        });
        assert!(
            out.contains("m_Script: {fileID: 99, guid: 00000000000000000000000000000000, type: 3}")
        );
    }

    #[test]
    fn retarget_whole_reference() {
        // Re-point a local m_Motion at a clip in another asset.
        let out = edited(CTRL, |f, d| {
            f.set_reference(
                d,
                &parse_path("m_Motion"),
                7400000,
                Some("11111111111111111111111111111111"),
                2,
            )
            .unwrap();
        });
        assert!(out.contains(
            "m_Motion: {fileID: 7400000, guid: 11111111111111111111111111111111, type: 2}"
        ));
    }

    #[test]
    fn edit_flow_map_sequence_element() {
        // m_Transitions/0 is a bare `- {fileID: N}` reference element.
        let out = edited(CTRL, |f, d| {
            f.set_scalar(d, &parse_path("m_Transitions/0/fileID"), Scalar::Int(42))
                .unwrap();
        });
        assert!(out.contains("- {fileID: 42}\n"));
    }

    #[test]
    fn edit_nested_sequence_element_reference() {
        // m_ChildStates/0/m_State is a reference on a following line of the element (not inline).
        let file = EditableUnityFile::parse(CTRL).unwrap();
        let sm = file.doc_by_file_id(110700000).unwrap();
        let mut file = file;
        file.set_reference(sm, &parse_path("m_ChildStates/0/m_State"), 555, None, 0)
            .unwrap();
        assert!(file.text().contains("m_State: {fileID: 555}\n"));
    }

    #[test]
    fn doc_selection_by_file_id() {
        let file = EditableUnityFile::parse(CTRL).unwrap();
        assert_eq!(file.documents().len(), 2);
        assert_eq!(file.doc_by_file_id(110200002), Some(0));
        assert_eq!(file.doc_by_file_id(110700000), Some(1));
        assert_eq!(file.doc_by_file_id(999), None);
        assert_eq!(file.documents()[0].type_name, "AnimatorState");
    }

    #[test]
    fn errors_are_legible_not_panics() {
        let mut file = EditableUnityFile::parse(PARAMS).unwrap();
        // Missing key.
        let e = file
            .set_scalar(0, &parse_path("m_Nope"), Scalar::Int(1))
            .unwrap_err();
        assert!(e.to_string().contains("m_Nope"), "got: {e}");
        // Index out of range.
        let e = file
            .set_scalar(0, &parse_path("parameters/9/saved"), Scalar::Int(1))
            .unwrap_err();
        assert!(e.to_string().contains("out of range"), "got: {e}");
        // Scalar op on a reference.
        let e = file
            .set_scalar(0, &parse_path("m_Script"), Scalar::Int(1))
            .unwrap_err();
        assert!(e.to_string().contains("reference"), "got: {e}");
        // Doc index out of range.
        let e = file
            .set_scalar(7, &parse_path("m_Name"), Scalar::Int(1))
            .unwrap_err();
        assert!(e.to_string().contains("out of range"), "got: {e}");
    }

    #[test]
    fn crlf_line_endings_are_preserved() {
        let crlf = PARAMS.replace('\n', "\r\n");
        let mut file = EditableUnityFile::parse(&crlf).unwrap();
        file.set_scalar(0, &parse_path("m_Name"), Scalar::Str("X"))
            .unwrap();
        let out = file.into_string();
        assert!(
            out.contains("m_Name: X\r\n"),
            "CRLF preserved around the edit"
        );
        assert!(out.contains("type: 3}\r\n"));
    }

    #[test]
    fn parse_path_distinguishes_keys_and_indices() {
        assert_eq!(
            parse_path("parameters/0/defaultValue"),
            vec![
                Seg::Key("parameters".into()),
                Seg::Index(0),
                Seg::Key("defaultValue".into())
            ]
        );
        assert_eq!(parse_path("/m_Name/"), vec![Seg::Key("m_Name".into())]);
    }
}

#[cfg(test)]
mod structural_tests {
    use super::*;

    const PREFAB: &str = "\
%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100
GameObject:
  m_ObjectHideFlags: 0
  serializedVersion: 6
  m_Component:
  - component: {fileID: 400}
  - component: {fileID: 11400}
  - component: {fileID: 18300}
  m_Layer: 0
  m_Name: Root
--- !u!4 &400
Transform:
  m_GameObject: {fileID: 100}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_Children: []
  m_Father: {fileID: 0}
--- !u!114 &11400
MonoBehaviour:
  m_GameObject: {fileID: 100}
  m_Script: {fileID: 11500000, guid: aaaa0000aaaa0000aaaa0000aaaa0000, type: 3}
  m_Name:
  m_Damping: 0.2
--- !u!183 &18300
Cloth:
  m_GameObject: {fileID: 100}
  m_Enabled: 1
--- !u!1107 &1107000
AnimatorStateMachine:
  m_Name: SM
  m_ChildStates:
  - serializedVersion: 1
    m_State: {fileID: 1102001}
    m_Position: {x: 200, y: 0, z: 0}
  - serializedVersion: 1
    m_State: {fileID: 1102002}
    m_Position: {x: 200, y: 120, z: 0}
  m_DefaultState: {fileID: 1102001}
";

    fn file() -> EditableUnityFile {
        EditableUnityFile::parse(PREFAB).unwrap()
    }

    #[test]
    fn remove_document_drops_header_and_body_only() {
        let mut f = file();
        let cloth = f.doc_by_file_id(18300).unwrap();
        f.remove_document(cloth).unwrap();
        assert!(!f.text().contains("Cloth:"));
        assert!(!f.text().contains("&18300"));
        // Neighbours are byte-identical.
        assert!(
            f.text()
                .contains("  m_Damping: 0.2\n--- !u!1107 &1107000\n")
        );
        assert!(f.doc_by_file_id(18300).is_none());
        assert_eq!(f.documents().len(), 4);
    }

    #[test]
    fn replace_document_body_keeps_header_and_references() {
        let mut f = file();
        let mb = f.doc_by_file_id(11400).unwrap();
        f.replace_document_body(
            mb,
            "MonoBehaviour:\n  m_GameObject: {fileID: 100}\n  m_Script: {fileID: 1661641543, guid: 2a2c05204084d904aa4945ccff20d8e5, type: 3}\n  pull: 0.5",
        )
        .unwrap();
        assert!(f.text().contains("--- !u!114 &11400\nMonoBehaviour:\n  m_GameObject: {fileID: 100}\n  m_Script: {fileID: 1661641543"));
        assert!(!f.text().contains("m_Damping"));
        // The owning GameObject still lists it.
        assert!(f.text().contains("- component: {fileID: 11400}"));
        // Trailing newline was added; the next header is intact.
        assert!(f.text().contains("  pull: 0.5\n--- !u!183 &18300\n"));
    }

    #[test]
    fn retag_document_rewrites_header() {
        let mut f = file();
        let d = f.doc_by_file_id(18300).unwrap();
        f.retag_document(d, 114, 18300).unwrap();
        assert!(f.text().contains("--- !u!114 &18300\nCloth:"));
        assert_eq!(f.documents()[d].class_id, 114);
    }

    #[test]
    fn append_document_adds_at_end_and_rejects_duplicate_ids() {
        let mut f = file();
        let idx = f
            .append_document(
                114,
                999,
                "MonoBehaviour:\n  m_GameObject: {fileID: 100}\n  m_Name: New",
            )
            .unwrap();
        assert_eq!(idx, f.documents().len() - 1);
        assert!(f.text().ends_with(
            "--- !u!114 &999\nMonoBehaviour:\n  m_GameObject: {fileID: 100}\n  m_Name: New\n"
        ));
        assert!(
            f.append_document(114, 999, "MonoBehaviour:\n  m_Name: Dup")
                .is_err()
        );
    }

    #[test]
    fn append_sequence_item_converts_empty_flow_list_to_block() {
        let mut f = file();
        let tr = f.doc_by_file_id(400).unwrap();
        f.append_sequence_item(tr, &parse_path("m_Children"), "{fileID: 401}")
            .unwrap();
        assert!(
            f.text()
                .contains("  m_Children:\n  - {fileID: 401}\n  m_Father: {fileID: 0}\n")
        );
        assert_eq!(f.sequence_len(tr, &parse_path("m_Children")).unwrap(), 1);
        f.append_sequence_item(tr, &parse_path("m_Children"), "{fileID: 402}")
            .unwrap();
        assert!(
            f.text()
                .contains("  - {fileID: 401}\n  - {fileID: 402}\n  m_Father")
        );
    }

    #[test]
    fn append_sequence_item_multiline_element() {
        let mut f = file();
        let sm = f.doc_by_file_id(1107000).unwrap();
        f.append_sequence_item(
            sm,
            &parse_path("m_ChildStates"),
            "serializedVersion: 1\nm_State: {fileID: 1102003}\nm_Position: {x: 200, y: 240, z: 0}",
        )
        .unwrap();
        assert!(f.text().contains(
            "    m_Position: {x: 200, y: 120, z: 0}\n  - serializedVersion: 1\n    m_State: {fileID: 1102003}\n    m_Position: {x: 200, y: 240, z: 0}\n  m_DefaultState:"
        ));
        assert_eq!(f.sequence_len(sm, &parse_path("m_ChildStates")).unwrap(), 3);
    }

    #[test]
    fn remove_sequence_item_middle_and_last() {
        let mut f = file();
        let go = f.doc_by_file_id(100).unwrap();
        let p = parse_path("m_Component");
        let items = f.sequence_items(go, &p).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1], "  - component: {fileID: 11400}\n");
        f.remove_sequence_item(go, &p, 1).unwrap();
        assert!(f.text().contains("  m_Component:\n  - component: {fileID: 400}\n  - component: {fileID: 18300}\n  m_Layer: 0\n"));
        f.remove_sequence_item(go, &p, 1).unwrap();
        f.remove_sequence_item(go, &p, 0).unwrap();
        assert!(f.text().contains("  m_Component: []\n  m_Layer: 0\n"));
        assert_eq!(f.sequence_len(go, &p).unwrap(), 0);
        assert!(f.remove_sequence_item(go, &p, 0).is_err());
        // Round-trips back to block form.
        f.append_sequence_item(go, &p, "component: {fileID: 400}")
            .unwrap();
        assert!(
            f.text()
                .contains("  m_Component:\n  - component: {fileID: 400}\n  m_Layer: 0\n")
        );
    }

    #[test]
    fn remove_sequence_item_multiline_element() {
        let mut f = file();
        let sm = f.doc_by_file_id(1107000).unwrap();
        f.remove_sequence_item(sm, &parse_path("m_ChildStates"), 0)
            .unwrap();
        assert!(f.text().contains(
            "  m_ChildStates:\n  - serializedVersion: 1\n    m_State: {fileID: 1102002}\n    m_Position: {x: 200, y: 120, z: 0}\n  m_DefaultState:"
        ));
    }

    #[test]
    fn locate_sequence_rejects_scalars_and_flow_maps() {
        let f = file();
        let go = f.doc_by_file_id(100).unwrap();
        assert!(f.sequence_len(go, &parse_path("m_Name")).is_err());
        let tr = f.doc_by_file_id(400).unwrap();
        assert!(f.sequence_len(tr, &parse_path("m_Father")).is_err());
    }
}
