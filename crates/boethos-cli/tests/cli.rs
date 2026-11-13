//! CLI contract: index, list, get, stats and export through the built binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "boethos-cli-{}-{label}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp dir");
    path
}

fn run(db: &Path, arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_boethos"))
        .arg("--db")
        .arg(db)
        .args(arguments)
        .output()
        .expect("run boethos");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn index_list_get_stats_export() {
    let root = temp_dir("root");
    fs::create_dir_all(root.join("SliderPresets")).expect("dirs");
    fs::write(root.join("SliderPresets/Amelia.xml"), b"<SliderPresets/>").expect("write");
    fs::create_dir_all(root.join("FaceGeom/skyrim.esm")).expect("dirs");
    fs::write(root.join("FaceGeom/skyrim.esm/0001.NIF"), b"NIF").expect("write");
    let db = root.with_extension("redb");

    let (code, stdout, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--no-archives",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(report["inserted"], 2);

    let (code, stdout, stderr) = run(&db, &["list", "--category", "preset", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(rows.as_array().expect("array").len(), 1);

    let id = rows[0]["id"].as_str().expect("id");
    let (code, stdout, stderr) = run(&db, &["get", "--json", id]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let asset: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(asset["category"], "preset");

    let (code, stdout, stderr) = run(&db, &["stats", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let stats: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(stats["total"], 2);

    let export = root.join("assets.jsonl");
    let (code, _, stderr) = run(&db, &["export", export.to_str().expect("export")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let lines = fs::read_to_string(&export).expect("lines");
    assert_eq!(lines.lines().count(), 2);
    let _ = fs::remove_file(&db);
    let _ = fs::remove_dir_all(root);
}
