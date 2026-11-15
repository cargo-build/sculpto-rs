//! Portable per-frame stress harness test: a synthetic skinned body + shell
//! pair posed at 2 declared clip times, plus the typed refusals.
//!
//! The shell starts outside the body and its joint animates it to the body
//! centre, so frame 0 must be clean and frame 1 must grade every shell
//! vertex inside: an expected visible_max change through the real
//! `actor stress` argv surface. The shell never crosses a body wall, so
//! frame 1's `body_pairs` stays 0 — the recorded witness that graded depth
//! is not containment.

#[path = "../../sculpto-mesh/tests/common/mod.rs"]
#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

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
fn cube(fixture: &mut common::Fixture, scale: f32, offset: [f32; 3]) -> (usize, usize) {
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
fn skinned(fixture: &mut common::Fixture, mesh: usize, joint: u8) {
    let joints = fixture.accessor(&[joint, 0, 0, 0].repeat(8), 5121, "VEC4", 8);
    let weights = fixture.floats(&[1.0, 0.0, 0.0, 0.0].repeat(8), "VEC4", 8);
    fixture.json["meshes"][mesh]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][mesh]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
}

/// Body cube centred on the origin; shell cube of half-size 0.3 centred at
/// the origin and skinned to its own joint, whose authored rest translation
/// parks it at z = 3 (clear of the body); `Walk` moves the joint to the
/// origin between t=0 and t=1 (fully inside).
fn fixture() -> common::Fixture {
    let mut fixture = common::Fixture {
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
    fixture
}

fn scratch(tag: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "sculpto-actor-stress-{tag}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&root).unwrap();
    root
}

fn run(input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .arg("actor")
        .arg("stress")
        .arg("--input")
        .arg(input)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn two_frames_grade_the_shell_from_clear_to_contained() {
    let root = scratch("frames");
    let input = root.join("actor.glb");
    let report = root.join("report.json");
    fs::write(&input, fixture().bytes()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["actor", "stress"])
        .arg("--input")
        .arg(&input)
        .args(["--body", "Body", "--garment", "Shell"])
        .args(["--clip", "Walk", "--frames", "2", "--tolerance", "0.0021"])
        .arg("--out")
        .arg(&report)
        .output()
        .unwrap();
    // Frame 1 genuinely fails the tolerance: the report is the product and
    // the exit code carries the failed verdict, like `garment check`.
    assert_eq!(result.status.code(), Some(1), "{result:?}");

    let parsed: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(parsed["pose"]["kind"], "clip");
    assert_eq!(parsed["pose"]["name"], "Walk");
    assert_eq!(parsed["body"], "Body");
    assert_eq!(parsed["garments"][0], "Shell");
    assert_eq!(parsed["passed"], json!(false));
    assert_eq!(parsed["frames"].as_array().unwrap().len(), 2);

    let clean = &parsed["frames"][0];
    assert_eq!(clean["time_seconds"], json!(0.0));
    assert_eq!(clean["visible_max"], json!(0.0));
    assert_eq!(clean["penetrations"], json!(0));
    assert_eq!(clean["body_pairs"], json!(0));
    assert_eq!(clean["passed"], json!(true));

    let contained = &parsed["frames"][1];
    assert_eq!(contained["time_seconds"], json!(1.0));
    let visible_max = contained["visible_max"].as_f64().unwrap();
    assert!(
        visible_max > 0.5,
        "the shell vertices sit 0.7 inside the body, got {visible_max}"
    );
    assert!(contained["penetrations"].as_u64().unwrap() >= 1);
    assert!(contained["violations"].as_u64().unwrap() >= 1);
    // The shell is fully inside the body wall: pairs stay 0 while the graded
    // depth fails — depth is not containment, recorded in the same row.
    assert_eq!(contained["body_pairs"], json!(0));
    assert_eq!(contained["passed"], json!(false));
    let garment = &contained["garments"][0];
    assert_eq!(garment["vertices"], json!(8));
    assert_eq!(garment["self_pairs"], json!(0));

    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(stdout.contains("frame 0 t=0.000000"), "{stdout}");
    assert!(stdout.contains("frame 1 t=1.000000"), "{stdout}");
    assert!(stdout.contains("FAIL"), "{stdout}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rest_runs_one_row_and_refusals_are_typed() {
    let root = scratch("refusals");
    let input = root.join("actor.glb");
    fs::write(&input, fixture().bytes()).unwrap();

    let rest = run(
        &input,
        &[
            "--body",
            "Body",
            "--garment",
            "Shell",
            "--rest",
            "--tolerance",
            "0.0021",
        ],
    );
    assert!(rest.status.success(), "{rest:?}");
    let stdout = String::from_utf8(rest.stdout).unwrap();
    assert!(stdout.contains("rest"), "{stdout}");
    assert!(stdout.contains("PASS"), "{stdout}");

    let missing_part = run(
        &input,
        &[
            "--body",
            "Missing",
            "--garment",
            "Shell",
            "--rest",
            "--tolerance",
            "1.0",
        ],
    );
    assert_eq!(missing_part.status.code(), Some(1));
    let stderr = String::from_utf8(missing_part.stderr).unwrap();
    assert!(
        stderr.contains("actor part \"Missing\": expected one mesh node, found 0"),
        "{stderr}"
    );

    let missing_clip = run(
        &input,
        &[
            "--body",
            "Body",
            "--garment",
            "Shell",
            "--clip",
            "Missing",
            "--frames",
            "2",
            "--tolerance",
            "1.0",
        ],
    );
    assert_eq!(missing_clip.status.code(), Some(1));
    let stderr = String::from_utf8(missing_clip.stderr).unwrap();
    assert!(
        stderr.contains("actor clip \"Missing\": expected one clip, found 0"),
        "{stderr}"
    );

    let ambiguous_primitive = run(
        &input,
        &[
            "--body",
            "Body",
            "--garment",
            "Shell#5",
            "--rest",
            "--tolerance",
            "1.0",
        ],
    );
    assert_eq!(ambiguous_primitive.status.code(), Some(1));
    let stderr = String::from_utf8(ambiguous_primitive.stderr).unwrap();
    assert!(stderr.contains("primitive 5 is out of range"), "{stderr}");

    let bad_frames = run(
        &input,
        &[
            "--body",
            "Body",
            "--garment",
            "Shell",
            "--clip",
            "Walk",
            "--frames",
            "0",
            "--tolerance",
            "1.0",
        ],
    );
    assert_eq!(bad_frames.status.code(), Some(2));
    let stderr = String::from_utf8(bad_frames.stderr).unwrap();
    assert!(
        stderr.contains("--frames must be a count of at least 1"),
        "{stderr}"
    );
    fs::remove_dir_all(root).unwrap();
}
