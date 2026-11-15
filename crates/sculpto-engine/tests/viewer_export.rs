//! `viewer.export` handler: units, palette, refusals and the report contract.

use std::path::Path;

use sculpto_engine::dispatch;
use serde_json::{Value, json};

/// A two-triangle OBJ at the given vertical offset, in display units.
fn fixture_obj(offset: f32) -> String {
    format!(
        "# test\nv 0 {offset} 0\nv 71.4286 {offset} 0\nv 0 {} 0\nf 1 2 3\n",
        offset + 71.4286
    )
}

fn write_fixture(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("write fixture");
    path
}

#[test]
fn the_report_scales_to_metres_and_names_the_palette() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-report");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let layer = write_fixture(&dir, "bra.obj", &fixture_obj(10.0));
    let out = dir.join("review.glb");

    let result = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": out.display().to_string(),
            "layers": [{ "order": 1, "obj": layer.display().to_string() }],
        }),
    )
    .expect("the export succeeds");

    assert_eq!(result["statement"], "review artifact, not a fit result");
    assert_eq!(result["review_only"], true);
    assert_eq!(result["units_per_metre"], 71.4286);
    let scale = result["scale"].as_f64().expect("scale");
    assert!((scale - 1.0 / 71.4286).abs() < 1e-9, "scale {scale}");

    let parts = result["parts"].as_array().expect("part rows");
    assert_eq!(parts.len(), 2, "body first, then the layer");
    assert_eq!(parts[0]["role"], "body");
    assert_eq!(parts[0]["order"], 0);
    assert_eq!(parts[0]["name"], "body");
    assert_eq!(parts[0]["vertices"], 3);
    assert_eq!(parts[0]["material"], "#E8D5C4");
    // One body-side vertex spans exactly 1 metre after the scale.
    let min = parts[0]["bbox_min"].as_array().expect("bbox");
    let max = parts[0]["bbox_max"].as_array().expect("bbox");
    let width = max[0].as_f64().expect("x") - min[0].as_f64().expect("x");
    assert!(
        (width - 1.0).abs() < 1e-6,
        "71.4286 units widen to 1 m, got {width}"
    );
    assert!((min[0].as_f64().expect("x")).abs() < 1e-9);
    assert_eq!(parts[1]["role"], "layer");
    assert_eq!(parts[1]["order"], 1);
    assert_eq!(parts[1]["material"], "#2ECC71", "rank 1 is the green slot");

    let bounds = &result["bounds"];
    assert!(bounds["min"].is_array() && bounds["max"].is_array());
    assert!(out.exists(), "the GLB was written");
    assert!(result["bytes"].as_u64().expect("bytes") > 0);
}

#[test]
fn a_layer_path_that_is_not_obj_is_refused() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-notobj");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let glb_layer = write_fixture(&dir, "bra.glb", "not an obj");

    let error = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": dir.join("review.glb").display().to_string(),
            "layers": [{ "order": 1, "obj": glb_layer.display().to_string() }],
        }),
    )
    .expect_err("a non-OBJ layer is refused");
    assert_eq!(error.code(), "viewer_layer_not_obj");
    assert_eq!(error.kind(), sculpto_engine::ErrorKind::InvalidParams);
}

#[test]
fn a_palette_order_outside_one_to_five_is_refused() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-palette");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let layer = write_fixture(&dir, "cloak.obj", &fixture_obj(1.0));

    for order in [0u32, 6, 9] {
        let error = dispatch(
            "viewer.export",
            &json!({
                "body": body.display().to_string(),
                "out": dir.join("review.glb").display().to_string(),
                "layers": [{ "order": order, "obj": layer.display().to_string() }],
            }),
        )
        .expect_err("the rank is outside the palette");
        assert_eq!(error.code(), "viewer_palette_order", "order {order}");
    }
}

#[test]
fn two_layers_on_one_rank_are_refused() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-duplicate");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let bra = write_fixture(&dir, "bra.obj", &fixture_obj(1.0));
    let panty = write_fixture(&dir, "panty.obj", &fixture_obj(2.0));

    let error = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": dir.join("review.glb").display().to_string(),
            "layers": [
                { "order": 1, "obj": bra.display().to_string() },
                { "order": 1, "obj": panty.display().to_string() },
            ],
        }),
    )
    .expect_err("one colour per rank");
    assert_eq!(error.code(), "viewer_palette_duplicate");
}

#[test]
fn an_empty_layer_mesh_is_refused() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-empty");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let hollow = write_fixture(&dir, "hollow.obj", "v 0 0 0\nv 1 0 0\n");

    let error = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": dir.join("review.glb").display().to_string(),
            "layers": [{ "order": 3, "obj": hollow.display().to_string() }],
        }),
    )
    .expect_err("no triangles is a refusal");
    assert_eq!(error.kind(), sculpto_engine::ErrorKind::Refused);
    assert_eq!(error.code(), "viewer_part_empty");
    assert!(
        error.message().contains("hollow"),
        "the refusal names the part"
    );
}

#[test]
fn layers_and_outfit_together_are_a_conflict() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-conflict");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let layer = write_fixture(&dir, "bra.obj", &fixture_obj(1.0));
    let manifest = write_fixture(
        &dir,
        "outfit.json",
        &json!({
            "layers": [{ "id": "bra", "order": 1, "obj": "bra.obj" }]
        })
        .to_string(),
    );

    let error = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": dir.join("review.glb").display().to_string(),
            "layers": [{ "order": 1, "obj": layer.display().to_string() }],
            "outfit": manifest.display().to_string(),
        }),
    )
    .expect_err("one layer route at a time");
    assert_eq!(error.code(), "invalid_value");
}

#[test]
fn the_outfit_manifest_supplies_layers_resolved_beside_itself() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-manifest");
    let outfit_dir = dir.join("outfit");
    std::fs::create_dir_all(&outfit_dir).expect("temp dir");
    let _body = write_fixture(&outfit_dir, "body.obj", &fixture_obj(0.0));
    write_fixture(&outfit_dir, "bra.obj", &fixture_obj(1.0));
    write_fixture(&outfit_dir, "panty.obj", &fixture_obj(2.0));
    let manifest = write_fixture(
        &outfit_dir,
        "layers.json",
        &json!({
            "body": "body.obj",
            "layers": [
                { "id": "bra", "order": 1, "obj": "bra.obj" },
                { "id": "panty", "order": 2, "obj": "panty.obj" }
            ]
        })
        .to_string(),
    );
    let out = dir.join("review.glb");
    let report_path = dir.join("report.json");

    let result = dispatch(
        "viewer.export",
        &json!({
            "out": out.display().to_string(),
            "outfit": manifest.display().to_string(),
            "report": report_path.display().to_string(),
        }),
    )
    .expect("the manifest export succeeds");

    let parts = result["parts"].as_array().expect("part rows");
    assert_eq!(parts.len(), 3, "body, bra, panty");
    assert_eq!(parts[1]["name"], "bra");
    assert_eq!(parts[1]["material"], "#2ECC71");
    assert_eq!(parts[2]["name"], "panty");
    assert_eq!(parts[2]["material"], "#3498DB", "rank 2 is the blue slot");
    assert!(out.exists());
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&report_path).expect("report"))
            .expect("a JSON report");
    assert_eq!(written["statement"], "review artifact, not a fit result");
}

#[test]
fn a_conflicting_manifest_body_is_refused() {
    let dir = std::env::temp_dir().join("sculpto-viewer-export-bodyconflict");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let body = write_fixture(&dir, "body.obj", &fixture_obj(0.0));
    let _other = write_fixture(&dir, "other.obj", &fixture_obj(3.0));
    write_fixture(&dir, "bra.obj", &fixture_obj(1.0));
    let manifest = write_fixture(
        &dir,
        "layers.json",
        &json!({
            "body": "other.obj",
            "layers": [{ "id": "bra", "order": 1, "obj": "bra.obj" }]
        })
        .to_string(),
    );

    let error = dispatch(
        "viewer.export",
        &json!({
            "body": body.display().to_string(),
            "out": dir.join("review.glb").display().to_string(),
            "outfit": manifest.display().to_string(),
        }),
    )
    .expect_err("two bodies is an ambiguity");
    assert_eq!(error.code(), "invalid_value");
    assert!(error.message().contains("disagree"));
}
