//! The five garment rule commands against the numbers the Blender add-on made.
//!
//! `tests/fixtures/garment_rules/worked_examples.json` holds one worked example
//! per rule group, recorded from `tools/blender_addon/sculpto_garment` under
//! Blender 5.2.3 before F03 moved the rules into the core. The commands must
//! reproduce those values; the add-on keeps only Blender data construction.
//!
//! |DECL0002|

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/garment_rules/worked_examples.json");
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture")).expect("fixture json")
}

/// Dispatch one command through the same `run` entry the add-on uses.
fn run(id: &str, payload: Value) -> Value {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["run", "--cmd", id, "--input", &payload.to_string()])
        .output()
        .expect("run sculpto");
    assert!(
        output.status.success(),
        "`{id}` failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: Value =
        serde_json::from_slice(&output.stdout).expect("one JSON envelope on stdout");
    assert_eq!(envelope["ok"], true, "`{id}` refused: {envelope}");
    envelope["result"].clone()
}

fn close(actual: f64, expected: f64, tolerance: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: rust {actual} python {expected}"
    );
}

#[test]
fn place_reproduces_the_python_placement() {
    let fixture = fixture();
    for (kind, expected) in fixture["place"].as_object().expect("place rows") {
        let result = run(
            "outfit.place",
            json!({
                "kind": kind,
                "body": {
                    "min_z": expected["body_min_z"],
                    "max_z": expected["body_max_z"],
                    "front_y": expected["body_front_y"],
                    "width_min_x": expected["body_width_min_x"],
                    "width_max_x": expected["body_width_max_x"],
                },
                "raw": {
                    "min_x": expected["raw_min_x"],
                    "max_x": expected["raw_max_x"],
                    "min_z": expected["raw_min_z"],
                    "max_z": expected["raw_max_z"],
                },
            }),
        );
        assert_eq!(result["kind"], kind.as_str());
        close(
            result["hem_z"].as_f64().expect("hem"),
            expected["hem_z"].as_f64().expect("hem"),
            1e-12,
            "hem_z",
        );
        close(
            result["collar_z"].as_f64().expect("collar"),
            expected["collar_z"].as_f64().expect("collar"),
            1e-12,
            "collar_z",
        );
        close(
            result["width"].as_f64().expect("width"),
            expected["width"].as_f64().expect("width"),
            1e-12,
            "width",
        );
        // The object transform is f32 in Blender, so it carries f32 precision.
        close(
            result["scale"].as_f64().expect("scale"),
            expected["scale"].as_f64().expect("scale"),
            1e-6,
            "scale",
        );
        for axis in 0..3 {
            close(
                result["location"][axis].as_f64().expect("location"),
                expected["location"][axis].as_f64().expect("location"),
                1e-6,
                "location",
            );
        }
    }
}

#[test]
fn band_reproduces_the_python_face_list() {
    let fixture = fixture();
    let band = &fixture["band"];
    for (kind, field) in [("upper-body", "upper_body"), ("trousers", "trousers")] {
        let result = run(
            "outfit.band",
            json!({
                "kind": kind,
                "positions": band["positions"],
                "polygons": band["polygons"],
                "hem_z": band["hem_z"],
                "collar_z": band["collar_z"],
            }),
        );
        assert_eq!(result["faces"], band[field], "{field} face list differs");
    }
}

/// One position key, quantized so the comparison ignores index order.
fn key(value: f64) -> i64 {
    (value * 1e6).round() as i64
}

#[test]
fn offsets_reproduce_the_python_surface() {
    let fixture = fixture();
    let offset = &fixture["offsets"];
    let result = run(
        "outfit.offsets",
        json!({
            "positions": offset["positions"],
            "polygons": offset["polygons"],
            "keep": offset["keep"],
        }),
    );
    // The fixture's body is a flat plank facing -y, so a displaced vertex is
    // `position - offset` on y.
    let mut rust: BTreeMap<(i64, i64), i64> = BTreeMap::new();
    for (position, amount) in result["positions"]
        .as_array()
        .expect("positions")
        .iter()
        .zip(result["offsets"].as_array().expect("offsets"))
    {
        rust.insert(
            (
                key(position[0].as_f64().expect("x")),
                key(position[2].as_f64().expect("z")),
            ),
            key(-amount.as_f64().expect("amount")),
        );
    }
    let python: BTreeMap<(i64, i64), i64> = offset["garment_positions"]
        .as_array()
        .expect("garment positions")
        .iter()
        .map(|position| {
            (
                (
                    key(position[0].as_f64().expect("x")),
                    key(position[2].as_f64().expect("z")),
                ),
                key(position[1].as_f64().expect("y")),
            )
        })
        .collect();
    assert_eq!(
        rust, python,
        "the offset surface differs from the Python one"
    );
    assert_eq!(result["boundary_vertices"], 8);
}

#[test]
fn projection_reproduces_the_python_uvs() {
    let fixture = fixture();
    let projection = &fixture["projection"];
    let result = run(
        "outfit.projection",
        json!({
            "origins": projection["origins"],
            "axis": projection["axis"],
            "raw": projection["raw"],
        }),
    );
    let uvs = result["uvs"].as_array().expect("uvs");
    let expected = projection["uvs"].as_array().expect("python uvs");
    assert_eq!(uvs.len(), expected.len(), "loop count differs");
    for (index, (actual, expected)) in uvs.iter().zip(expected).enumerate() {
        for axis in 0..2 {
            // Both sides are f32 in Blender; JSON widens them differently.
            let rust = actual[axis].as_f64().expect("uv") as f32;
            let python = expected[axis].as_f64().expect("uv") as f32;
            assert_eq!(rust.to_bits(), python.to_bits(), "loop {index} uv[{axis}]");
        }
    }
    assert_eq!(result["hits"], 37);
    assert_eq!(result["misses"], 17);
}

#[test]
fn weights_reproduce_the_python_transfer_and_unpose() {
    let fixture = fixture();
    let weights = &fixture["weights"];
    let transferred = run(
        "outfit.weights",
        json!({
            "stage": "transfer",
            "garment_positions": weights["garment_positions"],
            "body_positions": weights["body_positions"],
            "body_weights": weights["body_weights"],
        }),
    );
    let expected = weights["transferred"].as_array().expect("python weights");
    for (index, (actual, expected)) in transferred["weights"]
        .as_array()
        .expect("weights")
        .iter()
        .zip(expected)
        .enumerate()
    {
        let actual = actual.as_object().expect("influences");
        let expected = expected.as_object().expect("influences");
        assert_eq!(
            actual.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "vertex {index} has different bones"
        );
        for (bone, weight) in expected {
            close(
                actual[bone].as_f64().expect("weight"),
                weight.as_f64().expect("weight"),
                1e-6,
                &format!("vertex {index} {bone}"),
            );
        }
    }

    let unposed = run(
        "outfit.weights",
        json!({
            "stage": "unpose",
            "positions": weights["garment_positions"],
            "weights": weights["transferred"],
            "skinning": weights["skinning"],
        }),
    );
    assert_eq!(unposed["moved"], 4);
    assert_eq!(unposed["skipped"], 0);
    for (index, (actual, expected)) in unposed["positions"]
        .as_array()
        .expect("positions")
        .iter()
        .zip(weights["rest_positions"].as_array().expect("python rest"))
        .enumerate()
    {
        for axis in 0..3 {
            close(
                actual[axis].as_f64().expect("position"),
                expected[axis].as_f64().expect("position"),
                1e-5,
                &format!("vertex {index} axis {axis}"),
            );
        }
    }
}

#[test]
fn an_unknown_weight_stage_is_a_typed_refusal() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args([
            "run",
            "--cmd",
            "outfit.weights",
            "--input",
            r#"{"stage": "blend"}"#,
        ])
        .output()
        .expect("run sculpto");
    let envelope: Value = serde_json::from_slice(&output.stdout).expect("envelope");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "unknown_weight_stage");
}
