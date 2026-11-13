use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn cube_obj() -> String {
    let positions = [
        (-1.0, -1.0, -1.0),
        (1.0, -1.0, -1.0),
        (1.0, 1.0, -1.0),
        (-1.0, 1.0, -1.0),
        (-1.0, -1.0, 1.0),
        (1.0, -1.0, 1.0),
        (1.0, 1.0, 1.0),
        (-1.0, 1.0, 1.0),
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
    for (x, y, z) in positions {
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

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-depth-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

fn depth_with(body: &Path, garment: &Path, json: bool, tolerance: Option<f32>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command
        .args(["garment", "depth", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment);
    if json {
        command.arg("--json");
    }
    if let Some(tolerance) = tolerance {
        command.arg("--tolerance").arg(tolerance.to_string());
    }
    command.output().unwrap()
}

fn depth(body: &Path, garment: &Path, json: bool) -> Output {
    depth_with(body, garment, json, None)
}

fn json_of(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn json_and_text_report_the_measured_penetration() {
    let directory = fixture_directory("measured");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, cube_obj()).unwrap();
    // A triangle entirely inside the cube, 0.2 behind its +x face.
    fs::write(&garment, "v 0.8 0 0\nv 0.8 0.1 0\nv 0.8 0 0.1\nf 1 2 3\n").unwrap();

    let machine = depth(&body, &garment, true);
    let text = depth(&body, &garment, false);
    fs::remove_dir_all(&directory).unwrap();

    assert!(
        machine.status.success(),
        "{}",
        String::from_utf8_lossy(&machine.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(value["units"], "source");
    assert_eq!(value["garment_vertices"], 3);
    assert_eq!(value["garment_triangles"], 1);
    assert_eq!(value["body_triangles"], 12);
    assert_eq!(value["inside_vertices"], 3);
    assert_eq!(value["contact_vertices"], 3);
    assert_eq!(value["body_pairs"], 0);
    assert_eq!(value["self_pairs"], 0);
    assert_eq!(value["depth"]["vertices"], 3);
    assert_eq!(value["worst"].as_array().unwrap().len(), 3);
    let max = value["depth"]["max"].as_f64().unwrap();
    assert!((max - 0.2).abs() < 1e-4, "max depth {max}");
    assert_eq!(value["above"][0]["count"], 3);
    assert_eq!(value["above"][2]["count"], 3);
    assert_eq!(value["above"][3]["count"], 0);
    assert_eq!(value["layers"]["visible"]["count"], 3);
    assert_eq!(value["layers"]["inner"]["count"], 0);
    assert!(
        value.get("verdict").is_none(),
        "no --tolerance means no verdict"
    );

    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let summary = String::from_utf8(text.stdout).unwrap();
    assert!(summary.contains("contact vertices 3 (inside 3)"));
    assert!(summary.contains("worst (index, depth): ("));
    assert!(
        summary.contains("layers: visible 3 (max 0.200000, mean 0.200000, p90 0.200000), inner 0"),
        "{summary}"
    );
    assert!(summary.contains("note: inside = behind the nearest body triangle"));
}

#[test]
fn tolerance_judges_the_visible_layer_and_lists_violations() {
    let directory = fixture_directory("visible");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, cube_obj()).unwrap();
    // Outward-wound triangle 0.2 behind the cube's +x face: a visible layer.
    fs::write(&garment, "v 0.8 0 0\nv 0.8 0.1 0\nv 0.8 0 0.1\nf 1 2 3\n").unwrap();

    let failing = depth_with(&body, &garment, true, Some(0.1));
    let passing = depth_with(&body, &garment, true, Some(0.3));
    let unfenced = depth(&body, &garment, true);
    let failing_text = depth_with(&body, &garment, false, Some(0.1));
    fs::remove_dir_all(&directory).unwrap();

    assert!(
        failing.status.success(),
        "a measurement exits 0 even on fail"
    );
    let value = json_of(&failing);
    assert_eq!(value["layers"]["visible"]["count"], 3);
    assert_eq!(value["layers"]["inner"]["count"], 0);
    let visible_max = value["layers"]["visible"]["max"].as_f64().unwrap();
    assert!(
        (visible_max - 0.2).abs() < 1e-4,
        "visible max {visible_max}"
    );
    assert_eq!(value["verdict"]["result"], "fail");
    assert_eq!(value["verdict"]["tolerance"], 0.1);
    assert_eq!(
        value["verdict"]["visible_max"],
        value["layers"]["visible"]["max"]
    );
    assert_eq!(value["verdict"]["violations"].as_array().unwrap().len(), 3);
    assert_eq!(value["verdict"]["violations"][0]["vertex"], 0);

    let value = json_of(&passing);
    assert_eq!(value["verdict"]["result"], "pass");
    assert!(
        value["verdict"]["violations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let value = json_of(&unfenced);
    assert!(
        value.get("verdict").is_none(),
        "no --tolerance means no verdict"
    );
    assert_eq!(value["layers"]["visible"]["count"], 3);

    let summary = String::from_utf8(failing_text.stdout).unwrap();
    assert!(
        summary.contains("layers: visible 3 (max 0.200000"),
        "{summary}"
    );
    assert!(summary.contains("inner 0 (max 0.000000"), "{summary}");
    assert!(
        summary.contains("verdict: fail (tolerance 0.100000, visible max 0.200000, 3 above)"),
        "{summary}"
    );
    assert!(
        summary.contains("violating visible vertices (index, depth): (0, 0.200000)"),
        "{summary}"
    );
}

#[test]
fn an_inner_layer_does_not_fail_a_visible_tolerance() {
    let directory = fixture_directory("inner");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, cube_obj()).unwrap();
    // The same triangle wound the other way: its normal faces into the cube.
    fs::write(&garment, "v 0.8 0 0\nv 0.8 0.1 0\nv 0.8 0 0.1\nf 1 3 2\n").unwrap();

    let machine = depth_with(&body, &garment, true, Some(0.05));
    let text = depth_with(&body, &garment, false, Some(0.05));
    fs::remove_dir_all(&directory).unwrap();

    let value = json_of(&machine);
    assert_eq!(value["inside_vertices"], 3);
    assert_eq!(value["layers"]["visible"]["count"], 0);
    assert_eq!(value["layers"]["inner"]["count"], 3);
    let inner_max = value["layers"]["inner"]["max"].as_f64().unwrap();
    assert!((inner_max - 0.2).abs() < 1e-4, "inner max {inner_max}");
    assert_eq!(value["verdict"]["result"], "pass");
    assert!(
        value["verdict"]["violations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let summary = String::from_utf8(text.stdout).unwrap();
    assert!(
        summary.contains("verdict: pass (tolerance 0.050000"),
        "{summary}"
    );
}

#[test]
fn a_clear_garment_passes_with_zero_visible_depth() {
    let directory = fixture_directory("clear");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, cube_obj()).unwrap();
    fs::write(&garment, "v 2 0 0\nv 2 0.1 0\nv 2 0 0.1\nf 1 2 3\n").unwrap();

    let machine = depth_with(&body, &garment, true, Some(0.0));
    let text = depth_with(&body, &garment, false, Some(0.0));
    fs::remove_dir_all(&directory).unwrap();

    let value = json_of(&machine);
    assert_eq!(value["contact_vertices"], 0);
    assert_eq!(value["inside_vertices"], 0);
    assert_eq!(value["layers"]["visible"]["max"], 0.0);
    assert_eq!(value["verdict"]["result"], "pass");
    assert!(
        value["verdict"]["violations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let summary = String::from_utf8(text.stdout).unwrap();
    assert!(
        summary.contains("verdict: pass (tolerance 0.000000"),
        "{summary}"
    );
}

#[test]
fn a_malformed_tolerance_is_refused_by_name() {
    let directory = fixture_directory("tolerance");
    let body = directory.join("body.obj");
    let garment = directory.join("garment.obj");
    fs::write(&body, cube_obj()).unwrap();
    fs::write(&garment, "v 0 0 0\nv 0 0.1 0\nv 0 0 0.1\nf 1 2 3\n").unwrap();

    let run = |tolerance: &str| {
        Command::new(env!("CARGO_BIN_EXE_sculpto"))
            .args(["garment", "depth", "--body"])
            .arg(&body)
            .arg("--garment")
            .arg(&garment)
            .arg("--tolerance")
            .arg(tolerance)
            .output()
            .unwrap()
    };
    let word = run("tight");
    let negative = run("-1");
    fs::remove_dir_all(&directory).unwrap();

    assert!(!word.status.success());
    assert!(
        String::from_utf8(word.stderr)
            .unwrap()
            .contains("`--tolerance` must be a number, got `tight`")
    );
    assert!(!negative.status.success());
    assert!(
        String::from_utf8(negative.stderr)
            .unwrap()
            .contains("`--tolerance` must be a finite non-negative number, got `-1`")
    );
}

#[test]
fn missing_arguments_and_bad_geometry_fail_by_name() {
    let directory = fixture_directory("bad");
    let body = directory.join("body.obj");
    let broken = directory.join("broken.obj");
    fs::write(&body, cube_obj()).unwrap();
    fs::write(&broken, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n").unwrap();

    let missing = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["garment", "depth", "--garment"])
        .arg(&broken)
        .output()
        .unwrap();
    let absent = depth(&directory.join("absent.obj"), &broken, false);
    let malformed = depth(&body, &broken, false);
    fs::remove_dir_all(&directory).unwrap();

    assert!(!missing.status.success());
    assert!(
        String::from_utf8(missing.stderr)
            .unwrap()
            .contains("`--body` is required")
    );
    assert!(!absent.status.success());
    assert!(
        String::from_utf8(absent.stderr)
            .unwrap()
            .contains("absent.obj")
    );
    assert!(!malformed.status.success());
    assert!(
        String::from_utf8(malformed.stderr)
            .unwrap()
            .contains("face references vertex 9")
    );
}

/// The RiddleMaster A0 fixture is private: point `SCULPTO_RIDDLE_MASTER_FIXTURE`
/// at the directory holding `body.obj` and `pull-robe.obj`. Without it the test
/// skips visibly, so an absent fixture is never read as a pass.
#[test]
fn pull_robe_visible_layer_passes_the_declared_tolerance() {
    let Ok(fixture) = std::env::var("SCULPTO_RIDDLE_MASTER_FIXTURE") else {
        eprintln!(
            "skipping: set SCULPTO_RIDDLE_MASTER_FIXTURE to the directory holding body.obj and pull-robe.obj"
        );
        return;
    };
    let directory = PathBuf::from(fixture);
    let body = directory.join("body.obj");
    let garment = directory.join("pull-robe.obj");
    if !body.exists() || !garment.exists() {
        eprintln!(
            "skipping: missing {} or {}",
            body.display(),
            garment.display()
        );
        return;
    }

    let machine = depth_with(&body, &garment, true, Some(0.15));
    let text = depth_with(&body, &garment, false, Some(0.15));
    let value = json_of(&machine);

    assert_eq!(value["verdict"]["result"], "pass");
    let visible_max = value["layers"]["visible"]["max"].as_f64().unwrap();
    assert!(
        (visible_max - 0.107).abs() < 0.002,
        "visible max {visible_max}"
    );
    assert!(
        value["layers"]["visible"]["count"].as_u64().unwrap() >= 1,
        "the pull robe must have a visible inside vertex"
    );
    let summary = String::from_utf8(text.stdout).unwrap();
    assert!(summary.contains("verdict: pass"), "{summary}");
}
