use std::fs;
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

fn depth(body: &std::path::Path, garment: &std::path::Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command
        .args(["garment", "depth", "--body"])
        .arg(body)
        .arg("--garment")
        .arg(garment);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

#[test]
fn json_and_text_report_the_measured_penetration() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("sculpto-depth-{}-{stamp}", std::process::id()));
    fs::create_dir(&directory).unwrap();
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

    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let summary = String::from_utf8(text.stdout).unwrap();
    assert!(summary.contains("contact vertices 3 (inside 3)"));
    assert!(summary.contains("worst (index, depth): ("));
    assert!(summary.contains("note: inside = behind the nearest body triangle"));
}

#[test]
fn missing_arguments_and_bad_geometry_fail_by_name() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("sculpto-depth-bad-{}-{stamp}", std::process::id()));
    fs::create_dir(&directory).unwrap();
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
