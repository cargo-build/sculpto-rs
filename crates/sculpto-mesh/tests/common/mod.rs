use std::borrow::Cow;

use serde_json::{Value, json};

pub struct Fixture {
    pub json: Value,
    pub bin: Vec<u8>,
}

impl Fixture {
    pub fn triangle() -> Self {
        let mut fixture = Self {
            json: json!({
                "asset": {"version": "2.0"}, "buffers": [{"byteLength": 0}],
                "bufferViews": [], "accessors": [],
                "nodes": [{"name": "Body", "mesh": 0}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            }),
            bin: Vec::new(),
        };
        let positions = fixture.floats(&[1., 0., 0., 0., 1., 0., 0., 0., 1.], "VEC3", 3);
        fixture.json["accessors"][positions]["min"] = json!([0., 0., 0.]);
        fixture.json["accessors"][positions]["max"] = json!([1., 1., 1.]);
        fixture
    }

    pub fn accessor(
        &mut self,
        bytes: &[u8],
        component: u32,
        dimensions: &str,
        count: usize,
    ) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend(bytes);
        let views = self.json["bufferViews"].as_array_mut().unwrap();
        let view = views.len();
        views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": bytes.len()}));
        let accessors = self.json["accessors"].as_array_mut().unwrap();
        let index = accessors.len();
        accessors.push(json!({"bufferView": view, "componentType": component, "type": dimensions, "count": count}));
        index
    }

    pub fn floats(&mut self, values: &[f32], dimensions: &str, count: usize) -> usize {
        let bytes: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.accessor(&bytes, 5126, dimensions, count)
    }

    pub fn skin(&mut self) {
        self.json["nodes"] = json!([
            {"translation": [10., 0., 0.], "children": [1, 2]},
            {"translation": [0., 2., 0.], "children": [3]},
            {"name": "Body", "mesh": 0, "skin": 0, "translation": [99., 0., 0.]},
            {"translation": [0., 0., 3.]},
        ]);
        self.json["skins"] = json!([{"joints": [1, 3]}]);
        let joints = self.accessor(&[0, 1, 0, 0].repeat(3), 5121, "VEC4", 3);
        let weights = self.floats(&[0.25, 0.75, 0., 0.].repeat(3), "VEC4", 3);
        self.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
        self.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    }

    pub fn channel(&mut self, node: usize, property: &str, times: &[f32], values: &[f32]) {
        let input = self.floats(times, "SCALAR", times.len());
        let components = if property == "rotation" { 4 } else { 3 };
        let output = self.floats(
            values,
            if components == 4 { "VEC4" } else { "VEC3" },
            values.len() / components,
        );
        if self.json.get("animations").is_none() {
            self.json["animations"] = json!([{"name": "Run", "channels": [], "samplers": []}]);
        }
        let samplers = self.json["animations"][0]["samplers"]
            .as_array_mut()
            .unwrap();
        let sampler = samplers.len();
        samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
        self.json["animations"][0]["channels"]
            .as_array_mut()
            .unwrap()
            .push(json!({"sampler": sampler, "target": {"node": node, "path": property}}));
    }

    pub fn bytes(mut self) -> Vec<u8> {
        self.json["buffers"][0]["byteLength"] = json!(self.bin.len());
        gltf::binary::Glb {
            header: gltf::binary::Header {
                magic: *b"glTF",
                version: 2,
                length: 0,
            },
            json: Cow::Owned(serde_json::to_vec(&self.json).unwrap()),
            bin: Some(Cow::Owned(self.bin)),
        }
        .to_vec()
        .unwrap()
    }
}
