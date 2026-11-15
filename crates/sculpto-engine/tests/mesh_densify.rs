//! `mesh.densify` operation contract on portable fixtures: counts, the
//! diagnostic report shape, typed refusals, and the report file.
//!
//! The real-data acceptance (the supplied 664-v panty and the golden 3BA
//! panty) lives in the CLI suite, which owns the argv/`run` parity.

use std::path::{Path, PathBuf};

use sculpto_engine::dispatch;
use serde_json::{Value, json};

fn work_dir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("sculpto-mesh-densify-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A unit square split around a center vertex: 5 vertices (one interior),
/// 8 edges, one 4-edge boundary loop.
fn square_obj() -> String {
    "v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
v 0.5 0.5 0
f 1 2 5
f 2 3 5
f 3 4 5
f 4 1 5
"
    .to_string()
}

fn dispatch_ok(name: &str, payload: &Value) -> Value {
    let result = dispatch(name, payload);
    let value = sculpto_engine::envelope(result);
    assert_eq!(value["ok"], true, "dispatch failed: {value}");
    value["result"].clone()
}

fn densify(dir: &Path, input_obj: &str, extra: Value) -> Value {
    let input = PathBuf::from(dir).join("in.obj");
    std::fs::write(&input, input_obj).expect("fixture");
    let out = PathBuf::from(dir).join("out.obj");
    let mut payload = json!({
        "in": input.display().to_string(),
        "out": out.display().to_string(),
    });
    payload
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    dispatch_ok("mesh.densify", &payload)
}

fn count_lines(path: &Path, prefix: &str) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .lines()
        .filter(|line| line.starts_with(prefix))
        .count()
}

#[test]
fn counts_are_deterministic_and_the_report_marks_the_diagnostic() {
    let dir = work_dir("counts");
    let report = densify(&dir, &square_obj(), json!({ "cuts": 1 }));
    // V' = V + E = 5 + 8 = 13; F' = 4F = 16.
    assert_eq!(report["source_vertices"], 5);
    assert_eq!(report["source_triangles"], 4);
    assert_eq!(report["vertices"], 13);
    assert_eq!(report["triangles"], 16);
    assert_eq!(report["cuts"], 1);
    // Per-part preservation: one component before and after, one 4-edge loop
    // that becomes an 8-edge loop, boundary drift exactly zero.
    assert_eq!(report["components"]["indexed"]["before"], 1);
    assert_eq!(report["components"]["indexed"]["after"], 1);
    assert_eq!(report["components"]["exact_position"]["before"], 1);
    assert_eq!(report["components"]["exact_position"]["after"], 1);
    assert_eq!(report["openings"]["before"][0]["edges"], 4);
    assert_eq!(report["openings"]["after"][0]["edges"], 8);
    assert_eq!(report["boundary_vertices"]["before"], 4);
    assert_eq!(report["boundary_vertices"]["after"], 8);
    assert_eq!(report["boundary_max_drift"], 0.0);
    assert_eq!(report["smoothing"]["enabled"], false);
    // The no-fit marker: no band, clearance or fit field anywhere, and the
    // report says what the output is.
    assert_eq!(report["diagnostic"], true);
    assert_eq!(report["uv"], "absent");
    assert_eq!(report["material"], "not_carried");
    for forbidden in [
        "clearance",
        "contact",
        "band",
        "pokes",
        "tolerance",
        "floating",
    ] {
        assert!(
            report.get(forbidden).is_none(),
            "a densify report must not carry `{forbidden}`"
        );
    }
    assert!(
        report["note"]
            .as_str()
            .expect("note")
            .contains("not a fit result"),
        "the report must say the output is not a fit result"
    );
}

#[test]
fn the_written_obj_keeps_the_source_vertices_and_their_order() {
    let dir = work_dir("roundtrip");
    let report = densify(
        &dir,
        &square_obj(),
        json!({ "cuts": 1, "smooth_iterations": 3 }),
    );
    let out = PathBuf::from(&dir).join("out.obj");
    assert_eq!(count_lines(&out, "v "), 13);
    assert_eq!(count_lines(&out, "f "), 16);
    // The first four written vertices are the input's own, in order.
    let input: Vec<[f32; 3]> = square_obj()
        .lines()
        .filter_map(|line| line.strip_prefix("v "))
        .map(|rest| {
            let numbers: Vec<f32> = rest
                .split_whitespace()
                .map(|v| v.parse().expect("f32"))
                .collect();
            [numbers[0], numbers[1], numbers[2]]
        })
        .collect();
    let text = std::fs::read_to_string(&out).expect("out OBJ");
    let written: Vec<[f64; 3]> = text
        .lines()
        .filter_map(|line| line.strip_prefix("v "))
        .map(|rest| {
            let numbers: Vec<f64> = rest
                .split_whitespace()
                .map(|v| v.parse().expect("f64"))
                .collect();
            [numbers[0], numbers[1], numbers[2]]
        })
        .collect();
    for (index, expected) in input.iter().enumerate() {
        let found = written[index];
        assert!(
            found
                .iter()
                .zip(expected.iter())
                .all(|(found, expected)| (found - f64::from(*expected)).abs() < 1e-5),
            "vertex {index} moved: {found:?} vs {expected:?}"
        );
    }
    // Smoothing ran on the interior and the report measured it, while the
    // boundary stayed at zero drift.
    assert_eq!(report["smoothing"]["enabled"], true);
    assert!(
        report["smoothing"]["max_displacement"]
            .as_f64()
            .expect("number")
            > 0.0,
        "the interior should move under smoothing"
    );
    assert_eq!(report["smoothing"]["boundary_max_displacement"], 0.0);
}

#[test]
fn refusals_are_typed_and_leave_the_output_unwritten() {
    let dir = work_dir("refusals");
    let input = PathBuf::from(&dir).join("in.obj");
    let out = PathBuf::from(&dir).join("out.obj");
    let run = |text: &str, extra: Value| {
        std::fs::write(&input, text).expect("fixture");
        let mut payload = json!({
            "in": input.display().to_string(),
            "out": out.display().to_string(),
        });
        payload
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        let error = dispatch("mesh.densify", &payload).expect_err("must refuse");
        (error.kind().as_str(), error.code().to_string())
    };

    let (kind, code) = run(&square_obj(), json!({ "cuts": 3 }));
    assert_eq!(
        (kind, code.as_str()),
        ("invalid_params", "densify_parameter")
    );
    let (_, code) = run(&square_obj(), json!({ "smooth_iterations": 21 }));
    assert_eq!(code, "densify_parameter");

    // A repeated corner: subdivision would change the topology.
    let degenerate = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 1 2\n";
    let (_, code) = run(degenerate, json!({}));
    assert_eq!(code, "densify_degenerate_face");

    // Three faces on one edge: the boundary reading is ambiguous.
    let non_manifold = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nv 0 0.5 0\nf 1 2 3\nf 1 2 4\nf 1 2 5\n";
    let (_, code) = run(non_manifold, json!({}));
    assert_eq!(code, "densify_non_manifold_edge");

    // A smoothing stage beyond its bound refuses and writes nothing.
    let (_, code) = run(
        &square_obj(),
        json!({ "smooth_iterations": 2, "smooth_max_shift": 1e-6 }),
    );
    assert_eq!(code, "densify_smooth_shift");
    assert!(
        !out.exists(),
        "a refused densify must leave `out` unwritten"
    );

    // Unreadable input is the OBJ I/O class.
    let error = dispatch(
        "mesh.densify",
        &json!({ "in": dir.join("missing.obj"), "out": out.display().to_string() }),
    )
    .expect_err("must refuse");
    assert_eq!(
        (error.kind().as_str(), error.code()),
        ("io", "obj_unreadable")
    );
    assert!(
        !out.exists(),
        "a refused densify must leave `out` unwritten"
    );
}

#[test]
fn the_report_file_is_the_returned_object() {
    let dir = work_dir("report");
    let report_path = PathBuf::from(&dir).join("report.json");
    let report = densify(
        &dir,
        &square_obj(),
        json!({ "cuts": 0, "report": report_path }),
    );
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("report file"))
            .expect("JSON report");
    assert_eq!(written, report);
}
