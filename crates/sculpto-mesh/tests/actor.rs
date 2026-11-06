mod common;

use common::Fixture;
use sculpto_mesh::actor::{Actor, ActorError, Pose};
use serde_json::json;

fn near(actual: [f64; 3], expected: [f64; 3]) {
    actual
        .into_iter()
        .zip(expected)
        .for_each(|(a, b)| assert!((a - b).abs() < 1e-12, "{a} != {b}"));
}

#[test]
fn custom_vertex_identity_does_not_change_sampling_or_admit_invalid_semantics() {
    let baseline = Actor::from_glb(&Fixture::triangle().bytes())
        .unwrap()
        .sample(&["Body"], Pose::Rest)
        .unwrap();
    ["_SCULPTO_VERTEX", "UNKNOWN_SEMANTIC"]
        .into_iter()
        .for_each(|semantic| {
            let mut fixture = Fixture::triangle();
            let identity = fixture.floats(&[7., 11., 19.], "SCALAR", 3);
            fixture.json["meshes"][0]["primitives"][0]["attributes"][semantic] = json!(identity);
            let actor = Actor::from_glb(&fixture.bytes());
            match semantic {
                "_SCULPTO_VERTEX" => {
                    assert_eq!(
                        actor.unwrap().sample(&["Body"], Pose::Rest).unwrap(),
                        baseline
                    )
                }
                _ => assert!(actor.is_err()),
            }
        });
}

#[test]
fn static_hierarchy_and_primitive_ranges_preserve_selection_order() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"translation": [2., 3., 4.], "children": [1]},
        {"name": "Body", "mesh": 0, "matrix": [2.,0.,0.,0., 0.,3.,0.,0., 0.,0.,4.,0., 1.,2.,3.,1.]},
        {"name": "Other", "mesh": 0, "translation": [-2., 0., 0.]},
    ]);
    let indices = fixture.accessor(&[2, 1, 0], 5121, "SCALAR", 3);
    fixture.json["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(json!({"attributes": {"POSITION": 0}, "indices": indices}));
    let sample = Actor::from_glb(&fixture.bytes())
        .unwrap()
        .sample(&["Other", "Body"], Pose::Rest)
        .unwrap();
    assert_eq!(sample.parts.len(), 4);
    near(sample.vertices[0], [-1., 0., 0.]);
    near(sample.vertices[6], [5., 5., 7.]);
    near(sample.vertices[7], [3., 8., 7.]);
    assert_eq!(
        sample.triangles,
        [[0, 1, 2], [5, 4, 3], [6, 7, 8], [11, 10, 9]]
    );
    assert_eq!(sample.parts[2].node, 1);
    assert_eq!(sample.parts[2].mesh, 0);
    assert_eq!(sample.parts[2].primitive, 0);
    assert_eq!(sample.parts[2].vertices, 6..9);
    assert_eq!(sample.parts[2].triangles, 2..3);
}

#[test]
fn skin_world_hierarchy_inverse_binds_and_optional_identity() {
    let mut fixture = Fixture::triangle();
    fixture.skin();
    let identity = Actor::from_glb(&fixture.bytes())
        .unwrap()
        .sample(&["Body"], Pose::Rest)
        .unwrap();
    near(identity.vertices[0], [11., 2., 2.25]);
    let mut fixture = Fixture::triangle();
    fixture.skin();
    let mut matrices = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., -1., 0., 1.,
    ]
    .repeat(2);
    matrices[16 + 14] = -2.;
    let inverse = fixture.floats(&matrices, "MAT4", 2);
    fixture.json["skins"][0]["inverseBindMatrices"] = json!(inverse);
    let sample = Actor::from_glb(&fixture.bytes())
        .unwrap()
        .sample(&["Body"], Pose::Rest)
        .unwrap();
    near(sample.vertices[0], [11., 1., 0.75]);
}

#[test]
fn rest_is_asset_defaults_and_rotation_is_exact_quarter_interval_slerp() {
    [1., -1.].into_iter().for_each(|sign| {
        let mut fixture = Fixture::triangle();
        fixture.json["nodes"][0]["translation"] = json!([7., 8., 9.]);
        fixture.channel(0, "translation", &[0., 1.], &[0., 0., 0., 0., 0., 0.]);
        fixture.channel(
            0,
            "rotation",
            &[0., 1.],
            &[
                0.,
                0.,
                0.,
                1.,
                0.,
                0.,
                sign * (3_f32.sqrt() / 2.),
                sign * 0.5,
            ],
        );
        let actor = Actor::from_glb(&fixture.bytes()).unwrap();
        near(
            actor.sample(&["Body"], Pose::Rest).unwrap().vertices[0],
            [8., 8., 9.],
        );
        near(
            actor
                .sample(
                    &["Body"],
                    Pose::Clip {
                        name: "Run",
                        time: 0.,
                    },
                )
                .unwrap()
                .vertices[0],
            [1., 0., 0.],
        );
        let endpoint = (3_f32.sqrt() / 2.) as f64;
        let angle = endpoint.atan2(0.5) * 0.5;
        near(
            actor
                .sample(
                    &["Body"],
                    Pose::Clip {
                        name: "Run",
                        time: 0.25,
                    },
                )
                .unwrap()
                .vertices[0],
            [angle.cos(), angle.sin(), 0.],
        );
    });
}

#[test]
fn shorter_channels_clamp_and_single_key_channels_hold() {
    let mut fixture = Fixture::triangle();
    fixture.channel(0, "translation", &[0.25, 0.5], &[2., 0., 0., 4., 0., 0.]);
    fixture.channel(0, "scale", &[0., 1.], &[1., 1., 1., 2., 2., 2.]);
    fixture.channel(0, "rotation", &[0.5], &[0., 0., 0., 1.]);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    near(
        actor
            .sample(
                &["Body"],
                Pose::Clip {
                    name: "Run",
                    time: 0.,
                },
            )
            .unwrap()
            .vertices[0],
        [3., 0., 0.],
    );
    near(
        actor
            .sample(
                &["Body"],
                Pose::Clip {
                    name: "Run",
                    time: 1.,
                },
            )
            .unwrap()
            .vertices[0],
        [6., 0., 0.],
    );
}

#[test]
fn normalized_unsigned_weights_and_interleaved_accessors_are_f64() {
    [5121, 5123].into_iter().for_each(|component| {
        let mut fixture = Fixture::triangle();
        fixture.skin();
        let mut bytes = Vec::new();
        [ [1_f32,0.,0.], [0.,1.,0.], [0.,0.,1.] ].into_iter().for_each(|point| {
            bytes.extend(point.into_iter().flat_map(f32::to_le_bytes));
            bytes.extend([0,1,0,0]);
            match component {
                5121 => { bytes.extend([64,191,0,0]); bytes.extend([0;12]); }
                _ => { bytes.extend([16384_u16,49151,0,0].into_iter().flat_map(u16::to_le_bytes)); bytes.extend([0;8]); }
            }
        });
        let positions = fixture.accessor(&bytes, 5126, "VEC3", 3);
        let view = fixture.json["accessors"][positions]["bufferView"].as_u64().unwrap();
        fixture.json["bufferViews"][view as usize]["byteStride"] = json!(32);
        fixture.json["accessors"][positions]["min"] = json!([0.,0.,0.]);
        fixture.json["accessors"][positions]["max"] = json!([1.,1.,1.]);
        let joints = fixture.json["accessors"].as_array().unwrap().len();
        fixture.json["accessors"].as_array_mut().unwrap().extend([
            json!({"bufferView":view,"byteOffset":12,"componentType":5121,"type":"VEC4","count":3}),
            json!({"bufferView":view,"byteOffset":16,"componentType":component,"normalized":true,"type":"VEC4","count":3}),
        ]);
        fixture.json["meshes"][0]["primitives"][0]["attributes"] = json!({"POSITION":positions,"JOINTS_0":joints,"WEIGHTS_0":joints+1});
        let sample = Actor::from_glb(&fixture.bytes()).unwrap().sample(&["Body"], Pose::Rest).unwrap();
        let weight = if component == 5121 {191./255.} else {49151./65535.};
        near(sample.vertices[0], [11.,2.,3.*weight]);
    });
}

#[test]
fn unselected_cloth_weight_channels_do_not_block_solids() {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"Cloth","mesh":1}));
    fixture.json["meshes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"primitives":[{"attributes":{"POSITION":0},"targets":[{"POSITION":0}]}]}));
    fixture.channel(0, "translation", &[0., 1.], &[0., 0., 0., 1., 0., 0.]);
    fixture.channel(1, "weights", &[0., 1.], &[0., 0., 0., 1., 1., 1.]);
    let output = fixture.json["animations"][0]["samplers"][1]["output"]
        .as_u64()
        .unwrap() as usize;
    fixture.json["accessors"][output]["type"] = json!("SCALAR");
    fixture.json["accessors"][output]["count"] = json!(6);
    fixture.json["animations"][0]["samplers"][1]["interpolation"] = json!("CUBICSPLINE");
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    near(
        actor
            .sample(
                &["Body"],
                Pose::Clip {
                    name: "Run",
                    time: 0.5,
                },
            )
            .unwrap()
            .vertices[0],
        [1.5, 0., 0.],
    );
    assert!(matches!(
        actor.sample(&["Cloth"], Pose::Rest),
        Err(ActorError::Unsupported { .. })
    ));
}

#[test]
fn selection_clip_and_time_are_explicit_errors() {
    let mut fixture = Fixture::triangle();
    fixture.channel(0, "translation", &[0., 1.], &[0., 0., 0., 1., 0., 0.]);
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    assert!(matches!(
        actor.sample(&[], Pose::Rest),
        Err(ActorError::Invalid { .. })
    ));
    assert!(matches!(
        actor.sample(&["missing"], Pose::Rest),
        Err(ActorError::Selection { matches: 0, .. })
    ));
    assert!(matches!(
        actor.sample(&["Body", "Body"], Pose::Rest),
        Err(ActorError::DuplicatePart { .. })
    ));
    assert!(matches!(
        actor.sample(
            &["Body"],
            Pose::Clip {
                name: "missing",
                time: 0.
            }
        ),
        Err(ActorError::Clip { matches: 0, .. })
    ));
    [f64::NAN, f64::INFINITY, -0.01, 1.01]
        .into_iter()
        .for_each(|time| {
            assert!(matches!(
                actor.sample(&["Body"], Pose::Clip { name: "Run", time }),
                Err(ActorError::Time { .. })
            ));
        });
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"Body","mesh":0}));
    assert!(matches!(
        Actor::from_glb(&fixture.bytes())
            .unwrap()
            .sample(&["Body"], Pose::Rest),
        Err(ActorError::Selection { matches: 2, .. })
    ));
}

#[test]
fn malformed_headers_graphs_and_accessor_ranges_return_errors() {
    (0..20).for_each(|length| {
        let mut bytes = vec![0; 20];
        bytes[..4].copy_from_slice(b"glTF");
        bytes[4..8].copy_from_slice(&2_u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&(length as u32).to_le_bytes());
        assert!(Actor::from_glb(&bytes).is_err());
    });
    [
        json!([{"children":[1]},{"children":[0]}]),
        json!([{"children":[2]},{"children":[2]},{}]),
    ]
    .into_iter()
    .for_each(|nodes| {
        let mut fixture = Fixture::triangle();
        fixture.json["nodes"] = nodes;
        assert!(matches!(
            Actor::from_glb(&fixture.bytes()),
            Err(ActorError::Invalid { .. })
        ));
    });
    [0, 4, usize::MAX].into_iter().for_each(|count| {
        let mut fixture = Fixture::triangle();
        fixture.json["accessors"][0]["count"] = json!(count);
        let result =
            Actor::from_glb(&fixture.bytes()).and_then(|actor| actor.sample(&["Body"], Pose::Rest));
        assert!(result.is_err());
    });
    let mut fixture = Fixture::triangle();
    fixture.json["bufferViews"][0]["byteLength"] = json!(999);
    assert!(Actor::from_glb(&fixture.bytes()).is_err());
}

#[test]
fn invalid_skin_indices_weights_and_selected_features_are_rejected() {
    (0..7).for_each(|case| {
        let mut fixture = Fixture::triangle();
        fixture.skin();
        let attributes = fixture.json["meshes"][0]["primitives"][0]["attributes"].clone();
        match case {
            0 => {
                fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(0);
            }
            1 => {
                let i = attributes["JOINTS_0"].as_u64().unwrap() as usize;
                let view = fixture.json["accessors"][i]["bufferView"].as_u64().unwrap() as usize;
                let offset = fixture.json["bufferViews"][view]["byteOffset"]
                    .as_u64()
                    .unwrap() as usize;
                fixture.bin[offset] = 99;
            }
            2 => {
                let i = attributes["WEIGHTS_0"].as_u64().unwrap() as usize;
                let view = fixture.json["accessors"][i]["bufferView"].as_u64().unwrap() as usize;
                let offset = fixture.json["bufferViews"][view]["byteOffset"]
                    .as_u64()
                    .unwrap() as usize;
                fixture.bin[offset..offset + 4].copy_from_slice(&0.5_f32.to_le_bytes());
            }
            3 => {
                fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_1"] =
                    attributes["JOINTS_0"].clone();
            }
            4 => {
                fixture.json["meshes"][0]["primitives"][0]["mode"] = json!(1);
            }
            5 => {
                fixture.json["nodes"][1]["rotation"] = json!([0., 0., 0., 0.]);
            }
            _ => {
                let i = fixture.accessor(&[0, 1, 99], 5121, "SCALAR", 3);
                fixture.json["meshes"][0]["primitives"][0]["indices"] = json!(i);
            }
        }
        assert!(
            Actor::from_glb(&fixture.bytes())
                .and_then(|actor| actor.sample(&["Body"], Pose::Rest))
                .is_err(),
            "case {case}"
        );
    });
}

#[test]
fn step_interpolation_holds_the_lower_key_until_the_next_one() {
    let mut fixture = Fixture::triangle();
    fixture.channel(0, "translation", &[0.25, 0.75], &[2., 0., 0., 4., 0., 0.]);
    fixture.json["animations"][0]["samplers"][0]["interpolation"] = json!("STEP");
    let actor = Actor::from_glb(&fixture.bytes()).unwrap();
    for (time, expected) in [
        (0.3, [3., 0., 0.]),
        (0.5, [3., 0., 0.]),
        (0.75, [5., 0., 0.]),
        (0.7, [3., 0., 0.]),
    ] {
        near(
            actor
                .sample(&["Body"], Pose::Clip { name: "Run", time })
                .unwrap()
                .vertices[0],
            expected,
        );
    }
}

#[test]
fn duplicate_channels_invalid_times_and_unsupported_interpolation_fail() {
    (0..5).for_each(|case| {
        let mut fixture = Fixture::triangle();
        fixture.channel(0, "translation", &[0., 1.], &[0., 0., 0., 1., 0., 0.]);
        match case {
            0 => {
                let channel = fixture.json["animations"][0]["channels"][0].clone();
                fixture.json["animations"][0]["channels"]
                    .as_array_mut()
                    .unwrap()
                    .push(channel);
            }
            1 => {
                fixture.json["animations"][0]["samplers"][0]["interpolation"] = json!("CUBICSPLINE")
            }
            2 => {
                let i = fixture.json["animations"][0]["samplers"][0]["input"]
                    .as_u64()
                    .unwrap() as usize;
                let view = fixture.json["accessors"][i]["bufferView"].as_u64().unwrap() as usize;
                let offset = fixture.json["bufferViews"][view]["byteOffset"]
                    .as_u64()
                    .unwrap() as usize;
                fixture.bin[offset + 4..offset + 8].copy_from_slice(&0_f32.to_le_bytes());
            }
            3 => {
                fixture.json["nodes"][0]["matrix"] = json!([
                    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.
                ]);
            }
            _ => {
                fixture.channel(0, "rotation", &[0., 1.], &[0.; 8]);
            }
        }
        let result = Actor::from_glb(&fixture.bytes()).and_then(|actor| {
            actor.sample(
                &["Body"],
                Pose::Clip {
                    name: "Run",
                    time: 0.5,
                },
            )
        });
        assert!(result.is_err(), "case {case}");
    });
}

#[test]
fn missing_attributes_sparse_layouts_nonfinite_data_and_duplicate_names_fail() {
    (0..10).for_each(|case| {
        let mut fixture = Fixture::triangle();
        match case {
            0 => {
                fixture.json["meshes"][0]["primitives"][0]["attributes"] = json!({});
            }
            1 => {
                fixture.json["bufferViews"][0]["byteStride"] = json!(4);
            }
            2 => {
                let index = fixture.accessor(&[0], 5121, "SCALAR", 1);
                let value = fixture.floats(&[2., 3., 4.], "VEC3", 1);
                let index_view = fixture.json["accessors"][index]["bufferView"].clone();
                let value_view = fixture.json["accessors"][value]["bufferView"].clone();
                fixture.json["accessors"][0]["sparse"] = json!({
                    "count": 1, "indices": {"bufferView": index_view, "componentType": 5121},
                    "values": {"bufferView": value_view},
                });
            }
            3 => fixture.bin[..4].copy_from_slice(&f32::NAN.to_le_bytes()),
            4 => {
                fixture.skin();
                fixture.json["meshes"][0]["primitives"][0]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("WEIGHTS_0");
            }
            5 => {
                fixture.skin();
                let inverse = fixture.floats(&[1.; 16], "MAT4", 1);
                fixture.json["skins"][0]["inverseBindMatrices"] = json!(inverse);
            }
            6 => {
                fixture.json["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"name": "Body"}));
            }
            7 => {
                fixture.channel(0, "translation", &[0., 1.], &[0.; 6]);
                let clip = fixture.json["animations"][0].clone();
                fixture.json["animations"]
                    .as_array_mut()
                    .unwrap()
                    .push(clip);
            }
            8 => {
                fixture.channel(
                    0,
                    "rotation",
                    &[0., 1.],
                    &[0., 0., 0., 1., 0., 0., f32::INFINITY, 1.],
                );
            }
            _ => {
                fixture.json["meshes"][0]["primitives"] = json!([]);
            }
        }
        let pose = match case == 7 || case == 8 {
            true => Pose::Clip {
                name: "Run",
                time: 0.5,
            },
            false => Pose::Rest,
        };
        let result =
            Actor::from_glb(&fixture.bytes()).and_then(|actor| actor.sample(&["Body"], pose));
        assert!(result.is_err(), "case {case}");
    });
}
