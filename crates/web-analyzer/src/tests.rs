use std::io::Cursor;

use fbxcel::low::FbxVersion;
use fbxcel::tree::v7400::Tree;
use fbxcel::tree_v7400;
use fbxcel::writer::v7400::binary::{FbxFooter, Writer};

use super::{BlendshapeGroup, SceneView, analyze, classify_blendshape};

fn to_bytes(tree: &Tree) -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), FbxVersion::V7_4).unwrap();
    w.write_tree(tree).unwrap();
    w.finalize_and_flush(&FbxFooter::default())
        .unwrap()
        .into_inner()
}

/// PNG magic + a few bytes: enough for the MIME sniff (never decoded).
const PNG_STUB: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// A humanoid rig (every required bone, Mixamo names) skinning a two-quad strip that is authored
/// **lying along +Z** (hips at z≈0, head at z≈2) while declaring `UpAxis = Y` — so only the
/// hips→head auto-upright, not the axis fallback, stands it up. Plus: one textured material with
/// an embedded PNG, three blendshape channels, and a static prop mesh under a translated Model.
fn skinned_humanoid() -> Vec<u8> {
    let identity = vec![
        1.0f64, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];
    let tree = tree_v7400! {
        GlobalSettings: {
            Properties70: {
                P: ["UnitScaleFactor", "double", "Number", "", 100.0f64] {},
                P: ["UpAxis", "int", "Integer", "", 1i32] {},
            },
        },
        Objects: {
            Model: [10i64, "Body\u{0}\u{1}Model", "Mesh"] {},
            Geometry: [20i64, "Body\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![
                    0.0f64, 0.0, 0.0,  1.0, 0.0, 0.0,  1.0, 0.0, 1.0,  0.0, 0.0, 1.0,
                    1.0, 0.0, 2.0,  0.0, 0.0, 2.0,
                ]] {},
                PolygonVertexIndex: [vec![0i32, 1, 2, -4, 3, 2, 4, -6]] {},
                LayerElementUV: [0i32] {
                    MappingInformationType: ["ByPolygonVertex"] {},
                    ReferenceInformationType: ["IndexToDirect"] {},
                    UV: [vec![0.0f64, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]] {},
                    UVIndex: [vec![0i32, 1, 2, 3, 3, 2, 1, 0]] {},
                },
            },
            Model: [11i64, "Prop\u{0}\u{1}Model", "Mesh"] {
                Properties70: {
                    P: ["Lcl Translation", "Lcl Translation", "", "A", 5.0f64, 6.0f64, 7.0f64] {},
                },
            },
            Geometry: [21i64, "Prop\u{0}\u{1}Geometry", "Mesh"] {
                Vertices: [vec![0.0f64, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]] {},
                PolygonVertexIndex: [vec![0i32, 1, -3]] {},
            },
            Material: [60i64, "Skin\u{0}\u{1}Material", ""] {
                Properties70: {
                    P: ["DiffuseColor", "Color", "", "A", 0.5f64, 0.25f64, 0.125f64] {},
                },
            },
            Texture: [70i64, "SkinTex\u{0}\u{1}Texture", ""] {
                RelativeFilename: ["tex/skin.png"] {},
            },
            Video: [80i64, "SkinTex\u{0}\u{1}Video", "Clip"] {
                Content: [PNG_STUB.to_vec()] {},
            },
            Deformer: [40i64, "Skin\u{0}\u{1}Deformer", "Skin"] {},
            SubDeformer: [50i64, "Hips\u{0}\u{1}SubDeformer", "Cluster"] {
                Indexes: [vec![0i32, 1]] {},
                Weights: [vec![1.0f64, 1.0]] {},
                Transform: [identity.clone()] {},
                TransformLink: [identity.clone()] {},
            },
            SubDeformer: [51i64, "Spine\u{0}\u{1}SubDeformer", "Cluster"] {
                Indexes: [vec![2i32, 3, 4]] {},
                Weights: [vec![1.0f64, 1.0, 0.25]] {},
                Transform: [identity.clone()] {},
                TransformLink: [identity.clone()] {},
            },
            SubDeformer: [52i64, "Head\u{0}\u{1}SubDeformer", "Cluster"] {
                Indexes: [vec![4i32, 5]] {},
                Weights: [vec![1.0f64, 1.0]] {},
                Transform: [identity.clone()] {},
                TransformLink: [identity] {},
            },
            Deformer: [90i64, "Shapes\u{0}\u{1}Deformer", "BlendShape"] {},
            Deformer: [91i64, "vrc.v_aa\u{0}\u{1}Deformer", "BlendShapeChannel"] {},
            Deformer: [92i64, "Blink\u{0}\u{1}Deformer", "BlendShapeChannel"] {},
            Deformer: [93i64, "Smile\u{0}\u{1}Deformer", "BlendShapeChannel"] {},
            Deformer: [94i64, "Fcl_MTH_Custom\u{0}\u{1}Deformer", "BlendShapeChannel"] {},
            // The rig.
            Model: [100i64, "mixamorig:Hips\u{0}\u{1}Model", "LimbNode"] {},
            Model: [101i64, "mixamorig:Spine\u{0}\u{1}Model", "LimbNode"] {},
            Model: [102i64, "mixamorig:Spine1\u{0}\u{1}Model", "LimbNode"] {},
            Model: [103i64, "mixamorig:Neck\u{0}\u{1}Model", "LimbNode"] {},
            Model: [104i64, "mixamorig:Head\u{0}\u{1}Model", "LimbNode"] {},
            Model: [110i64, "mixamorig:LeftShoulder\u{0}\u{1}Model", "LimbNode"] {},
            Model: [111i64, "mixamorig:LeftArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [112i64, "mixamorig:LeftForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [113i64, "mixamorig:LeftHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [120i64, "mixamorig:RightShoulder\u{0}\u{1}Model", "LimbNode"] {},
            Model: [121i64, "mixamorig:RightArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [122i64, "mixamorig:RightForeArm\u{0}\u{1}Model", "LimbNode"] {},
            Model: [123i64, "mixamorig:RightHand\u{0}\u{1}Model", "LimbNode"] {},
            Model: [130i64, "mixamorig:LeftUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [131i64, "mixamorig:LeftLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [132i64, "mixamorig:LeftFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [133i64, "mixamorig:LeftToeBase\u{0}\u{1}Model", "LimbNode"] {},
            Model: [140i64, "mixamorig:RightUpLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [141i64, "mixamorig:RightLeg\u{0}\u{1}Model", "LimbNode"] {},
            Model: [142i64, "mixamorig:RightFoot\u{0}\u{1}Model", "LimbNode"] {},
            Model: [143i64, "mixamorig:RightToeBase\u{0}\u{1}Model", "LimbNode"] {},
        },
        Connections: {
            C: ["OO", 20i64, 10i64] {},
            C: ["OO", 21i64, 11i64] {},
            C: ["OO", 60i64, 10i64] {},
            C: ["OP", 70i64, 60i64, "DiffuseColor"] {},
            C: ["OO", 80i64, 70i64] {},
            C: ["OO", 40i64, 20i64] {},
            C: ["OO", 50i64, 40i64] {},
            C: ["OO", 51i64, 40i64] {},
            C: ["OO", 52i64, 40i64] {},
            C: ["OO", 90i64, 20i64] {},
            C: ["OO", 91i64, 90i64] {},
            C: ["OO", 92i64, 90i64] {},
            C: ["OO", 93i64, 90i64] {},
            C: ["OO", 94i64, 90i64] {},
            C: ["OO", 101i64, 100i64] {},
            C: ["OO", 102i64, 101i64] {},
            C: ["OO", 103i64, 102i64] {},
            C: ["OO", 104i64, 103i64] {},
            C: ["OO", 110i64, 102i64] {},
            C: ["OO", 111i64, 110i64] {},
            C: ["OO", 112i64, 111i64] {},
            C: ["OO", 113i64, 112i64] {},
            C: ["OO", 120i64, 102i64] {},
            C: ["OO", 121i64, 120i64] {},
            C: ["OO", 122i64, 121i64] {},
            C: ["OO", 123i64, 122i64] {},
            C: ["OO", 130i64, 100i64] {},
            C: ["OO", 131i64, 130i64] {},
            C: ["OO", 132i64, 131i64] {},
            C: ["OO", 133i64, 132i64] {},
            C: ["OO", 140i64, 100i64] {},
            C: ["OO", 141i64, 140i64] {},
            C: ["OO", 142i64, 141i64] {},
            C: ["OO", 143i64, 142i64] {},
            // Bone → cluster links come after the hierarchy, as exporters write them: the scene's
            // `parent_of` takes a child's *first* OO connection as its hierarchy parent.
            C: ["OO", 100i64, 50i64] {},
            C: ["OO", 101i64, 51i64] {},
            C: ["OO", 104i64, 52i64] {},
        },
    };
    to_bytes(&tree)
}

#[test]
fn analyzes_synthetic_humanoid() {
    let bytes = avatar_testkit::fbx::humanoid_skeleton();
    let report = analyze(&bytes, "humanoid.fbx").expect("analyze");
    assert!(report.fbx.models > 0);
    assert!(
        report.armature.is_humanoid_ready(),
        "synthetic humanoid must map clean"
    );
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    for key in [
        "fbx",
        "global_settings",
        "armature",
        "stats",
        "blendshapes",
        "meshes",
        "materials",
        "bone_tree",
    ] {
        assert!(json.get(key).is_some(), "missing key {key}");
    }
    assert!(json["stats"]["pc_overall"].is_string());
    assert_eq!(json["global_settings"]["up_axis"], 1);
    assert_eq!(json["global_settings"]["unit_scale_factor"], 1.0);
    assert!(json["global_settings"]["front_axis"].is_null());
    assert_eq!(json["meshes"].as_array().unwrap().len(), 0);
    let tree = json["bone_tree"].as_array().unwrap();
    assert_eq!(tree.len(), report.fbx.models);
    let hips = tree.iter().find(|b| b["humanoid"] == "Hips").unwrap();
    assert_eq!(hips["depth"], 0);
    assert!(hips["parent"].is_null());
    assert_eq!(hips["bone_like"], true);
    let spine = tree.iter().find(|b| b["humanoid"] == "Spine").unwrap();
    assert_eq!(spine["parent"], hips["id"]);
    assert_eq!(spine["depth"], 1);
}

#[test]
fn analyze_reports_meshes_materials_and_blendshape_groups() {
    let report = analyze(&skinned_humanoid(), "skinned.fbx").expect("analyze");
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
    let meshes = json["meshes"].as_array().unwrap();
    assert_eq!(meshes.len(), 2);
    let body = &meshes[0];
    assert_eq!(body["name"], "Body");
    assert_eq!(
        body["vertices"], 12,
        "one emitted vertex per triangle corner"
    );
    assert_eq!(body["control_points"], 6);
    assert_eq!(body["triangles"], 4);
    assert_eq!(body["skinned"], true);
    assert_eq!(body["material_slots"], 1);
    assert_eq!(body["bones_influencing"], 3);
    assert_eq!(meshes[1]["skinned"], false);

    let mats = json["materials"].as_array().unwrap();
    assert_eq!(mats.len(), 1);
    assert_eq!(mats[0]["name"], "Skin");
    assert_eq!(mats[0]["diffuse_color"][0], 0.5);
    assert_eq!(mats[0]["texture"]["relative"], "tex/skin.png");
    assert_eq!(mats[0]["texture"]["embedded"], true);
    assert_eq!(mats[0]["texture"]["embedded_bytes"], PNG_STUB.len());

    let groups: Vec<(String, String)> = json["blendshapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["name"].as_str().unwrap().to_string(),
                b["group"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(groups.contains(&("vrc.v_aa".into(), "viseme".into())));
    assert!(groups.contains(&("Blink".into(), "blink".into())));
    assert!(groups.contains(&("Smile".into(), "expression".into())));
    assert!(groups.contains(&("Fcl_MTH_Custom".into(), "other".into())));
}

#[test]
fn classifies_blendshape_names() {
    assert_eq!(classify_blendshape("vrc.v_aa"), BlendshapeGroup::Viseme);
    assert_eq!(classify_blendshape("VRC.v_SIL"), BlendshapeGroup::Viseme);
    assert_eq!(classify_blendshape("aa"), BlendshapeGroup::Viseme);
    assert_eq!(classify_blendshape("Blink"), BlendshapeGroup::Blink);
    assert_eq!(classify_blendshape("EyeClose_L"), BlendshapeGroup::Blink);
    assert_eq!(classify_blendshape("Wink"), BlendshapeGroup::Blink);
    assert_eq!(classify_blendshape("Smile"), BlendshapeGroup::Expression);
    assert_eq!(classify_blendshape("Angry"), BlendshapeGroup::Expression);
    assert_eq!(classify_blendshape("Mouth_Open"), BlendshapeGroup::Other);
}

#[test]
fn sample_round_trips_through_scene_view() {
    let bytes = super::sample_fbx();
    let report = analyze(&bytes, "sample.fbx").expect("analyze sample");
    assert!(report.armature.is_humanoid_ready());
    let view = SceneView::from_bytes(&bytes).expect("load sample");
    let m: serde_json::Value = serde_json::from_str(&view.manifest()).unwrap();

    // Real geometry: one skinned mesh of a few thousand triangles, two material slots.
    let meshes = m["meshes"].as_array().unwrap();
    assert_eq!(meshes.len(), 1);
    assert_eq!(meshes[0]["skinned"], true);
    let tris = meshes[0]["triangles"].as_u64().unwrap();
    assert!((2000..10000).contains(&tris), "triangles {tris}");
    assert_eq!(meshes[0]["material_slots"], serde_json::json!([0, 1]));
    let nv = meshes[0]["vertices"].as_u64().unwrap() as usize;
    assert_eq!(view.positions(0).len(), 3 * nv);
    assert_eq!(view.normals(0).len(), 3 * nv);
    assert_eq!(view.uvs(0).len(), 2 * nv);
    assert_eq!(view.triangle_materials(0).len(), tris as usize);
    assert!(view.triangle_materials(0).contains(&1));
    let w = view.skin_weights(0);
    assert!((0..nv).all(|v| (w[v * 4..v * 4 + 4].iter().sum::<f32>() - 1.0).abs() < 1e-5));

    let mats = m["materials"].as_array().unwrap();
    assert_eq!(mats.len(), 2);
    assert_eq!(mats[0]["name"], "Body");
    assert!(mats[0]["diffuse_color"].is_array());
    assert_eq!(mats[1]["name"], "Head");

    // Bones sit at distinct positions, with the head ~1.5 m above the feet (cm).
    let bones = m["bones"].as_array().unwrap();
    let pos = |slot: &str| -> Vec<f64> {
        let b = bones.iter().find(|b| b["humanoid"] == slot).unwrap();
        b["position"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    };
    let (hips, head, lfoot, lhand, rhand) = (
        pos("Hips"),
        pos("Head"),
        pos("LeftFoot"),
        pos("LeftHand"),
        pos("RightHand"),
    );
    assert!(head[1] - hips[1] > 40.0, "head {head:?} hips {hips:?}");
    assert!(head[1] - lfoot[1] > 130.0);
    assert!(
        lhand[0] > 50.0 && rhand[0] < -50.0,
        "T-pose hands {lhand:?} {rhand:?}"
    );
    let mut distinct: Vec<String> = bones.iter().map(|b| b["position"].to_string()).collect();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        bones.len(),
        "every bone at its own position"
    );
    assert!(
        bones
            .iter()
            .filter(|b| b["influenced_vertices"].as_u64().unwrap() > 0)
            .count()
            > 15
    );
    // Upright is (near-)identity: authored Y-up, hips→head already +Y.
    assert!((m["upright"][3].as_f64().unwrap() - 1.0).abs() < 1e-3);
    assert!(m["bounds"]["max"][1].as_f64().unwrap() > 160.0);

    let groups: Vec<(String, String)> = m["blendshapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["name"].as_str().unwrap().into(),
                b["group"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        vec![
            ("vrc.v_aa".to_string(), "viseme".to_string()),
            ("vrc.v_oh".into(), "viseme".into()),
            ("Blink".into(), "blink".into()),
            ("Smile".into(), "expression".into()),
        ]
    );
}

#[test]
fn skeleton_only_file_loads_with_bounds_from_bones() {
    let bytes = avatar_testkit::fbx::humanoid_skeleton();
    let view = SceneView::from_bytes(&bytes).expect("load skeleton");
    let m: serde_json::Value = serde_json::from_str(&view.manifest()).unwrap();
    assert_eq!(m["meshes"].as_array().unwrap().len(), 0);
    assert!(
        m["bones"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["humanoid"] == "Hips")
    );
    assert_eq!(m["upright"][3], 1.0);
    assert!(view.positions(0).is_empty());
}

#[test]
fn scene_view_exposes_uprighted_buffers() {
    let view = SceneView::from_bytes(&skinned_humanoid()).expect("load");
    let m: serde_json::Value = serde_json::from_str(&view.manifest()).unwrap();

    let meshes = m["meshes"].as_array().unwrap();
    assert_eq!(meshes.len(), 2);
    let nv = meshes[0]["vertices"].as_u64().unwrap() as usize;
    assert_eq!(nv, 12);
    assert_eq!(meshes[0]["material_slots"], serde_json::json!([0]));
    assert_eq!(view.positions(0).len(), 3 * nv);
    assert_eq!(view.normals(0).len(), 3 * nv);
    assert_eq!(view.uvs(0).len(), 2 * nv);
    assert_eq!(view.indices(0).len(), 12);
    assert!(view.triangle_materials(0).is_empty());
    assert_eq!(view.skin_indices(0).len(), 4 * nv);
    assert_eq!(view.skin_weights(0).len(), 4 * nv);
    // Every skinned vertex has normalized weights.
    let w = view.skin_weights(0);
    for v in 0..nv {
        let s: f32 = w[v * 4..v * 4 + 4].iter().sum();
        assert!((s - 1.0).abs() < 1e-5, "vertex {v} weights sum {s}");
    }
    // Computed normals are unit length (the file has none).
    let n = view.normals(0);
    for v in 0..nv {
        let l = (n[v * 3] * n[v * 3] + n[v * 3 + 1] * n[v * 3 + 1] + n[v * 3 + 2] * n[v * 3 + 2])
            .sqrt();
        assert!((l - 1.0).abs() < 1e-4, "vertex {v} normal length {l}");
    }
    // The strip was authored along +Z; hips→head is now +Y.
    let bones = m["bones"].as_array().unwrap();
    let pos = |slot: &str| -> [f64; 3] {
        let b = bones.iter().find(|b| b["humanoid"] == slot).unwrap();
        let p = b["position"].as_array().unwrap();
        [
            p[0].as_f64().unwrap(),
            p[1].as_f64().unwrap(),
            p[2].as_f64().unwrap(),
        ]
    };
    let (hips, head) = (pos("Hips"), pos("Head"));
    assert!(head[1] - hips[1] > 1.5, "head {head:?} above hips {hips:?}");
    assert!((head[0] - hips[0]).abs() < 1e-4 && (head[2] - hips[2]).abs() < 1e-4);
    let p = view.positions(0);
    let max_y = (0..nv).map(|v| p[v * 3 + 1]).fold(f32::MIN, f32::max);
    assert!((max_y - 2.0).abs() < 1e-3, "strip top at y=2, got {max_y}");
    assert_eq!(m["bounds"]["max"][1].as_f64().unwrap().round(), 2.0);
    let hips_bone = bones.iter().find(|b| b["humanoid"] == "Hips").unwrap();
    assert_eq!(
        hips_bone["influenced_vertices"], 3,
        "cps 0 and 1 across the corners of quad A"
    );
    assert!(hips_bone["parent"].is_null());
    let spine = bones.iter().find(|b| b["humanoid"] == "Spine").unwrap();
    assert_eq!(spine["parent"], hips_bone["index"]);
    // Skin indices point at the manifest bone list.
    let hips_index = hips_bone["index"].as_u64().unwrap() as u32;
    assert_eq!(view.skin_indices(0)[0], hips_index);
    // An unskinned Model gets its composed Lcl translation, through the same upright rotation
    // (here Z→Y: (5,6,7) ↦ (5,7,-6)).
    let prop = bones.iter().find(|b| b["name"] == "Prop").unwrap();
    let pp: Vec<f64> = prop["position"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    for (got, want) in pp.iter().zip([5.0, 7.0, -6.0]) {
        assert!((got - want).abs() < 1e-4, "prop position {pp:?}");
    }
    assert_eq!(prop["bone_like"], false);

    // Materials + textures.
    let mats = m["materials"].as_array().unwrap();
    assert_eq!(mats.len(), 1);
    assert_eq!(mats[0]["texture"]["mime"], "image/png");
    assert_eq!(view.texture(0), PNG_STUB.to_vec());
    assert!(view.texture(1).is_empty());

    // Blendshapes resolve to their mesh index and group.
    let bs = m["blendshapes"].as_array().unwrap();
    assert_eq!(bs.len(), 4);
    assert!(bs.iter().all(|b| b["mesh"] == 0));
    assert!(
        bs.iter()
            .any(|b| b["name"] == "vrc.v_aa" && b["group"] == "viseme")
    );

    // Static mesh: no skin, unit weights zero.
    assert_eq!(view.positions(1).len(), 9);
    assert!(view.skin_weights(1).iter().all(|&w| w == 0.0));
    // Out-of-range mesh: empty, never a panic.
    assert!(view.positions(9).is_empty());
}

#[test]
fn texture_mime_sniffs_magic_and_guesses_tga() {
    use super::texture_mime;
    assert_eq!(texture_mime(&PNG_STUB, [None, None]), Some("image/png"));
    assert_eq!(
        texture_mime(&[0xFF, 0xD8, 0xFF], [None, None]),
        Some("image/jpeg")
    );
    assert_eq!(
        texture_mime(&[0, 0, 2], [Some("a/b.TGA"), None]),
        Some("image/x-tga")
    );
    assert_eq!(texture_mime(&[], [Some("x.bmp"), None]), None);
}
