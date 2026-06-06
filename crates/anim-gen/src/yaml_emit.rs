//! Low-level helpers for emitting Unity-flavoured YAML.
//!
//! Unity's serializer has a handful of stable typographic conventions a generator must match
//! exactly or the import is rejected / mis-parsed:
//!
//! - Two-space indentation; sequences are `- ` at the *parent's* indent (Unity does not add an
//!   extra indent level for the `-`).
//! - Small fixed-shape structs are written as **inline (flow) maps**: `{x: 0, y: 0, z: 0}`,
//!   `{fileID: 7400000, guid: ..., type: 2}`. We reproduce that shape rather than block style so a
//!   diff against a Unity-authored asset is minimal.
//! - Floats are written without a trailing `.0` when integral (`0`, `1`), matching Unity (which
//!   prints `m_Threshold: 0`, not `0.0`). Non-integral floats print with enough precision to
//!   round-trip an `f32`.
//! - Object references are the inline `{fileID: N}` (local) or `{fileID: N, guid: G, type: T}`
//!   (external asset) maps; a null reference is `{fileID: 0}`.
//!
//! These helpers are deliberately small and string-based: the documents we generate are fixed in
//! shape, so a typed-then-rendered approach is clearer (and easier to diff against real Unity
//! output) than a generic YAML emitter.

use std::fmt::Write as _;

/// Render an `f32` the way Unity does: an integral value as a bare integer (`0`, `1`, `-2`), and a
/// fractional value with the shortest representation that round-trips. Unity uses up to ~9
/// significant digits for `float`; Rust's default `{}` for `f32` already yields the shortest
/// round-tripping decimal, which matches Unity closely enough for import (Unity re-quantizes on
/// import anyway).
pub fn fmt_f32(v: f32) -> String {
    if v == v.trunc() && v.is_finite() && v.abs() < 1e15 {
        // Integral: print without a decimal point. `-0.0` collapses to `0`.
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// A Unity object reference: either local (`{fileID: N}`) or external (`{fileID: N, guid: G,
/// type: T}`). A null reference is the local form with `file_id == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRef {
    pub file_id: i64,
    /// Present for references into another asset (e.g. an `.fbx` mesh, or a clip in a different
    /// `.anim`). `None` for a reference within the same file.
    pub guid: Option<String>,
    /// Unity asset `type`: 2 = an asset imported by a native importer (meshes, clips inside model
    /// files), 3 = a script/MonoBehaviour. Only meaningful when `guid` is set.
    pub asset_type: i64,
}

impl ObjectRef {
    /// A reference to another object within the *same* file, by local file id.
    pub fn local(file_id: i64) -> Self {
        ObjectRef {
            file_id,
            guid: None,
            asset_type: 0,
        }
    }

    /// The null reference, `{fileID: 0}`.
    pub fn null() -> Self {
        ObjectRef::local(0)
    }

    /// A reference into another asset file by its GUID (e.g. an AnimationClip in a `.anim`, or a
    /// mesh inside an `.fbx`).
    pub fn external(file_id: i64, guid: impl Into<String>, asset_type: i64) -> Self {
        ObjectRef {
            file_id,
            guid: Some(guid.into()),
            asset_type,
        }
    }

    /// Render to Unity's inline reference syntax.
    pub fn render(&self) -> String {
        match &self.guid {
            Some(g) => format!(
                "{{fileID: {}, guid: {}, type: {}}}",
                self.file_id, g, self.asset_type
            ),
            None => format!("{{fileID: {}}}", self.file_id),
        }
    }
}

/// A small string builder that tracks the current indent level and emits Unity-style lines.
///
/// `Emitter` is intentionally low-level: callers decide structure, it handles indentation, the
/// `key: value` shape, and sequence dashes. One indent level is two spaces.
#[derive(Debug, Default)]
pub struct Emitter {
    buf: String,
    indent: usize,
}

impl Emitter {
    pub fn new() -> Self {
        Emitter::default()
    }

    fn pad(&mut self) {
        for _ in 0..self.indent {
            self.buf.push_str("  ");
        }
    }

    /// Emit a raw line at the current indent.
    pub fn line(&mut self, s: &str) {
        self.pad();
        self.buf.push_str(s);
        self.buf.push('\n');
    }

    /// Emit a `key: value` line at the current indent.
    pub fn kv(&mut self, key: &str, value: &str) {
        self.pad();
        let _ = write!(self.buf, "{key}: {value}");
        self.buf.push('\n');
    }

    /// Emit a `key:` line with no value (introducing a nested block or sequence).
    pub fn key(&mut self, key: &str) {
        self.pad();
        self.buf.push_str(key);
        self.buf.push_str(":\n");
    }

    /// Emit a mapping key whose value is an integer.
    pub fn kv_i64(&mut self, key: &str, value: i64) {
        self.kv(key, &value.to_string());
    }

    /// Emit a mapping key whose value is a Unity float.
    pub fn kv_f32(&mut self, key: &str, value: f32) {
        self.kv(key, &fmt_f32(value));
    }

    /// Emit a mapping key whose value is an object reference.
    pub fn kv_ref(&mut self, key: &str, value: &ObjectRef) {
        self.kv(key, &value.render());
    }

    /// Run `f` with the indent increased by one level.
    pub fn indented(&mut self, f: impl FnOnce(&mut Emitter)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    /// Emit a `--- !u!<class_id> &<file_id>` document header (always at column 0).
    pub fn doc_header(&mut self, class_id: u32, file_id: i64) {
        let _ = writeln!(self.buf, "--- !u!{class_id} &{file_id}");
    }

    /// The accumulated text.
    pub fn into_string(self) -> String {
        self.buf
    }

    /// The accumulated text without consuming the emitter (for assertions in tests).
    pub fn as_str(&self) -> &str {
        &self.buf
    }
}

/// The `%YAML` / `%TAG` preamble Unity puts at the top of every multi-document asset stream.
pub const UNITY_PREAMBLE: &str = "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_floats_like_unity() {
        assert_eq!(fmt_f32(0.0), "0");
        assert_eq!(fmt_f32(-0.0), "0");
        assert_eq!(fmt_f32(1.0), "1");
        assert_eq!(fmt_f32(-2.0), "-2");
        assert_eq!(fmt_f32(0.5), "0.5");
        assert_eq!(fmt_f32(0.25), "0.25");
    }

    #[test]
    fn renders_object_refs() {
        assert_eq!(ObjectRef::null().render(), "{fileID: 0}");
        assert_eq!(ObjectRef::local(110200000).render(), "{fileID: 110200000}");
        assert_eq!(
            ObjectRef::external(7400000, "1234567890abcdef1234567890abcdef", 2).render(),
            "{fileID: 7400000, guid: 1234567890abcdef1234567890abcdef, type: 2}"
        );
    }

    #[test]
    fn emitter_indentation_is_two_spaces() {
        let mut e = Emitter::new();
        e.key("AnimationClip");
        e.indented(|e| {
            e.kv("m_Name", "Test");
            e.key("m_Curve");
            e.indented(|e| e.kv_i64("time", 0));
        });
        assert_eq!(
            e.as_str(),
            "AnimationClip:\n  m_Name: Test\n  m_Curve:\n    time: 0\n"
        );
    }
}
