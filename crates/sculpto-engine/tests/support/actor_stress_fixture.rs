//! The synthetic skinned body + shell pair shared by the stress tests.
//!
//! Body cube centred on the origin; shell cube of half-size 0.3 centred at
//! the origin and skinned to its own joint, whose authored rest translation
//! parks it at z = 3 (clear of the body); `Walk` moves the joint to the
//! origin between t=0 and t=1 (fully inside). Frame 0 and the rest pose are
//! clear, frame 1 contains the shell.

use serde_json::json;

use crate::common::Fixture;

/// Outward-wound cube: 8 vertices, 12 triangles.
const CUBE_VERTICES: [f32; 24] = [
    -1.0, -1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0, //
    -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
];
const CUBE_INDICES: [u16; 36] = [
    4, 5, 6, 4, 6, 7, // z+
    1, 0, 3, 1, 3, 2, // z-
    5, 1, 2, 5, 2, 6, // x+
    0, 4, 7, 0, 7, 3, // x-
    3, 7, 6, 3, 6, 2, // y+
    4, 0, 1, 4, 1, 5, // y-
];

/// Append one scaled, offset outward-wound cube: `(POSITION, indices)` accessor ids.
fn cube(fixture: &mut Fixture, scale: f32, offset: [f32; 3]) -> (usize, usize) {
    let values: Vec<f32> = CUBE_VERTICES
        .iter()
        .enumerate()
        .map(|(index, value)| value * scale + offset[index % 3])
        .collect();
    let positions = fixture.floats(&values, "VEC3", 8);
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for corner in values.chunks(3) {
        for (axis, value) in corner.iter().enumerate() {
            min[axis] = min[axis].min(*value);
            max[axis] = max[axis].max(*value);
        }
    }
    fixture.json["accessors"][positions]["min"] = json!(min);
    fixture.json["accessors"][positions]["max"] = json!(max);
    let indices = fixture.accessor(
        &CUBE_INDICES
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<u8>>(),
        5123,
        "SCALAR",
        36,
    );
    (positions, indices)
}

/// Bind every vertex of `mesh` to one joint with weight 1.
fn skinned(fixture: &mut Fixture, mesh: usize, joint: u8) {
    let joints = fixture.accessor(&[joint, 0, 0, 0].repeat(8), 5121, "VEC4", 8);
    let weights = fixture.floats(&[1.0, 0.0, 0.0, 0.0].repeat(8), "VEC4", 8);
    fixture.json["meshes"][mesh]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][mesh]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
}

/// The GLB bytes of the pair.
pub fn fixture() -> Vec<u8> {
    let mut fixture = Fixture {
        json: json!({
            "asset": {"version": "2.0"}, "buffers": [{"byteLength": 0}],
            "bufferViews": [], "accessors": [],
            "nodes": [
                {"name": "BodyJoint", "translation": [0., 0., 0.]},
                {"name": "ShellJoint", "translation": [0., 0., 3.]},
                {"name": "Body", "mesh": 0, "skin": 0},
                {"name": "Shell", "mesh": 1, "skin": 0},
            ],
            "meshes": [
                {"primitives": [{"attributes": {}}]},
                {"primitives": [{"attributes": {}}]},
            ],
            "skins": [{"joints": [0, 1]}],
            "animations": [{"name": "Walk", "channels": [], "samplers": []}],
        }),
        bin: Vec::new(),
    };
    let (body_positions, body_indices) = cube(&mut fixture, 1.0, [0.0; 3]);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(body_positions);
    fixture.json["meshes"][0]["primitives"][0]["indices"] = json!(body_indices);
    skinned(&mut fixture, 0, 0);
    let (shell_positions, shell_indices) = cube(&mut fixture, 0.3, [0.0, 0.0, 0.0]);
    fixture.json["meshes"][1]["primitives"][0]["attributes"]["POSITION"] = json!(shell_positions);
    fixture.json["meshes"][1]["primitives"][0]["indices"] = json!(shell_indices);
    skinned(&mut fixture, 1, 1);
    fixture.channel(
        1,
        "translation",
        &[0.0, 1.0],
        &[0.0, 0.0, 3.0, 0.0, 0.0, 0.0],
    );
    fixture.bytes()
}
