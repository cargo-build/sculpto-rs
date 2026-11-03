#[path = "common/packing.rs"]
mod fixtures;

use std::borrow::Cow;

use sculpto_mesh::actor::{Actor, ActorError, ClothPacking, Pose, pack_cloth};
use serde_json::{Value, json};

fn document(bytes: &[u8]) -> (Value, Vec<u8>) {
    let glb = gltf::binary::Glb::from_slice(bytes).unwrap();
    (
        serde_json::from_slice(&glb.json).unwrap(),
        glb.bin.unwrap().to_vec(),
    )
}

fn accessor(json: &Value, bin: &[u8], index: &Value) -> Vec<u8> {
    let accessor = &json["accessors"][index.as_u64().unwrap() as usize];
    let view = &json["bufferViews"][accessor["bufferView"].as_u64().unwrap() as usize];
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize
        + accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    let width = match accessor["type"].as_str().unwrap() {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => panic!("fixture type"),
    } * match accessor["componentType"].as_u64().unwrap() {
        5121 => 1,
        5123 => 2,
        _ => 4,
    };
    let stride = view["byteStride"]
        .as_u64()
        .map_or(width, |stride| stride as usize);
    (0..accessor["count"].as_u64().unwrap() as usize)
        .flat_map(|i| {
            bin[start + i * stride..start + i * stride + width]
                .iter()
                .copied()
        })
        .collect()
}

#[test]
fn replacement_preserves_actor_and_rebases_rest_geometry() {
    let (base, targets) = fixtures::pair();
    let before = base.bytes();
    let target = targets.bytes();
    let packed = pack_cloth(
        &before,
        &target,
        ClothPacking::new("Cloth", "Run", &[0., 1.]),
    )
    .unwrap();
    let (old, old_bin) = document(&before);
    let (expected, expected_bin) = document(&target);
    let (result, bin) = document(&packed.glb);
    assert_eq!(&bin[..old_bin.len()], old_bin);
    [
        "asset",
        "nodes",
        "scenes",
        "scene",
        "skins",
        "materials",
        "unknownVendorField",
        "extensions",
    ]
    .into_iter()
    .for_each(|field| {
        if field != "nodes" {
            assert_eq!(result[field], old[field], "{field}");
        }
    });
    assert_eq!(result["nodes"][1], old["nodes"][1]);
    assert_eq!(result["nodes"][0]["weights"], json!([0., 0.]));
    assert_eq!(result["meshes"][1], old["meshes"][1]);
    assert_eq!(
        result["meshes"][0]["extras"]["keep"],
        old["meshes"][0]["extras"]["keep"]
    );
    assert_eq!(
        result["meshes"][0]["extras"]["targetNames"],
        json!(["baked0", "baked1"])
    );
    assert_eq!(result["meshes"][0]["weights"], json!([0., 0.]));
    let primitive = &result["meshes"][0]["primitives"][0];
    let original = &old["meshes"][0]["primitives"][0];
    let target = &expected["meshes"][0]["primitives"][0];
    ["POSITION", "NORMAL", "TANGENT"]
        .into_iter()
        .for_each(|name| {
            assert_eq!(
                accessor(&result, &bin, &primitive["attributes"][name]),
                accessor(&expected, &expected_bin, &target["attributes"][name])
            )
        });
    assert_ne!(
        accessor(&result, &bin, &primitive["attributes"]["POSITION"]),
        accessor(&old, &old_bin, &original["attributes"]["POSITION"])
    );
    ["indices", "material", "extras"]
        .into_iter()
        .for_each(|field| assert_eq!(primitive[field], original[field]));
    assert_eq!(
        primitive["attributes"]["TEXCOORD_0"],
        original["attributes"]["TEXCOORD_0"]
    );
    assert_eq!(
        result["animations"][0]["channels"],
        old["animations"][0]["channels"]
    );
    assert_eq!(
        result["animations"][0]["samplers"][0],
        old["animations"][0]["samplers"][0]
    );
    assert_eq!(
        result["animations"][0]["samplers"][1]["extras"],
        json!({"keep": true})
    );
    let sampler = &result["animations"][0]["samplers"][1];
    assert_eq!(
        accessor(&result, &bin, &sampler["output"]),
        [1_f32, 0., 0., 1.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>()
    );
    let before = Actor::from_glb(&before).unwrap();
    let after = Actor::from_glb(&packed.glb).unwrap();
    [0., 0.5, 1.].into_iter().for_each(|time| {
        assert_eq!(
            before
                .sample(&["Body"], Pose::Clip { name: "Run", time })
                .unwrap(),
            after
                .sample(&["Body"], Pose::Clip { name: "Run", time })
                .unwrap()
        )
    });
}

#[test]
fn new_channel_multiple_primitives_and_optional_names() {
    let (mut base, mut target) = fixtures::pair();
    base.json["animations"][0]["channels"]
        .as_array_mut()
        .unwrap()
        .pop();
    base.json["animations"][0]["samplers"]
        .as_array_mut()
        .unwrap()
        .pop();
    target.json["meshes"][0]["extras"]
        .as_object_mut()
        .unwrap()
        .remove("targetNames");
    [&mut base, &mut target].into_iter().for_each(|fixture| {
        let second = fixture.json["meshes"][0]["primitives"][0].clone();
        fixture.json["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .push(second);
    });
    let packed = pack_cloth(
        &base.bytes(),
        &target.bytes(),
        ClothPacking::new("Cloth", "Run", &[0., 1.]),
    )
    .unwrap();
    let (json, _) = document(&packed.glb);
    assert_eq!(packed.primitives, 2);
    assert_eq!(
        json["animations"][0]["channels"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        json["animations"][0]["channels"][1]["target"],
        json!({"node": 0, "path": "weights"})
    );
    assert!(json["meshes"][0]["extras"].get("targetNames").is_none());
    assert_eq!(json["meshes"][0]["extras"]["keep"], "metadata");
    assert_eq!(
        json["meshes"][0]["primitives"][0]["targets"],
        json["meshes"][0]["primitives"][1]["targets"]
    );
}

#[test]
fn interleaved_geometry_and_matching_static_ancestry_preserve_frames() {
    let (mut base, mut target) = fixtures::pair();
    base.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"translation": [2., 3., 4.], "children": [0]}));
    base.json["scenes"][0]["nodes"] = json!([3, 1, 2]);
    target.json["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"translation": [2., 3., 4.], "children": [0]}));
    let position = target.floats(
        &[
            1., 0., 1., 0., 1., 0., 0., 1., 1., 0., 1., 0., 0., 0., 2., 0., 1., 0.,
        ],
        "VEC3",
        3,
    );
    let view = target.json["accessors"][position]["bufferView"]
        .as_u64()
        .unwrap() as usize;
    target.json["bufferViews"][view]["byteStride"] = json!(24);
    target.json["accessors"][position]["min"] = json!([0., 0., 1.]);
    target.json["accessors"][position]["max"] = json!([1., 1., 2.]);
    let mut normal = target.json["accessors"][position].clone();
    normal["byteOffset"] = json!(12);
    normal.as_object_mut().unwrap().remove("min");
    normal.as_object_mut().unwrap().remove("max");
    let normal_index = target.json["accessors"].as_array().unwrap().len();
    target.json["accessors"]
        .as_array_mut()
        .unwrap()
        .push(normal);
    target.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(position);
    target.json["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal_index);
    let target = target.bytes();
    let packed = pack_cloth(
        &base.bytes(),
        &target,
        ClothPacking::new("Cloth", "Run", &[0., 1.]),
    )
    .unwrap();
    let (json, bin) = document(&packed.glb);
    let (expected, expected_bin) = document(&target);
    let attributes = &json["meshes"][0]["primitives"][0]["attributes"];
    ["POSITION", "NORMAL"].into_iter().for_each(|name| {
        assert_eq!(
            accessor(&json, &bin, &attributes[name]),
            accessor(
                &expected,
                &expected_bin,
                &expected["meshes"][0]["primitives"][0]["attributes"][name]
            )
        )
    });
    let p = attributes["POSITION"].as_u64().unwrap() as usize;
    let n = attributes["NORMAL"].as_u64().unwrap() as usize;
    assert_eq!(
        json["accessors"][p]["bufferView"],
        json["accessors"][n]["bufferView"]
    );
    assert_eq!(
        json["nodes"][3],
        json!({"translation": [2., 3., 4.], "children": [0]})
    );
}

#[test]
fn timing_validation_includes_f32_collapse_and_existing_endpoints() {
    let (base, target) = fixtures::pair();
    let (base, target) = (base.bytes(), target.bytes());
    [
        vec![],
        vec![0.],
        vec![0., f64::NAN],
        vec![-1., 1.],
        vec![0., 0.],
        vec![0., f64::INFINITY],
        vec![0., 2.],
        vec![0.1, 1.],
        vec![1., 1. + 1e-10],
        vec![0.; 257],
    ]
    .into_iter()
    .for_each(|times| {
        assert!(
            pack_cloth(&base, &target, ClothPacking::new("Cloth", "Run", &times)).is_err(),
            "{times:?}"
        );
    });
}

#[test]
fn rejects_correspondence_ranges_frames_and_animation_aliases() {
    (0..20).for_each(|case| {
        let (mut base, mut target) = fixtures::pair();
        match case {
            0 => target.json["accessors"][3]["count"] = json!(2),
            1 => { let offset = target.json["bufferViews"][6]["byteOffset"].as_u64().unwrap() as usize; target.bin[offset..offset + 4].copy_from_slice(&f32::NAN.to_le_bytes()); }
            2 => target.json["accessors"][6]["byteOffset"] = json!(u64::MAX),
            3 => target.json["meshes"][0]["primitives"][0]["targets"].as_array_mut().unwrap().pop().map(|_| ()).unwrap(),
            4 => target.json["nodes"][0]["translation"] = json!([1., 0., 0.]),
            5 => base.json["nodes"].as_array_mut().unwrap().push(json!({"name": "Cloth", "mesh": 0})),
            6 => base.json["nodes"].as_array_mut().unwrap().push(json!({"name": "Another", "mesh": 0})),
            7 => { let clip = base.json["animations"][0].clone(); base.json["animations"].as_array_mut().unwrap().push(clip); base.json["animations"][1]["name"] = json!("Other"); }
            8 => base.json["animations"][0]["channels"][0]["sampler"] = json!(1),
            9 => base.json["animations"][0]["channels"][0]["target"]["node"] = json!(0),
            10 => base.json["nodes"][0]["skin"] = json!(0),
            11 => target.json["accessors"][1]["extensions"] = json!({"EXAMPLE_layout": {}}),
            12 => target.json["meshes"][0]["extras"]["targetNames"] = json!(["missing"]),
            13 => { target.json["accessors"][6]["count"] = json!(2); }
            14 => { let view = target.json["accessors"][3]["bufferView"].as_u64().unwrap() as usize; let offset = target.json["bufferViews"][view]["byteOffset"].as_u64().unwrap() as usize; target.bin[offset..offset + 4].copy_from_slice(&0.5_f32.to_le_bytes()); }
            15 => { let offset = target.json["bufferViews"][4]["byteOffset"].as_u64().unwrap() as usize; target.bin[offset] = 2; }
            16 => target.json["bufferViews"][0]["byteLength"] = json!(u64::MAX),
            17 => { base.json["nodes"][2]["children"] = json!([0]); }
            18 => target.json["accessors"][5]["count"] = json!(2),
            _ => { let index = target.accessor(&[0], 5121, "SCALAR", 1); target.json["accessors"][6]["sparse"] = json!({"count": 1, "indices": {"bufferView": target.json["accessors"][index]["bufferView"], "componentType": 5121}, "values": {"bufferView": 0}}); }
        }
        let result = pack_cloth(&base.bytes(), &target.bytes(), ClothPacking::new("Cloth", "Run", &[0., 1.]));
        assert!(result.is_err(), "case {case}");
    });
}

#[test]
fn invalid_framing_does_not_panic() {
    let (_, target) = fixtures::pair();
    assert!(
        pack_cloth(
            &[],
            &target.bytes(),
            ClothPacking::new("Cloth", "Run", &[0., 1.])
        )
        .is_err()
    );
    let raw = gltf::binary::Glb {
        header: gltf::binary::Header {
            magic: *b"glTF",
            version: 2,
            length: 0,
        },
        json: Cow::Borrowed(b"{}"),
        bin: Some(Cow::Borrowed(&[])),
    }
    .to_vec()
    .unwrap();
    assert!(pack_cloth(&raw, &raw, ClothPacking::new("Cloth", "Run", &[0., 1.])).is_err());
}

#[test]
fn untouched_json_retains_exact_decimal_metadata() {
    let (mut base, target) = fixtures::pair();
    base.json["precision"] = json!({"decimal": "REPLACE_DECIMAL", "huge": "REPLACE_INTEGER"});
    let input = base.bytes();
    let mut glb = gltf::binary::Glb::from_slice(&input).unwrap();
    let decimal = "0.09553151577711105";
    let integer = "9999999999999999999999999999999999999999999999999";
    glb.json = Cow::Owned(
        String::from_utf8(glb.json.to_vec())
            .unwrap()
            .replace("\"REPLACE_DECIMAL\"", decimal)
            .replace("\"REPLACE_INTEGER\"", integer)
            .into_bytes(),
    );
    let packed = pack_cloth(
        &glb.to_vec().unwrap(),
        &target.bytes(),
        ClothPacking::new("Cloth", "Run", &[0., 1.]),
    )
    .unwrap();
    let output = gltf::binary::Glb::from_slice(&packed.glb).unwrap();
    let json = String::from_utf8(output.json.to_vec()).unwrap();
    assert!(json.contains(decimal), "{json}");
    assert!(json.contains(integer), "{json}");
}

#[test]
fn base_external_image_uris_are_contextual_unsupported_errors() {
    ["fabric.png", "../textures/fabric.png", "/tmp/fabric.png", "https://example.invalid/fabric.png", "file:///tmp/fabric.png", ""].into_iter().for_each(|uri| {
        let (mut base, targets) = fixtures::pair();
        base.json["images"] = json!([{"uri": uri}]);
        let error = pack_cloth(&base.bytes(), &targets.bytes(), ClothPacking::new("Cloth", "Run", &[0., 1.])).unwrap_err();
        assert!(matches!(error, ActorError::Unsupported { ref context, ref feature } if context == "base image 0" && feature.contains("external image URI")), "{error}");
    });
}

#[test]
fn embedded_and_data_uri_base_images_survive_and_target_images_are_unused() {
    let (mut base, mut targets) = fixtures::pair();
    let image = base.accessor(&[137, 80, 78, 71], 5121, "SCALAR", 4);
    let view = base.json["accessors"][image]["bufferView"].clone();
    base.json["images"] = json!([
        {"bufferView": view, "mimeType": "image/png", "extras": {"keep": "embedded"}},
        {"uri": "data:image/png;base64,iVBORw==", "extras": {"keep": "data"}},
        {"uri": "DATA:image/png;base64,iVBORw=="},
    ]);
    targets.json["images"] = json!([{"uri": "unused-target-texture.png"}]);
    let before = base.bytes();
    let packed = pack_cloth(
        &before,
        &targets.bytes(),
        ClothPacking::new("Cloth", "Run", &[0., 1.]),
    )
    .unwrap();
    let (original, original_bin) = document(&before);
    let (result, result_bin) = document(&packed.glb);
    assert_eq!(result["images"], original["images"]);
    assert_eq!(&result_bin[..original_bin.len()], original_bin);
}
