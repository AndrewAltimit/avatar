//! Adversarial-input battery for the Unity YAML reader: empty/garbage input, missing or malformed
//! `--- !u!<classID> &<fileID>` headers, garbage class/file ids, non-string (all-digit) GUIDs, a
//! truncated document, malformed YAML bodies, and pathologically deep nesting. Every case must
//! yield a `Result` (or a graceful empty parse), never a panic. YAML is embedded as string
//! literals — no fixtures committed.

use std::panic::catch_unwind;

use avatar_unity_yaml::{
    UnityFile, field_bool, field_f64, field_i64, field_str, meta_guid, parse_meta,
};

/// Parse under a panic guard. Panics (which `catch_unwind` traps) fail the test loudly; any
/// returned `Result` — Ok or Err — is acceptable.
fn parse_no_panic(name: &str, text: &str) -> UnityFile {
    let owned = text.to_string();
    match catch_unwind(|| UnityFile::parse(&owned)) {
        Ok(Ok(f)) => f,
        Ok(Err(_)) => UnityFile {
            documents: Vec::new(),
        },
        Err(_) => panic!("{name}: PANICKED on malformed input"),
    }
}

#[test]
fn empty_input_parses_empty() {
    let f = parse_no_panic("empty", "");
    assert!(f.documents.is_empty());
}

#[test]
fn non_unity_yaml_yields_no_documents() {
    // Plain text and ordinary (non-`--- !u!`) YAML both have no Unity headers → no documents.
    assert!(
        parse_no_panic("plain", "just some text, not yaml at all")
            .documents
            .is_empty()
    );
    assert!(
        parse_no_panic("ordinary_yaml", "key: value\nlist:\n  - 1\n  - 2\n")
            .documents
            .is_empty()
    );
}

#[test]
fn missing_header_yields_no_documents() {
    // A body with no `---` header line is dropped (there is no class/file id to recover).
    let f = parse_no_panic("missing_header", "MonoBehaviour:\n  m_Name: x\n");
    assert!(f.documents.is_empty());
}

#[test]
fn garbage_class_and_file_ids_are_skipped() {
    // Non-numeric class id / file id → `parse_header` returns None → the document is skipped, not
    // a panic.
    let f = parse_no_panic(
        "garbage_ids",
        "--- !u!abc &xyz\nMonoBehaviour:\n  m_Name: x\n",
    );
    assert!(f.documents.is_empty());

    // Overflowing class id (> u32::MAX) and a file id that parses are likewise handled gracefully.
    let f = parse_no_panic(
        "overflow_classid",
        "--- !u!999999999999 &-5\nGameObject:\n  m_Name: x\n",
    );
    assert!(f.documents.is_empty(), "u32-overflow class id is skipped");
}

#[test]
fn header_without_body_is_a_null_document() {
    // A header with an empty body (e.g. a stripped prefab placeholder) yields one document with a
    // null body — not a panic.
    let f = parse_no_panic("header_only", "--- !u!114 &11400000\n");
    assert_eq!(f.documents.len(), 1);
    assert_eq!(f.documents[0].class_id, 114);
    assert_eq!(f.documents[0].file_id, 11400000);
    assert_eq!(f.documents[0].type_name, "");
}

#[test]
fn all_digit_guid_degrades_to_none() {
    // The documented gotcha: an all-digit "guid" is parsed by yaml-rust2 as a *number*, so
    // `as_str()` is None. The reader must degrade gracefully (return None), never unwrap.
    let text = "--- !u!114 &1\nMonoBehaviour:\n  m_Script: {fileID: 1, guid: 12345678901234567890123456789012, type: 3}\n  m_Name: Foo\n";
    let f = parse_no_panic("all_digit_guid", text);
    assert_eq!(f.documents.len(), 1);
    assert_eq!(
        f.documents[0].script_guid(),
        None,
        "all-digit guid is a YAML number, so as_str() is None"
    );
    // A normal (letter-containing) guid resolves fine.
    let ok = "--- !u!114 &1\nMonoBehaviour:\n  m_Script: {fileID: 1, guid: aaaa5678901234567890123456789012, type: 3}\n";
    let f = parse_no_panic("letter_guid", ok);
    assert_eq!(
        f.documents[0].script_guid(),
        Some("aaaa5678901234567890123456789012")
    );
}

#[test]
fn malformed_yaml_body_returns_err_not_panic() {
    // A header that parses but a body that is not valid YAML: `UnityFile::parse` surfaces the
    // loader error as an `Err` (caught here), never a panic.
    let cases = [
        "--- !u!1 &1\nGameObject:\n  : : :\n  - [\n",
        "--- !u!1 &1\nGameObject:\n  m_Component: [{a: 1",
        "--- !u!1 &1\nGameObject:\n\t\tbad-tab-indent: 1\n",
    ];
    for (i, body) in cases.iter().enumerate() {
        let owned = (*body).to_string();
        match catch_unwind(|| UnityFile::parse(&owned)) {
            Ok(_) => {} // Ok or Err both fine — the point is it returned
            Err(_) => panic!("malformed body case {i} PANICKED"),
        }
    }
}

#[test]
fn truncated_document_does_not_panic() {
    // A document cut off mid-mapping / mid-flow.
    let cases = [
        "--- !u!114 &11400000\nMonoBehaviour:\n  m_Script: {fileID: 11500",
        "%YAML 1.1\n%TAG !u! tag:unity3d.com,2011:\n--- !u!114 &1\nMonoBehaviour:\n  m_Na",
    ];
    for (i, body) in cases.iter().enumerate() {
        let owned = (*body).to_string();
        assert!(
            catch_unwind(|| UnityFile::parse(&owned)).is_ok(),
            "truncated case {i} PANICKED"
        );
    }
}

#[test]
fn deeply_nested_yaml_returns_err_not_stack_overflow() {
    // Pathologically deep flow nesting. yaml-rust2 0.11 bounds recursion and returns an Err
    // ("recursion limit exceeded") rather than overflowing the stack — assert we surface a Result
    // and never panic / abort.
    let depth = 100_000;
    let body = format!(
        "--- !u!1 &1\nGameObject:\n  x: {}{}",
        "[".repeat(depth),
        "]".repeat(depth)
    );
    let owned = body.clone();
    match catch_unwind(|| UnityFile::parse(&owned)) {
        Ok(Ok(_)) | Ok(Err(_)) => {} // returned a Result — good
        Err(_) => panic!("deeply nested YAML PANICKED / overflowed"),
    }
}

#[test]
fn meta_and_parse_meta_handle_garbage() {
    // `meta_guid` / `parse_meta` over empty, non-string-guid, and garbage input never panic.
    assert_eq!(meta_guid(""), None);
    assert_eq!(meta_guid("not: a guid here\n"), None);
    // All-digit guid in a .meta degrades to None (same number-vs-string gotcha).
    assert_eq!(meta_guid("guid: 12345678901234567890123456789012\n"), None);
    assert_eq!(
        meta_guid("fileFormatVersion: 2\nguid: aaaa5678901234567890123456789012\n").as_deref(),
        Some("aaaa5678901234567890123456789012")
    );
    assert!(parse_meta("").is_none());
    assert!(parse_meta(": : :\n[").is_none()); // malformed → None, not panic
}

#[test]
fn field_helpers_tolerate_wrong_types() {
    // The convenience field readers must return None (never panic) on missing keys and on values
    // of an unexpected YAML type.
    let f = parse_no_panic(
        "fields",
        "--- !u!114 &1\nMonoBehaviour:\n  num: 42\n  txt: hello\n  flag: 1\n",
    );
    let body = &f.documents[0].body;
    assert_eq!(field_i64(body, "num"), Some(42));
    assert_eq!(field_f64(body, "num"), Some(42.0));
    assert_eq!(field_bool(body, "flag"), Some(true));
    assert_eq!(field_str(body, "txt"), Some("hello"));
    // Missing keys and type mismatches → None.
    assert_eq!(field_i64(body, "absent"), None);
    assert_eq!(field_str(body, "num"), None);
    assert_eq!(field_i64(body, "txt"), None);
}
