//! `garment dress`: the argv surface, its JSON envelope and `run` parity.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

/// A square patch OBJ of `cells` × `cells` quads on `z`, written in the display
/// frame both surfaces read.
fn patch_obj(cells: usize, z: f32, half: f32) -> String {
    let step = 2.0 * half / cells as f32;
    let mut text = String::new();
    for row in 0..=cells {
        for column in 0..=cells {
            let x = -half + column as f32 * step;
            let y = -half + row as f32 * step;
            text.push_str(&format!("v {x} {y} {z}\n"));
        }
    }
    let stride = cells + 1;
    for row in 0..cells {
        for column in 0..cells {
            let corner = row * stride + column + 1;
            let next = corner + stride;
            text.push_str(&format!("f {corner} {} {next}\n", corner + 1));
            text.push_str(&format!("f {corner} {next} {}\n", next + 1));
        }
    }
    text
}

fn fixture_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "sculpto-dress-{label}-{}-{stamp}",
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

fn dress_argv(body: &Path, garment: &Path, out: &Path, extra: &[&str]) -> Output {
    let mut command = sculpto();
    command
        .args(["garment", "dress", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment)
        .arg("--out")
        .arg(out)
        .arg("--cuts")
        .arg("0")
        .arg("--rounds")
        .arg("0");
    command.args(extra);
    command.output().unwrap()
}

#[test]
fn argv_writes_a_dressed_obj_and_reports_the_band() {
    let directory = fixture_directory("argv");
    let body = write(&directory, "body.obj", &patch_obj(4, 0.0, 2.0));
    let garment = write(&directory, "garment.obj", &patch_obj(2, 1.0, 0.5));
    let out = directory.join("dressed.obj");
    let result = dress_argv(&body, &garment, &out, &["--clearance", "0.25", "--json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&result.stdout).expect("one JSON object on stdout");
    assert_eq!(report["vertices"], 9);
    assert_eq!(report["triangles"], 8);
    assert_eq!(report["clearance"], 0.25);
    assert_eq!(report["max_floating_gap"], 0.0);
    assert_eq!(report["max_penetration"], 0.0);
    assert_eq!(report["floating_before"], 9);
    assert_eq!(report["converged"], true);
    assert_eq!(report["out"], out.display().to_string());

    let written = fs::read_to_string(&out).expect("the dressed OBJ is written");
    let vertices = written
        .lines()
        .filter(|line| line.starts_with("v "))
        .count();
    let faces = written
        .lines()
        .filter(|line| line.starts_with("f "))
        .count();
    assert_eq!(
        (vertices, faces),
        (9, 8),
        "the report must describe the file"
    );
}

#[test]
fn argv_refuses_a_parameter_it_cannot_honour() {
    let directory = fixture_directory("refuse");
    let body = write(&directory, "body.obj", &patch_obj(2, 0.0, 1.0));
    let garment = write(&directory, "garment.obj", &patch_obj(1, 1.0, 0.5));
    let out = directory.join("dressed.obj");
    let result = dress_argv(&body, &garment, &out, &["--cuts", "9"]);
    assert!(
        !result.status.success(),
        "an impossible cut count must fail"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("cuts"),
        "the refusal names its parameter: {stderr}"
    );
    assert!(!out.exists(), "a refused dress writes no artifact");
}

#[test]
fn run_and_argv_produce_the_same_dressed_obj() {
    let directory = fixture_directory("parity");
    let body = write(&directory, "body.obj", &patch_obj(3, 0.0, 1.5));
    let garment = write(&directory, "garment.obj", &patch_obj(2, 0.8, 0.5));
    let argv_out = directory.join("argv.obj");
    let run_out = directory.join("run.obj");
    let result = dress_argv(&body, &garment, &argv_out, &["--clearance", "0.3"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let input = serde_json::json!({
        "body": body.display().to_string(),
        "garment": garment.display().to_string(),
        "out": run_out.display().to_string(),
        "clearance": 0.3,
        "cuts": 0,
        "rounds": 0,
    })
    .to_string();
    let run = sculpto()
        .args(["run", "--cmd", "outfit.dress", "--input", &input])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        fs::read_to_string(&argv_out).unwrap(),
        fs::read_to_string(&run_out).unwrap(),
        "both transports build the same parameters and write the same file"
    );
}
