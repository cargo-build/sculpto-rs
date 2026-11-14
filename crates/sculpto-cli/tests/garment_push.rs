//! `garment push`: the opt-in final push-out stage over saved geometry.

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
    write_obj_text(&positions, &faces)
}

/// One triangle `0.1` inside the cube behind its `y = 1` face, wound `+y`.
fn penetrating_obj() -> String {
    write_obj_text(
        &[(0.0, 0.9, 0.0), (0.0, 0.9, 0.2), (0.2, 0.9, 0.0)],
        &[[0, 1, 2]],
    )
}

fn write_obj_text(positions: &[(f32, f32, f32)], faces: &[[u32; 3]]) -> String {
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
        "sculpto-push-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir(&directory).unwrap();
    directory
}

fn write(directory: &Path, name: &str, text: &str) -> PathBuf {
    let path = directory.join(name);
    fs::write(&path, text).unwrap();
    path
}

fn push(
    body: &Path,
    garment: &Path,
    out: &Path,
    tolerance: Option<&str>,
    passes: &str,
    json: bool,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command
        .args(["garment", "push", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment)
        .arg("--out")
        .arg(out)
        .arg("--passes")
        .arg(passes);
    if let Some(tolerance) = tolerance {
        command.arg("--tolerance").arg(tolerance);
    }
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

fn depth(body: &Path, garment: &Path, tolerance: f32) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["garment", "depth", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment)
        .arg("--tolerance")
        .arg(tolerance.to_string())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The graded maximum `garment depth` reports, in the one measurement both
/// commands share.
fn depth_max(body: &Path, garment: &Path, tolerance: f32) -> f64 {
    let output = Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(["garment", "depth", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment)
        .arg("--tolerance")
        .arg(tolerance.to_string())
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    payload["layers"]["visible_occluded"]["max"]
        .as_f64()
        .unwrap()
}

/// The frame defect on a frame-sensitive pair: an exact rotation of the same
/// geometry grades differently, so a pass that graded a converted frame used to
/// report a maximum its own output could not reproduce. Opt in with both
/// fixture paths; without them the assertion is vacuous and says so.
#[test]
fn a_frame_sensitive_pair_reports_the_maximum_its_output_reproduces() {
    let (Some(garment), Some(body)) = (
        std::env::var_os("SCULPTO_FRAME_GARMENT"),
        std::env::var_os("SCULPTO_FRAME_BODY"),
    ) else {
        eprintln!(
            "SKIP a_frame_sensitive_pair_reports_the_maximum_its_output_reproduces: missing SCULPTO_FRAME_GARMENT and SCULPTO_FRAME_BODY"
        );
        return;
    };
    let garment = PathBuf::from(garment);
    let body = PathBuf::from(body);
    let directory = fixture_directory("frame-pair");
    let out = directory.join("pushed.obj");
    let tolerance = 0.15_f32;

    let output = push(&body, &garment, &out, Some("0.15"), "10", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let before = depth_max(&body, &garment, tolerance);
    let after = depth_max(&body, &out, tolerance);
    assert!(
        (report["before_max"].as_f64().unwrap() - before).abs() < 1e-6,
        "reported before {} vs depth {before}",
        report["before_max"]
    );
    assert!(
        (report["max"].as_f64().unwrap() - after).abs() < 1e-6,
        "reported max {} vs depth {after}",
        report["max"]
    );
    assert_eq!(report["violations"], 0);
    assert!(after < f64::from(tolerance), "{after}");
}

#[test]
fn the_push_closes_a_visible_violation_and_the_regrade_passes() {
    let directory = fixture_directory("closes");
    let body = write(&directory, "body.obj", &cube_obj());
    let garment = write(&directory, "garment.obj", &penetrating_obj());
    let out = directory.join("pushed.obj");

    let output = push(&body, &garment, &out, Some("0.05"), "3", false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("visible violations 3 (max 0.100000) at tolerance 0.050000 -> 0 (max 0.049900) after 1 passes"),
        "{text}"
    );
    assert!(
        text.contains("pass 1: moved 3, violations 0, max 0.049900"),
        "{text}"
    );

    let regrade = depth(&body, &out, 0.05);
    assert!(
        regrade.contains("verdict: pass (tolerance 0.050000, visible max 0.049900)"),
        "{regrade}"
    );
}

/// The frame contract: the pass grades the coordinates it writes, so its
/// `before_max` is `garment depth`'s maximum on the input and its `max` is that
/// command's maximum on its own output. Grading a converted frame would report
/// numbers the written file cannot reproduce, because the depth predicate
/// resolves one nearest body triangle and its near ties break by traversal
/// order.
#[test]
fn the_report_repeats_the_depth_regrade() {
    let directory = fixture_directory("frame");
    let body = write(&directory, "body.obj", &cube_obj());
    let garment = write(&directory, "garment.obj", &penetrating_obj());
    let out = directory.join("pushed.obj");

    let output = push(&body, &garment, &out, Some("0.05"), "3", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let before = depth_max(&body, &garment, 0.05);
    let after = depth_max(&body, &out, 0.05);
    let reported_before = report["before_max"].as_f64().unwrap();
    let reported_after = report["max"].as_f64().unwrap();
    assert!(
        (reported_before - before).abs() < 1e-6,
        "reported before {reported_before} vs depth {before}"
    );
    assert!(
        (reported_after - after).abs() < 1e-6,
        "reported max {reported_after} vs depth {after}"
    );
    assert!(after < 0.05, "{after}");
}

#[test]
fn json_carries_the_per_pass_record() {
    let directory = fixture_directory("json");
    let body = write(&directory, "body.obj", &cube_obj());
    let garment = write(&directory, "garment.obj", &penetrating_obj());
    let out = directory.join("pushed.obj");

    let output = push(&body, &garment, &out, Some("0.05"), "2", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(payload["units"], "source");
    assert_eq!(payload["budget"], 2);
    assert_eq!(payload["passes"], 1);
    assert_eq!(payload["before_violations"], 3);
    assert_eq!(payload["before_max"], 0.1);
    assert_eq!(payload["violations"], 0);
    assert_eq!(payload["max"], 0.0499);
    assert_eq!(payload["held"], 0);
    assert_eq!(payload["per_pass"][0]["moved"], 3);
    assert_eq!(payload["per_pass"][0]["violations"], 0);
}

#[test]
fn a_missing_tolerance_or_a_bad_budget_is_refused_by_name() {
    let directory = fixture_directory("refused");
    let body = write(&directory, "body.obj", &cube_obj());
    let garment = write(&directory, "garment.obj", &penetrating_obj());
    let out = directory.join("pushed.obj");

    let missing = push(&body, &garment, &out, None, "1", false);
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("`--tolerance` is required"),
        "{}",
        String::from_utf8_lossy(&missing.stderr)
    );
    assert!(!out.exists());

    let zero = push(&body, &garment, &out, Some("0.05"), "0", false);
    assert!(!zero.status.success());
    let stderr = String::from_utf8_lossy(&zero.stderr);
    assert!(
        stderr.contains("the final push pass count 0 is outside 1..=10"),
        "{stderr}"
    );
    assert!(!out.exists());
}
