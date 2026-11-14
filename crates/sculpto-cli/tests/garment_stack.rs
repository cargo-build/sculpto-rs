use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// Outward-wound cube of half-extent `half` around `center`.
fn cube_obj(center: [f32; 3], half: f32) -> String {
    let [cx, cy, cz] = center;
    let positions = [
        [cx - half, cy - half, cz - half],
        [cx + half, cy - half, cz - half],
        [cx + half, cy + half, cz - half],
        [cx - half, cy + half, cz - half],
        [cx - half, cy - half, cz + half],
        [cx + half, cy - half, cz + half],
        [cx + half, cy + half, cz + half],
        [cx - half, cy + half, cz + half],
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

/// One sheet facing `+x` at `x`: an open piece whose depth is readable.
fn sheet_obj(x: f32) -> String {
    format!("v {x} 0 0\nv {x} 0.1 0\nv {x} 0 0.1\nf 1 2 3\n")
}

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-stack-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

/// Write `layers.json` and run `garment stack` with `extra` arguments.
fn stack(directory: &Path, manifest: &str, extra: &[&str]) -> Output {
    let path = directory.join("layers.json");
    fs::write(&path, manifest).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command
        .args(["garment", "stack", "--outfit"])
        .arg(&path)
        .args(extra);
    command.output().unwrap()
}

fn json_of(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn pair<'a>(report: &'a serde_json::Value, outer: &str, inner: &str) -> &'a serde_json::Value {
    report["pairs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pair| pair["outer"] == outer && pair["inner"] == inner)
        .unwrap_or_else(|| panic!("no `{outer}` vs `{inner}` pair in {report}"))
}

#[test]
fn json_report_grades_each_pair_against_its_inner_layer() {
    let directory = fixture_directory("json");
    fs::write(directory.join("body.obj"), cube_obj([0.0; 3], 0.4)).unwrap();
    fs::write(directory.join("panty.obj"), cube_obj([0.0; 3], 0.5)).unwrap();
    fs::write(directory.join("tunic.obj"), cube_obj([0.0; 3], 0.6)).unwrap();
    // 0.45 is outside the body and the panty by 0.05 and inside the tunic by 0.15.
    fs::write(directory.join("robe.obj"), sheet_obj(0.45)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "panty", "order": 1, "obj": "panty.obj"},
            {"id": "tunic", "order": 3, "obj": "tunic.obj"},
            {"id": "robe", "order": 4, "obj": "robe.obj"}
        ]}"#;

    let output = stack(&directory, manifest, &["--tolerance", "0.1", "--json"]);
    let report = json_of(&output);
    fs::remove_dir_all(&directory).unwrap();

    assert!(
        output.status.success(),
        "a measurement exits 0 even on fail"
    );
    assert_eq!(report["units"], "source");
    assert_eq!(report["tolerance"], 0.1);
    assert_eq!(
        report["selection"], "implied",
        "no `pairs` means the order rule"
    );
    assert_eq!(report["declared_pairs"], serde_json::Value::Null);
    assert_eq!(report["pass"], false);
    assert_eq!(report["failures"]["penetration"], 1);
    assert_eq!(report["failures"]["boot_mode_violation"], 0);
    assert_eq!(report["pairs"].as_array().unwrap().len(), 6);

    let poke = pair(&report, "robe", "tunic");
    assert_eq!(poke["kind"], "layer");
    assert_eq!(poke["classification"], "penetration");
    assert_eq!(poke["visible"], 3);
    assert_eq!(poke["violations"], 3);
    let max = poke["visible_max"].as_f64().unwrap();
    assert!((max - 0.15).abs() < 1e-4, "visible_max {max}");
    assert_eq!(poke["worst"].as_array().unwrap().len(), 3);
    assert_eq!(poke["worst"][0]["vertex"], 0);

    let shallow = pair(&report, "robe", "panty");
    assert_eq!(shallow["classification"], "layering_ok");
    assert_eq!(shallow["visible"], 3);
    let max = shallow["visible_max"].as_f64().unwrap();
    assert!((max - 0.05).abs() < 1e-4, "visible_max {max}");

    // A pair is graded outer-against-inner only: the outermost piece is never
    // the inner surface of anything.
    assert!(
        report["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|pair| pair["inner"] != "robe")
    );

    let layers = report["layers"].as_array().unwrap();
    let ids: Vec<&str> = layers
        .iter()
        .map(|layer| layer["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["panty", "tunic", "robe"]);
    assert_eq!(layers[2]["order"], 4);
    assert_eq!(layers[2]["pairs"], 3);
    assert_eq!(layers[2]["failed"], 1);
    assert_eq!(layers[2]["classification"], "penetration");
}

#[test]
fn text_report_prints_every_contact_type() {
    let directory = fixture_directory("types");
    fs::write(directory.join("body.obj"), cube_obj([0.0; 3], 0.4)).unwrap();
    fs::write(directory.join("panty.obj"), cube_obj([0.0; 3], 0.5)).unwrap();
    fs::write(directory.join("tunic.obj"), cube_obj([0.0; 3], 0.6)).unwrap();
    // Several steps off the body, so the accessory and the footwear are graded
    // only against the layers they declare.
    fs::write(directory.join("belt.obj"), sheet_obj(0.54)).unwrap();
    fs::write(
        directory.join("trousers.obj"),
        cube_obj([3.0, 0.0, 0.0], 0.6),
    )
    .unwrap();
    fs::write(directory.join("boots.obj"), cube_obj([3.0, 0.0, 0.0], 0.55)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "pairs": [["belt", "tunic"], ["boots", "trousers"]],
        "layers": [
            {"id": "panty", "order": 1, "obj": "panty.obj"},
            {"id": "trousers", "order": 2, "obj": "trousers.obj"},
            {"id": "tunic", "order": 3, "obj": "tunic.obj"},
            {"id": "belt", "order": 6, "obj": "belt.obj", "anchor": "tunic"},
            {"id": "boots", "order": 9, "obj": "boots.obj",
             "mode": "over-trousers", "pair": "trousers"}
        ]}"#;

    let output = stack(&directory, manifest, &["--tolerance", "0.03"]);
    fs::remove_dir_all(&directory).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("5 layers, body"), "{text}");
    assert!(
        text.contains("pairs: 7 graded, 0 penetration, 1 boot-mode violations"),
        "{text}"
    );
    assert!(
        text.contains("selection: 2 declared layer pair(s)"),
        "the declared selection is named: {text}"
    );
    assert!(
        text.contains("accessory_contact") && text.contains("visible 3 max 0.060000"),
        "the anchor contact is reported, not hidden: {text}"
    );
    assert!(
        text.contains("boot_mode_violation"),
        "boots inside the over-trousers layer fail the mode: {text}"
    );
    assert!(text.contains("layering_ok"), "{text}");
    assert!(text.contains("layers:"), "{text}");
    assert!(
        text.contains("order 9"),
        "a sparse footwear order is accepted and printed: {text}"
    );
    assert!(
        text.contains("verdict: fail (tolerance 0.030000; 0 penetration, 1 boot-mode violation)"),
        "{text}"
    );
    assert!(
        text.contains("note: a pair is graded outer-against-inner only"),
        "{text}"
    );
}

#[test]
fn body_can_come_from_the_flag_or_the_manifest() {
    let directory = fixture_directory("body");
    fs::write(directory.join("body.obj"), cube_obj([0.0; 3], 0.4)).unwrap();
    fs::write(directory.join("panty.obj"), cube_obj([0.0; 3], 0.5)).unwrap();
    let body = directory.join("body.obj");
    let declared_manifest =
        r#"{"body": "body.obj", "layers": [{"id": "panty", "order": 1, "obj": "panty.obj"}]}"#;
    let undeclared = r#"{"layers": [{"id": "panty", "order": 1, "obj": "panty.obj"}]}"#;
    let body_text = body.to_string_lossy().to_string();

    let declared = stack(
        &directory,
        declared_manifest,
        &["--tolerance", "0.1", "--json"],
    );
    let flagged = stack(
        &directory,
        undeclared,
        &["--tolerance", "0.1", "--body", &body_text, "--json"],
    );
    let mismatch = stack(
        &directory,
        declared_manifest,
        &["--tolerance", "0.1", "--body", "/nowhere/body.obj"],
    );
    let neither = stack(&directory, undeclared, &["--tolerance", "0.1"]);
    fs::remove_dir_all(&directory).unwrap();

    let report = json_of(&declared);
    assert_eq!(report["body"], body_text);
    assert_eq!(report["pass"], true);
    let flagged_report = json_of(&flagged);
    assert_eq!(flagged_report["body"], body_text);
    assert_eq!(flagged_report["pass"], true);

    assert!(!mismatch.status.success());
    let stderr = String::from_utf8(mismatch.stderr).unwrap();
    assert!(stderr.contains("`--body` names"), "{stderr}");
    assert!(!neither.status.success());
    let stderr = String::from_utf8(neither.stderr).unwrap();
    assert!(stderr.contains("declares no body"), "{stderr}");
}

#[test]
fn malformed_manifests_are_refused_loudly() {
    let directory = fixture_directory("refusals");
    fs::write(directory.join("body.obj"), cube_obj([0.0; 3], 0.4)).unwrap();
    fs::write(directory.join("panty.obj"), cube_obj([0.0; 3], 0.5)).unwrap();

    let unknown_anchor = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "belt", "order": 6, "obj": "panty.obj", "anchor": "ghost"}]}"#,
        &["--tolerance", "0.1"],
    );
    let boots_without_pair = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "boots", "order": 7, "obj": "panty.obj", "mode": "under-trousers"}]}"#,
        &["--tolerance", "0.1"],
    );
    fs::write(directory.join("cuff.obj"), cube_obj([10.0, 0.0, 0.0], 0.3)).unwrap();
    let disjoint_implied = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "panty", "order": 1, "obj": "panty.obj"},
                                            {"id": "cuff", "order": 2, "obj": "cuff.obj"}]}"#,
        &["--tolerance", "0.1"],
    );
    let unknown_declared_pair = stack(
        &directory,
        r#"{"body": "body.obj", "pairs": [["ghost", "panty"]],
            "layers": [{"id": "panty", "order": 1, "obj": "panty.obj"}]}"#,
        &["--tolerance", "0.1"],
    );
    let invalid_json = stack(&directory, "{\"layers\": [}", &["--tolerance", "0.1"]);
    let no_tolerance = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "panty", "order": 1, "obj": "panty.obj"}]}"#,
        &[],
    );
    let unknown_option = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "panty", "order": 1, "obj": "panty.obj"}]}"#,
        &["--tolerance", "0.1", "--layer", "panty"],
    );
    let missing_mesh = stack(
        &directory,
        r#"{"body": "body.obj", "layers": [{"id": "panty", "order": 1, "obj": "absent.obj"}]}"#,
        &["--tolerance", "0.1"],
    );

    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    let missing_manifest = command
        .args(["garment", "stack", "--outfit"])
        .arg(directory.join("absent.json"))
        .args(["--tolerance", "0.1"])
        .output()
        .unwrap();
    fs::remove_dir_all(&directory).unwrap();

    let cases = [
        (unknown_anchor, "unknown anchor `ghost`"),
        (boots_without_pair, "boot mode without `pair`"),
        (disjoint_implied, "`cuff` vs `panty`"),
        (unknown_declared_pair, "names unknown layer `ghost`"),
        (invalid_json, "invalid manifest"),
        (no_tolerance, "`--tolerance` is required"),
        (unknown_option, "unknown option `--layer`"),
        (missing_mesh, "absent.obj"),
        (missing_manifest, "absent.json"),
    ];
    for (output, expected) in cases {
        assert!(!output.status.success(), "`{expected}` was accepted");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("garment: "),
            "`{expected}`: no command prefix in {stderr}"
        );
        assert!(stderr.contains(expected), "`{expected}`: got {stderr}");
    }
}
