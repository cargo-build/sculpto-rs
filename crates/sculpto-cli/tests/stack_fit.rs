//! The `outfit stack-fit` argv surface and the registry row behind it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

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

fn sheet_obj(x: f32) -> String {
    format!("v {x} 0 0\nv {x} 0.1 0\nv {x} 0 0.1\nf 1 2 3\n")
}

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-cli-stack-fit-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

/// Write `layers.json` and run `outfit stack-fit` with `extra` arguments.
fn stack_fit(directory: &Path, manifest: &str, extra: &[&str]) -> Output {
    let path = directory.join("layers.json");
    fs::write(&path, manifest).unwrap();
    let mut command = sculpto();
    command
        .args(["outfit", "stack-fit", "--outfit"])
        .arg(&path)
        .args(extra);
    command.output().unwrap()
}

fn json_of(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn the_registry_lists_the_command_and_run_dispatches_it() {
    let listed = sculpto().args(["commands", "--json"]).output().unwrap();
    let value: Value = serde_json::from_slice(&listed.stdout).unwrap();
    let row = value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "outfit.stack_fit")
        .expect("the registry lists outfit.stack_fit");
    assert_eq!(row["implemented"], true);
    assert_eq!(row["mutates"], true);

    let envelope = sculpto()
        .args(["run", "--cmd", "outfit.stack_fit"])
        .output()
        .unwrap();
    let value: Value = serde_json::from_slice(&envelope.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["kind"], "invalid_params");
    assert_eq!(value["error"]["code"], "missing_field");
}

#[test]
fn the_argv_surface_reports_provenance_and_breakdown() {
    let directory = fixture_directory("argv");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("cloak.obj"), sheet_obj(0.44)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "robe", "order": 40, "obj": "robe.obj"},
            {"id": "cloak", "order": 60, "obj": "cloak.obj"}
        ]}"#;

    let output = stack_fit(&directory, manifest, &["--tolerance", "0.02", "--json"]);
    let report = json_of(&output);
    fs::remove_dir_all(&directory).unwrap();

    assert_eq!(report["units"], "source");
    assert_eq!(report["tolerance"], 0.02);
    assert_eq!(report["passes"], 4, "an undeclared budget defaults to 4");
    let cloak = report["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["layer"] == "cloak")
        .unwrap();
    assert_eq!(cloak["obstacle"]["surface"], "accumulated[body, robe]");
    assert_eq!(cloak["contact_breakdown"], json!({"body": 0, "robe": 3}));
    assert_eq!(cloak["fit"]["after_violations"], 0);
    assert_eq!(report["pass"], true);

    // Text output carries the same rows.
    let directory = fixture_directory("argv-text");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("cloak.obj"), sheet_obj(0.44)).unwrap();
    let output = stack_fit(&directory, manifest, &["--tolerance", "0.02"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    fs::remove_dir_all(&directory).unwrap();
    assert!(
        text.contains("obstacle accumulated[body, robe]"),
        "the text report names the obstacle: {text}"
    );
    assert!(text.contains("graded contact: body 0, robe 3"), "{text}");
    assert!(text.contains("verdict: pass"), "{text}");
}

#[test]
fn a_failing_stack_is_a_reported_verdict_not_an_error_exit() {
    // A fitted stack that fits: every layer pushes to just inside its contact
    // member, and the pair verdict passes.
    let directory = fixture_directory("verdict-pass");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("robe.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("cloak.obj"), sheet_obj(0.44)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "robe", "order": 40, "obj": "robe.obj"},
            {"id": "cloak", "order": 60, "obj": "cloak.obj"}
        ]}"#;
    let output = stack_fit(
        &directory,
        manifest,
        &["--tolerance", "0.02", "--passes", "1", "--json"],
    );
    assert!(output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["pass"], true, "{report}");
    fs::remove_dir_all(&directory).unwrap();

    // A declared boot mode the geometry contradicts: trousers (0.5) sit inside
    // the boots (0.55), which `under-trousers` forbids, and no push repairs it
    // because nothing penetrates. The verdict fails, the exit code stays 0.
    let directory = fixture_directory("verdict-fail");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    fs::write(directory.join("trousers.obj"), cube_obj(0.5)).unwrap();
    fs::write(directory.join("boots.obj"), cube_obj(0.55)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [
            {"id": "trousers", "order": 2, "obj": "trousers.obj"},
            {"id": "boots", "order": 7, "obj": "boots.obj",
             "mode": "under-trousers", "pair": "trousers"}
        ]}"#;
    let output = stack_fit(&directory, manifest, &["--tolerance", "0.02", "--json"]);
    assert!(
        output.status.success(),
        "a failed verdict is a report, not an error exit: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["pass"], false, "{report}");
    assert_eq!(report["failures"]["boot_mode_violation"], 1);
    fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn refused_runs_exit_nonzero_and_name_the_code() {
    let directory = fixture_directory("refused");
    fs::write(directory.join("body.obj"), cube_obj(0.4)).unwrap();
    let manifest = r#"{"body": "body.obj",
        "layers": [{"id": "robe", "order": 40, "obj": "ghost.obj"}]}"#;
    let output = stack_fit(&directory, manifest, &["--tolerance", "0.05"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("stack_fit_layer_missing"),
        "the typed code reaches the operator: {stderr}"
    );

    let missing_tolerance = stack_fit(&directory, manifest, &[]);
    assert_eq!(missing_tolerance.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&missing_tolerance.stderr).contains("--tolerance"),
        "tolerance is required: no default acceptance"
    );
    fs::remove_dir_all(&directory).unwrap();
}
