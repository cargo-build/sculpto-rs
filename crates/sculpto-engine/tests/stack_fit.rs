//! `outfit.stack_fit`: the accumulated obstacle surface, proven and refused.
//!
//! The falsifying case of the FIT-10 contract: a layer fitted against the
//! accumulated body-plus-layers obstacle must differ measurably from a body-only
//! fit of the same layer. The exact-Bijin cloak chain is the real-fixture
//! witness, gated on the host fixture root.
//! @req DECL0013 DECL0015

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sculpto_engine::ops::{StackFitParams, stack_fit};
use sculpto_engine::{ErrorKind, dispatch};
use sculpto_mesh::wrap::FitOptions;
use sculpto_mesh::wrap::repair::final_push::{FinalPushOptions, final_push};
use serde_json::{Value, json};

/// Outward-wound cube OBJ of half-extent `half`.
fn cube_obj(half: f32) -> String {
    let positions = [
        [-half, -half, -half],
        [half, -half, -half],
        [half, half, -half],
        [-half, half, -half],
        [-half, -half, half],
        [half, -half, half],
        [half, half, half],
        [-half, half, half],
    ];
    let faces = [
        [4, 5, 6],
        [4, 6, 7],
        [1, 0, 3],
        [1, 3, 2],
        [1, 2, 6],
        [1, 6, 5],
        [0, 4, 7],
        [0, 7, 3],
        [3, 7, 6],
        [3, 6, 2],
        [0, 1, 5],
        [0, 5, 4],
    ];
    let mut text = String::new();
    for [x, y, z] in positions {
        text.push_str(&format!("v {x} {y} {z}\n"));
    }
    for face in faces {
        text.push_str(&format!(
            "f {} {} {}\n",
            face[0] + 1,
            face[1] + 1,
            face[2] + 1
        ));
    }
    text
}

/// One sheet OBJ facing `+x` at `x`.
fn sheet_obj(x: f32) -> String {
    format!("v {x} 0 0\nv {x} 0.1 0\nv {x} 0 0.1\nf 1 2 3\n")
}

/// Three proper triangles fanned on one edge: a non-manifold obstacle.
fn non_manifold_obj() -> String {
    let mut text = String::new();
    for [x, y, z] in [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, -1.0, 0.0],
    ] {
        text.push_str(&format!("v {x} {y} {z}\n"));
    }
    text.push_str("f 1 2 3\nf 1 2 4\nf 1 2 5\n");
    text
}

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-stack-fit-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

/// Write `layers.json` and run the operation at `tolerance`/`passes`.
fn fit(
    directory: &Path,
    manifest: &str,
    tolerance: f32,
    passes: Option<u32>,
) -> Result<Value, String> {
    let path = directory.join("layers.json");
    fs::write(&path, manifest).unwrap();
    stack_fit(&StackFitParams {
        manifest: path,
        tolerance,
        body: None,
        passes,
    })
    .map(|outcome| outcome.to_json())
    .map_err(|error| format!("{:?}: {}: {}", error.kind(), error.code(), error.message()))
}

/// The refused `(kind, code, message)` of a run.
fn refusal(directory: &Path, manifest: &str, tolerance: f32) -> (String, String, String) {
    let path = directory.join("layers.json");
    fs::write(&path, manifest).unwrap();
    let error = stack_fit(&StackFitParams {
        manifest: path,
        tolerance,
        body: None,
        passes: None,
    })
    .expect_err("the stack was fitted instead of refused");
    (
        format!("{:?}", error.kind()),
        error.code().to_string(),
        error.message().to_string(),
    )
}

fn step<'a>(report: &'a Value, layer: &str) -> &'a Value {
    report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["layer"] == layer)
        .unwrap_or_else(|| panic!("no `{layer}` step in {report}"))
}

fn pair<'a>(report: &'a Value, outer: &str, inner: &str) -> &'a Value {
    report["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pair| pair["outer"] == outer && pair["inner"] == inner)
        .unwrap_or_else(|| panic!("no `{outer}` vs `{inner}` pair in {report}"))
}

#[test]
fn layer_two_is_fitted_against_the_accumulated_surface_not_the_body() {
    let directory = fixture_directory("accumulated");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    // 0.44 is outside the body (0.04 clear of it) and 0.06 inside the robe: a
    // body-only fit has nothing to move, an accumulated fit must push the
    // sheet out to just inside the robe's face.
    fs::write(directory.join("cloak.obj"), sheet_obj(0.44)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "robe", "order": 40, "obj": "robe.obj"},
            {"id": "cloak", "order": 60, "obj": "cloak.obj"}
        ]}"#;

    let report = fit(&directory, manifest, 0.02, None).expect("fitted");
    let cloak = step(&report, "cloak");
    assert_eq!(cloak["obstacle"]["surface"], "accumulated[body, robe]");
    assert_eq!(
        cloak["obstacle"]["layers"],
        json!(["body", "robe"]),
        "the report names the surface the step used"
    );
    assert_eq!(cloak["obstacle"]["triangles"], 24, "body plus robe");
    assert_eq!(
        cloak["contact_breakdown"],
        json!({"body": 0, "robe": 3}),
        "every graded cloak vertex touched the robe, none the body"
    );
    let fit_row = &cloak["fit"];
    assert_eq!(fit_row["before_violations"], 3);
    let before_max = fit_row["before_max"].as_f64().unwrap();
    assert!((before_max - 0.06).abs() < 1e-4, "{before_max}");
    assert_eq!(fit_row["after_violations"], 0);
    let after_max = fit_row["after_max"].as_f64().unwrap();
    assert!((after_max - 0.0199).abs() < 1e-4, "{after_max}");

    // The robe below was already clear of the body: its step moved nothing.
    let robe = step(&report, "robe");
    assert_eq!(robe["obstacle"]["surface"], "accumulated[body]");
    assert_eq!(robe["obstacle"]["layers"], json!(["body"]));
    assert_eq!(robe["fit"]["before_violations"], 0);
    assert_eq!(robe["fit"]["passes"], 0);

    // The fitted stack grades clean against the declared pairs.
    let cloak_robe = pair(&report, "cloak", "robe");
    assert_eq!(cloak_robe["classification"], "layering_ok");
    assert_eq!(cloak_robe["violations"], 0);
    let max = cloak_robe["visible_max"].as_f64().unwrap();
    assert!((max - 0.0199).abs() < 1e-4, "{max}");
    assert_eq!(report["pass"], true, "{report}");

    // The falsifying control: the same cloak fitted against the body only
    // (today's `outfit.fit`/`garment push` path) stays inside the robe.
    let body = sculpto_engine::obj::read_obj_mesh(&directory.join("body.obj")).unwrap();
    let cloak = sculpto_engine::obj::read_obj_mesh(&directory.join("cloak.obj")).unwrap();
    let options = FinalPushOptions {
        passes: 4,
        tolerance: 0.02,
    };
    let (body_only, body_report) = final_push(
        (&cloak.0, &cloak.1),
        (&body.0, &body.1),
        None,
        &FitOptions::default(),
        &options,
    )
    .expect("a valid body-only push");
    assert_eq!(
        body_report.before_violations, 0,
        "the body sees nothing to fix: this is why a body-only fit is silent"
    );
    assert_eq!(body_only, cloak.0, "the body-only fit moves nothing");
    let graded =
        sculpto_mesh::check::garment_depth_report((&body_only, &cloak.1), (&body.0, &body.1))
            .unwrap();
    assert_eq!(graded.visible_occluded.count, 0);
    let robe_mesh = sculpto_engine::obj::read_obj_mesh(&directory.join("robe.obj")).unwrap();
    let against_robe = sculpto_mesh::check::garment_depth_report(
        (&body_only, &cloak.1),
        (&robe_mesh.0, &robe_mesh.1),
    )
    .unwrap();
    let verdict = against_robe.visible_verdict(0.02);
    assert_eq!(
        verdict.violations.len(),
        3,
        "the body-only fit leaves the cloak inside the robe"
    );
    let body_only_max = against_robe.visible_occluded.max as f64;
    assert!((body_only_max - 0.06).abs() < 1e-4, "{body_only_max}");
    assert!(
        body_only_max - after_max > 0.03,
        "the accumulated fit must differ measurably: body-only {body_only_max} vs accumulated {after_max}"
    );

    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn the_fitted_outputs_are_written_and_regrade_clean() {
    let directory = fixture_directory("written");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("cloak.obj"), sheet_obj(0.44)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "robe", "order": 40, "obj": "robe.obj"},
            {"id": "cloak", "order": 60, "obj": "cloak.obj", "out": "dressed/cloak.obj"}
        ]}"#;

    let report = fit(&directory, manifest, 0.02, None).expect("fitted");
    let cloak = step(&report, "cloak");
    let out = PathBuf::from(cloak["out"].as_str().unwrap());
    assert_eq!(out, directory.join("dressed/cloak.obj"));
    assert!(out.is_file(), "the declared `out` was written");
    let default = PathBuf::from(step(&report, "robe")["out"].as_str().unwrap());
    assert_eq!(
        default,
        directory.join("robe.stackfit.obj"),
        "an undeclared `out` lands beside its source"
    );
    assert!(default.is_file());

    // The written cloak re-grades to the reported numbers: the file keeps the
    // coordinates the step graded.
    let written = sculpto_engine::obj::read_obj_mesh(&out).unwrap();
    let robe = sculpto_engine::obj::read_obj_mesh(&directory.join("robe.obj")).unwrap();
    let regrade =
        sculpto_mesh::check::garment_depth_report((&written.0, &written.1), (&robe.0, &robe.1))
            .unwrap();
    assert_eq!(regrade.visible_verdict(0.02).violations.len(), 0);
    let max = regrade.visible_occluded.max;
    assert!((max - 0.0199).abs() < 1e-4, "{max}");

    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn refused_declarations_and_underivable_surfaces_name_their_code() {
    let directory = fixture_directory("refused");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("torn.obj"), non_manifold_obj()).unwrap();

    let empty = r#"{"body": "body.obj", "layers": []}"#;
    let (kind, code, message) = refusal(&directory, empty, 0.05);
    assert_eq!(
        (kind.as_str(), code.as_str()),
        ("InvalidParams", "stack_fit_manifest_invalid")
    );
    assert!(message.contains("declares no layers"), "{message}");

    let missing = r#"{"body": "body.obj",
        "layers": [{"id": "robe", "order": 40, "obj": "ghost.obj"}]}"#;
    let (kind, code, message) = refusal(&directory, missing, 0.05);
    assert_eq!(
        (kind.as_str(), code.as_str()),
        ("NotFound", "stack_fit_layer_missing")
    );
    assert!(message.contains("ghost.obj"), "{message}");

    // Two layers dressing at one rank: the sequence has no total order.
    let shared = r#"{"body": "body.obj",
        "layers": [{"id": "robe", "order": 40, "obj": "robe.obj"},
                   {"id": "cloak", "order": 40, "obj": "robe.obj"}]}"#;
    let (kind, code, message) = refusal(&directory, shared, 0.05);
    assert_eq!(
        (kind.as_str(), code.as_str()),
        ("Refused", "stack_fit_layer_underivable")
    );
    assert!(
        message.contains("`cloak` declares order 40, not above"),
        "{message}"
    );

    let torn = r#"{"body": "body.obj",
        "layers": [{"id": "robe", "order": 40, "obj": "torn.obj"}]}"#;
    let (kind, code, message) = refusal(&directory, torn, 0.05);
    assert_eq!(
        (kind.as_str(), code.as_str()),
        ("Refused", "stack_fit_layer_underivable")
    );
    assert!(message.contains("non-manifold edge"), "{message}");

    // A body the composite cannot derive is its own refusal.
    let bad_body = r#"{"body": "torn.obj",
        "layers": [{"id": "robe", "order": 40, "obj": "robe.obj"}]}"#;
    let (kind, code, _message) = refusal(&directory, bad_body, 0.05);
    assert_eq!(
        (kind.as_str(), code.as_str()),
        ("Refused", "stack_fit_body_underivable")
    );

    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn the_registry_dispatches_the_command_with_typed_missing_fields() {
    let error = dispatch("outfit.stack_fit", &json!({})).expect_err("params are required");
    assert_eq!(error.kind(), ErrorKind::InvalidParams);
    assert_eq!(error.code(), "missing_field");

    let listed = dispatch("commands.list", &Value::Null).unwrap();
    let row = listed["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "outfit.stack_fit")
        .expect("the registry lists outfit.stack_fit");
    assert_eq!(row["implemented"], true);
    assert_eq!(row["mutates"], true);
    assert!(row["input_schema"].get("title").is_some());
}

/// The recorded exact-Bijin cloak chain: the cloak pushed against the
/// accumulated body-plus-pushed-robe surface reproduces the manual chain's
/// `2.233330 / 67 -> 0.149904 / 0` through the command.
#[test]
fn the_exact_bijin_cloak_chain_reproduces_through_the_command() {
    let Some(root) = std::env::var_os("SCULPTO_BIJIN_REPRO_FIXTURE") else {
        eprintln!(
            "SKIP the_exact_bijin_cloak_chain_reproduces_through_the_command: missing SCULPTO_BIJIN_REPRO_FIXTURE"
        );
        return;
    };
    let root = PathBuf::from(root);
    let body = root.join("collision.obj");
    let robe = root.join("pull-robe.obj");
    let cloak = root.join("pull-cloak.obj");
    for path in [&body, &robe, &cloak] {
        assert!(
            path.is_file(),
            "configured fixture {} must exist",
            path.display()
        );
    }
    let directory = fixture_directory("bijin");
    let manifest = format!(
        r#"{{"body": "{}",
            "pairs": [["cloak", "robe"]],
            "layers": [
                {{"id": "robe", "order": 40, "obj": "{}", "out": "{}"}},
                {{"id": "cloak", "order": 60, "obj": "{}", "out": "{}"}}
            ]}}"#,
        body.display(),
        robe.display(),
        directory.join("robe.stackfit.obj").display(),
        cloak.display(),
        directory.join("cloak.stackfit.obj").display(),
    );

    let report = fit(&directory, &manifest, 0.15, Some(4)).expect("fitted");

    let robe_step = step(&report, "robe");
    assert_eq!(robe_step["obstacle"]["surface"], "accumulated[body]");
    assert_eq!(robe_step["fit"]["before_violations"], 1);
    let before = robe_step["fit"]["before_max"].as_f64().unwrap();
    assert!((before - 0.151778).abs() < 1e-5, "{before}");
    assert_eq!(robe_step["fit"]["after_violations"], 0);
    let after = robe_step["fit"]["after_max"].as_f64().unwrap();
    assert!((after - 0.149900).abs() < 1e-5, "{after}");

    let cloak_step = step(&report, "cloak");
    assert_eq!(cloak_step["obstacle"]["surface"], "accumulated[body, robe]");
    assert_eq!(
        cloak_step["contact_breakdown"],
        json!({"body": 0, "robe": 86}),
        "the graded cloak touched only the robe: the step provably did not query the body alone"
    );
    assert_eq!(cloak_step["fit"]["before_violations"], 67);
    let before = cloak_step["fit"]["before_max"].as_f64().unwrap();
    assert!((before - 2.233330).abs() < 1e-5, "{before}");
    assert_eq!(cloak_step["fit"]["after_violations"], 0);
    let after = cloak_step["fit"]["after_max"].as_f64().unwrap();
    assert!((after - 0.149904).abs() < 1e-5, "{after}");
    assert_eq!(
        cloak_step["fit"]["passes"], 2,
        "the recorded chain took two passes"
    );

    let cloak_robe = pair(&report, "cloak", "robe");
    assert_eq!(cloak_robe["classification"], "layering_ok");
    assert_eq!(cloak_robe["visible"], 86);
    assert_eq!(cloak_robe["violations"], 0);
    let max = cloak_robe["visible_max"].as_f64().unwrap();
    assert!((max - 0.149904).abs() < 1e-5, "{max}");
    let cloak_body = pair(&report, "cloak", "body");
    assert_eq!(cloak_body["visible"], 0);
    assert_eq!(cloak_body["visible_max"], 0.0);
    assert_eq!(report["pass"], true, "{report}");

    fs::remove_dir_all(&directory).unwrap();
}
