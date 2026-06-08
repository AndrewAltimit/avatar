//! Surgical-edit round-trip against the *real* committed fixture corpus (not embedded snippets).
//!
//! The contract these tests pin: an edit changes exactly the targeted value and leaves every other
//! byte — every `&fileID` anchor, every `{fileID, guid}` reference, key order, indentation — intact,
//! and the result re-parses as the same set of documents. This is the property that makes the editor
//! safe to point at someone's actual avatar.

use avatar_testkit::corpus;
use avatar_unity_yaml::{EditableUnityFile, Scalar, UnityFile, parse_path};

const PARAMETERS: &str = "projects/AvatarProject/Assets/Avatar/Parameters.asset";
const HANDS: &str = "projects/AvatarProject/Assets/Avatar/Hands.controller";

fn read(rel: &str) -> String {
    std::fs::read_to_string(corpus(rel)).unwrap_or_else(|e| panic!("read fixture {rel}: {e}"))
}

/// The set of `&fileID` anchors in a file, as parsed by the reader — the thing references point at.
fn file_ids(text: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = UnityFile::parse(text)
        .expect("fixture parses")
        .documents
        .iter()
        .map(|d| d.file_id)
        .collect();
    ids.sort_unstable();
    ids
}

/// Count lines that differ between two texts of equal line count.
fn changed_lines(before: &str, after: &str) -> usize {
    assert_eq!(
        before.lines().count(),
        after.lines().count(),
        "a surgical edit must not add or remove lines"
    );
    before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .count()
}

#[test]
fn rename_parameters_asset_preserves_fileids_and_script_ref() {
    let src = read(PARAMETERS);
    let before_ids = file_ids(&src);

    let mut file = EditableUnityFile::parse(&src).unwrap();
    let doc = file
        .doc_by_file_id(11400000)
        .expect("the MonoBehaviour doc");
    file.set_scalar(doc, &parse_path("m_Name"), Scalar::Str("Parameters2"))
        .unwrap();
    let out = file.into_string();

    assert!(out.contains("m_Name: Parameters2"));
    // The script reference (fileID + guid + type) is byte-for-byte intact.
    assert!(
        out.contains(
            "m_Script: {fileID: 11500000, guid: 03b990c4d4d4f3a4f9c8c4d4f3a4f9c8, type: 3}"
        )
    );
    // Exactly one line changed; the anchor set is unchanged; it still parses.
    assert_eq!(changed_lines(&src, &out), 1);
    assert_eq!(file_ids(&out), before_ids);
}

#[test]
fn edit_nested_parameter_field_in_real_asset() {
    let src = read(PARAMETERS);
    let mut file = EditableUnityFile::parse(&src).unwrap();
    // Flip `saved` on the third parameter (FloatThing) from 0 → 1.
    file.set_scalar(0, &parse_path("parameters/2/saved"), Scalar::Int(1))
        .unwrap();
    let out = file.into_string();

    // Verify via the reader that the *third* element changed and the others did not.
    let parsed = UnityFile::parse(&out).unwrap();
    let params = parsed.documents[0].body["parameters"].as_vec().unwrap();
    let saved: Vec<i64> = params
        .iter()
        .map(|p| avatar_unity_yaml::field_i64(p, "saved").unwrap())
        .collect();
    assert_eq!(saved, vec![1, 1, 1]); // VRCEmote=1, Toggle1=1, FloatThing now 1
    assert_eq!(changed_lines(&src, &out), 1);
}

#[test]
fn retarget_motion_reference_in_real_controller() {
    let src = read(HANDS);
    let before_ids = file_ids(&src);

    let mut file = EditableUnityFile::parse(&src).unwrap();
    // The Fist state (fileID 110200002) points m_Motion at a local BlendTree; re-point it at a clip
    // in another asset by GUID — the canonical "swap an animation" edit.
    let doc = file.doc_by_file_id(110200002).unwrap();
    file.set_reference(
        doc,
        &parse_path("m_Motion"),
        7400000,
        Some("1234567890abcdef1234567890abcdef"),
        2,
    )
    .unwrap();
    let out = file.into_string();

    assert!(
        out.contains(
            "m_Motion: {fileID: 7400000, guid: 1234567890abcdef1234567890abcdef, type: 2}"
        )
    );
    // Every document anchor still present; the Idle state's own m_Motion untouched.
    assert_eq!(file_ids(&out), before_ids);
    assert_eq!(changed_lines(&src, &out), 1);
}

#[test]
fn fix_misnamed_blend_parameter_in_real_controller() {
    // A realistic lint-then-fix: the BlendTree's m_BlendParameter is `MissingBlend`; correct it.
    let src = read(HANDS);
    let mut file = EditableUnityFile::parse(&src).unwrap();
    let doc = file.doc_by_file_id(110600000).expect("the BlendTree doc");
    assert_eq!(file.documents()[doc].type_name, "BlendTree");
    file.set_scalar(
        doc,
        &parse_path("m_BlendParameter"),
        Scalar::Str("GestureLeftWeight"),
    )
    .unwrap();
    let out = file.into_string();

    assert!(out.contains("m_BlendParameter: GestureLeftWeight"));
    assert!(!out.contains("MissingBlend"));
    assert_eq!(changed_lines(&src, &out), 1);
}

#[test]
fn every_document_in_the_corpus_round_trips_unchanged_with_no_edits() {
    // Parsing then re-emitting with no edits must be the identity (the strongest round-trip claim).
    for rel in [PARAMETERS, HANDS] {
        let src = read(rel);
        let file = EditableUnityFile::parse(&src).unwrap();
        assert_eq!(file.into_string(), src, "no-op round trip changed {rel}");
    }
}
