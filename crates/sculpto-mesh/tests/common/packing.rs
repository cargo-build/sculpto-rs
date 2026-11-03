#[path = "mod.rs"]
#[expect(
    dead_code,
    reason = "shared sampler fixtures expose additional helpers"
)]
mod common;

use common::Fixture;
use serde_json::json;

fn cloth() -> Fixture {
    let mut fixture = Fixture::triangle();
    let normals = fixture.floats(&[0., 1., 0.].repeat(3), "VEC3", 3);
    let tangents = fixture.floats(&[1., 0., 0., 1.].repeat(3), "VEC4", 3);
    let uv = fixture.floats(&[0., 0., 1., 0., 0., 1.], "VEC2", 3);
    let identity = fixture.floats(&[7., 11., 19.], "SCALAR", 3);
    let indices = fixture.accessor(&[0, 1, 2], 5121, "SCALAR", 3);
    let morph = fixture.floats(&[0., 0., 0.25].repeat(3), "VEC3", 3);
    fixture.json["nodes"][0]["name"] = json!("Cloth");
    fixture.json["nodes"][0]["weights"] = json!([0.5, 0.5]);
    fixture.json["meshes"][0] = json!({
        "primitives": [{"attributes": {"POSITION": 0, "NORMAL": normals, "TANGENT": tangents, "TEXCOORD_0": uv, "_SCULPTO_VERTEX": identity},
            "indices": indices, "targets": [{"POSITION": morph, "NORMAL": morph, "TANGENT": morph}, {"POSITION": morph}], "extras": {"primitive_custom": true}}],
        "weights": [0.5, 0.5], "extras": {"targetNames": ["old0", "old1"], "keep": "metadata"},
    });
    fixture
}

pub fn pair() -> (Fixture, Fixture) {
    let mut base = cloth();
    let mut targets = cloth();
    let normal_offset = targets.json["bufferViews"][1]["byteOffset"]
        .as_u64()
        .unwrap() as usize;
    targets.bin[normal_offset..normal_offset + 12].copy_from_slice(
        &[1_f32, 0., 0.]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let positions = targets.floats(&[1., 0., 1., 0., 1., 1., 0., 0., 2.], "VEC3", 3);
    targets.json["accessors"][positions]["min"] = json!([0., 0., 1.]);
    targets.json["accessors"][positions]["max"] = json!([1., 1., 2.]);
    targets.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
    targets.json["meshes"][0]["extras"]["targetNames"] = json!(["baked0", "baked1"]);
    let joints = base.accessor(&[0; 12], 5121, "VEC4", 3);
    let weights = base.floats(&[1., 0., 0., 0.].repeat(3), "VEC4", 3);
    base.json["meshes"].as_array_mut().unwrap().push(json!({"primitives": [{"attributes": {"POSITION": 0, "JOINTS_0": joints, "WEIGHTS_0": weights}}]}));
    base.json["nodes"].as_array_mut().unwrap().extend([
        json!({"name": "Body", "mesh": 1, "skin": 0}),
        json!({"name": "Joint"}),
    ]);
    base.json["skins"] = json!([{"joints": [2]}]);
    base.json["scenes"] = json!([{"nodes": [0, 1, 2], "extras": {"keep": true}}]);
    base.json["scene"] = json!(0);
    base.json["materials"] = json!([{"name": "Original material", "extras": {"unknown": [1, 2]}}]);
    base.json["meshes"][0]["primitives"][0]["material"] = json!(0);
    base.json["unknownVendorField"] = json!({"deep": {"unrecognized": [1, "two", null]}});
    base.json["extensions"] = json!({"EXAMPLE_metadata": {"unrecognized": true}});
    base.channel(2, "translation", &[0., 1.], &[0., 0., 0., 1., 0., 0.]);
    let input = base.floats(&[0., 1.], "SCALAR", 2);
    let output = base.floats(&[1., 0., 0., 1.], "SCALAR", 4);
    base.json["animations"][0]["samplers"]
        .as_array_mut()
        .unwrap()
        .push(json!({"input": input, "output": output, "extras": {"keep": true}}));
    base.json["animations"][0]["channels"].as_array_mut().unwrap().push(json!({"sampler": 1, "target": {"node": 0, "path": "weights"}, "extras": {"keep": true}}));
    (base, targets)
}
