//! Adversarial-input battery for the FBX reader: truncated headers, truncated bodies, hostile
//! length fields, absurd array counts, pathological nesting, and a deterministic random-bytes
//! fuzz loop. Every case must return a clean `anyhow::Error` (or a graceful empty result) and
//! must never panic. Bytes are synthesized in-test via the `fbxcel` writer or hand-built — no
//! binary fixtures are committed (per the repo's no-user-asset policy).

use std::io::Cursor;
use std::panic::catch_unwind;

use avatar_fbx::FbxDocument;
use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

/// Serialize a tree to binary-FBX bytes.
fn to_bytes(tree: &Tree) -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(tree).unwrap();
    w.finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner()
}

/// A small valid FBX: one named `Model` plus a small `f64` array, so the bytes contain both a
/// string attribute (corruptible length field) and an array attribute.
fn valid_fbx() -> Vec<u8> {
    let tree = tree_v7400! {
        Objects: {
            Model: [100i64, "HelloWorldName\u{0}\u{1}Model", "LimbNode"] {
                Properties70: {
                    P: ["Lcl Scaling", "Lcl Scaling", "", "A", 1.0f64, 1.0f64, 1.0f64] {},
                },
            },
            Geometry: [200i64, "G\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]] {},
                PolygonVertexIndex: [vec![0i32, 1, -3]] {},
            },
        },
        Connections: {
            C: ["OO", 200i64, 100i64] {},
        },
    };
    to_bytes(&tree)
}

/// Run `FbxDocument::from_bytes` under a panic guard; `true` if it returned (Ok or Err) without
/// panicking, `false` if it panicked.
fn parses_without_panic(bytes: &[u8]) -> bool {
    let owned = bytes.to_vec();
    catch_unwind(|| {
        let _ = FbxDocument::from_bytes(&owned);
    })
    .is_ok()
}

/// Assert that input is rejected with `Err` (and never panics).
fn assert_err_no_panic(name: &str, bytes: &[u8]) {
    let owned = bytes.to_vec();
    match catch_unwind(|| FbxDocument::from_bytes(&owned)) {
        Ok(Ok(_)) => panic!("{name}: expected Err, got Ok"),
        Ok(Err(_)) => {} // good: clean anyhow error
        Err(_) => panic!("{name}: PANICKED on malformed input"),
    }
}

#[test]
fn empty_input_errors() {
    assert_err_no_panic("empty", &[]);
}

#[test]
fn garbage_input_errors() {
    assert_err_no_panic("garbage", &[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01, 0x02]);
}

#[test]
fn truncated_header_errors() {
    // Just the start of the magic, nothing else.
    assert_err_no_panic("partial_magic", b"Kaydara FBX");

    // Full magic but no version / body.
    let mut magic = b"Kaydara FBX Binary  \x00".to_vec();
    magic.extend_from_slice(&[0x1a, 0x00]);
    assert_err_no_panic("magic_only", &magic);

    // Magic + version but no node data.
    let mut magic_ver = magic.clone();
    magic_ver.extend_from_slice(&7400u32.to_le_bytes());
    assert_err_no_panic("magic_plus_version", &magic_ver);
}

#[test]
fn valid_header_then_truncated_body_errors() {
    let valid = valid_fbx();
    assert!(valid.len() > 128, "fixture should be non-trivial");
    // The 27-byte header is intact; the node body is cut mid-stream at several offsets. (Cuts are
    // kept within the node data — truncating only the trailing footer is legitimately tolerated by
    // the reader, since the tree is fully recoverable without it.)
    for cut in [27usize, 40, 64, valid.len() / 2, valid.len() * 3 / 4] {
        let cut = cut.min(valid.len());
        assert_err_no_panic(&format!("truncated@{cut}"), &valid[..cut]);
    }
}

#[test]
fn hostile_string_length_does_not_panic() {
    // Overwrite the u32 length prefix of the embedded string attribute with a huge value. The
    // reader must surface this as an Err (it reads past the node end / hits invalid UTF-8 /
    // short read), never an indexing panic. (The allocation is bounded by fbxcel's `take`.)
    let valid = valid_fbx();
    let needle = b"HelloWorldName";
    let pos = valid
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("fixture string present");
    let mut corrupt = valid.clone();
    corrupt[pos - 4..pos].copy_from_slice(&0x7FFF_FFF0u32.to_le_bytes());
    assert_err_no_panic("hostile_string_len", &corrupt);
}

#[test]
fn absurd_array_count_does_not_panic() {
    // An FBX array attribute is `type_byte ('d'=ArrF64) | u32 element_count | u32 encoding |
    // u32 byte_len | payload`. Inflate the element_count immediately after each `'d'` marker to
    // u32::MAX. fbxcel streams array elements (the collect's size_hint lower bound is 0, so it
    // does NOT pre-allocate the hostile count) — so this must hit a short read and surface an Err,
    // never an OOM or an indexing panic. (Bytes other than the real marker are harmless false
    // positives: the property under test is "no panic", which must hold for those too.)
    let valid = valid_fbx();
    let mut hit_a_marker = false;
    for i in 0..valid.len().saturating_sub(8) {
        if valid[i] == b'd' {
            hit_a_marker = true;
            let mut corrupt = valid.clone();
            corrupt[i + 1..i + 5].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            assert!(
                parses_without_panic(&corrupt),
                "absurd array count at offset {i} caused a panic"
            );
        }
    }
    assert!(hit_a_marker, "fixture should contain an f64 array marker");
}

#[test]
fn pathologically_nested_input_errors() {
    // Build a node nesting far deeper than MAX_NODE_DEPTH (1024). Must bail cleanly.
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    let depth = 4096;
    for _ in 0..depth {
        let _ = w.new_node("N").unwrap();
    }
    for _ in 0..depth {
        w.close_node().unwrap();
    }
    let bytes = w
        .finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner();
    assert_err_no_panic("deeply_nested", &bytes);
}

#[test]
fn deterministic_random_bytes_never_panic() {
    // A tiny, seeded (NOT Math.random / NOT thread RNG) xorshift PRNG so the fuzz set is fully
    // reproducible. Every random buffer thrown at the entry point must return — Ok or Err — and
    // never panic.
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15; // fixed seed
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for case in 0..512u32 {
        let len = (next() % 256) as usize;
        let mut buf = vec![0u8; len];
        for b in buf.iter_mut() {
            *b = (next() & 0xff) as u8;
        }
        // Half the cases get a valid magic prefix so the bytes reach deeper parsing stages.
        if case % 2 == 0 && buf.len() >= 23 {
            let mut magic = b"Kaydara FBX Binary  \x00".to_vec();
            magic.extend_from_slice(&[0x1a, 0x00]);
            let n = magic.len().min(buf.len());
            buf[..n].copy_from_slice(&magic[..n]);
        }
        assert!(
            parses_without_panic(&buf),
            "random buffer (case {case}, len {len}) caused a panic"
        );
    }
}
