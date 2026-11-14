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

#[test]
fn missing_db_value_is_refused() {
    let root = temp_dir("db-missing");
    let output = Command::new(env!("CARGO_BIN_EXE_boethos"))
        .args(["list", "--db"])
        .env("XDG_STATE_HOME", &root)
        .output()
        .expect("run boethos");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--db requires a value"), "stderr: {stderr}");
    assert!(!root.join("boethos/assets.redb").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn equals_db_flag_is_honoured() {
    let root = temp_dir("equals");
    fs::write(root.join("asset.nif"), b"NIF").expect("write");
    let chosen = root.join("chosen.redb");
    let decoy = root.join("decoy.redb");
    let output = Command::new(env!("CARGO_BIN_EXE_boethos"))
        .arg(format!("--db={}", chosen.display()))
        .args([
            "index",
            "--root",
            root.to_str().expect("root"),
            "--no-archives",
        ])
        .env("BOETHOS_DB", &decoy)
        .output()
        .expect("run boethos");
    assert_eq!(output.status.code(), Some(0));
    assert!(chosen.is_file(), "chosen database was not created");
    assert!(!decoy.exists(), "decoy database was created");
    let _ = fs::remove_file(chosen);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unknown_options_and_stray_arguments_are_refused() {
    let root = temp_dir("strict");
    let db = root.with_extension("redb");
    for arguments in [
        vec!["list", "--bogus"],
        vec!["list", "stray"],
        vec!["list", "--category"],
        vec!["stats", "--db"],
    ] {
        let (code, stdout, stderr) = run(&db, &arguments);
        assert_eq!(code, 2, "{arguments:?} -> {stdout} {stderr}");
        assert!(!stdout.contains("assets"), "unexpected dump: {stdout}");
    }
    let (code, _, _) = run(&db, &["list", "--help"]);
    assert_eq!(code, 0);
    let (code, _, stderr) = run(&db, &["--version"]);
    assert_eq!(code, 0, "{stderr}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reads_do_not_create_a_missing_database() {
    let root = temp_dir("missing");
    let db = root.join("never.redb");
    let (code, stdout, stderr) = run(&db, &["stats"]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stderr.contains("database not indexed"), "{stderr}");
    assert!(!db.exists(), "read command created the database");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn list_filters_prefix_get_and_truncation() {
    let root = temp_dir("filters");
    fs::create_dir_all(root.join("SliderPresets")).expect("dirs");
    fs::write(root.join("SliderPresets/Bijin Body.xml"), b"same").expect("write");
    fs::write(root.join("SliderPresets/Pride Male.xml"), b"same").expect("write");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--source",
            "pack",
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");

    let (code, stdout, stderr) = run(&db, &["list", "--category", "preset", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(rows.as_array().expect("array").len(), 2);

    let (code, stdout, stderr) = run(&db, &["list", "--family", "bijin"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
    assert!(stdout.contains("Bijin Body.xml"));

    let (code, stdout, stderr) = run(&db, &["list", "--name", "pride", "--source", "pack"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
    assert!(stdout.contains("Pride Male.xml"));

    let (code, stdout, stderr) = run(&db, &["list", "--offset", "1", "--limit", "1"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("1 of 2 assets"), "{stdout}");

    let id = rows[0]["id"].as_str().expect("id");
    let prefix = &id[..12];
    let (code, stdout, stderr) = run(&db, &["get", prefix]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains(id), "{stdout}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn text_output_escapes_control_characters() {
    let root = temp_dir("escape");
    fs::write(root.join("tab\tnewline\nname.nif"), b"NIF").expect("write");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, stdout, stderr) = run(&db, &["list"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 2, "{stdout:?}");
    assert!(stdout.contains("tab\\tnewline\\nname.nif"), "{stdout:?}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn prune_removes_vanished_files() {
    let root = temp_dir("prune");
    fs::write(root.join("one.nif"), b"one").expect("write");
    fs::write(root.join("two.nif"), b"two").expect("write");
    let db = root.with_extension("redb");
    let arguments = [
        "index",
        "--root",
        root.to_str().expect("root"),
        "--no-archives",
    ];
    let (code, _, stderr) = run(&db, &arguments);
    assert_eq!(code, 0, "{stderr}");
    fs::remove_file(root.join("two.nif")).expect("remove");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--no-archives",
            "--prune",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("pruned 1"), "{stdout}");
    let (code, stdout, stderr) = run(&db, &["stats", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let stats: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(stats["total"], 1);
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn failed_index_is_a_refusal() {
    let root = temp_dir("failed");
    let file = root.join("unreadable.nif");
    fs::write(&file, b"NIF").expect("write");
    let mut permissions = fs::metadata(&file).expect("metadata").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o000);
    }
    fs::set_permissions(&file, permissions).expect("chmod");
    let db = root.with_extension("redb");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--no-archives",
        ],
    );
    let mut permissions = fs::metadata(&file).expect("metadata").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    fs::set_permissions(&file, permissions).expect("chmod");
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stderr.contains("wrote nothing"), "{stderr}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_removes_an_archive_and_its_members() {
    let root = temp_dir("delete");
    fs::create_dir_all(root.join("SliderPresets")).expect("dirs");
    fs::write(root.join("SliderPresets/Test.xml"), b"<SliderPresets/>").expect("write");
    fs::create_dir_all(root.join("FaceGeom/skyrim.esm")).expect("dirs");
    fs::write(root.join("FaceGeom/skyrim.esm/0001.NIF"), b"NIF").expect("write");
    let status = std::process::Command::new("7z")
        .current_dir(&root)
        .args([
            "a",
            "-y",
            "pack.7z",
            "SliderPresets/Test.xml",
            "FaceGeom/skyrim.esm/0001.NIF",
        ])
        .status()
        .expect("7z");
    assert!(status.success(), "7z could not create the fixture archive");
    fs::remove_dir_all(root.join("SliderPresets")).expect("remove");
    fs::remove_dir_all(root.join("FaceGeom")).expect("remove");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(&db, &["index", "--root", root.to_str().expect("root")]);
    assert_eq!(code, 0, "{stderr}");
    let (code, stdout, stderr) = run(&db, &["list", "--category", "archive", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let id = rows[0]["id"].as_str().expect("id");
    let (code, stdout, stderr) = run(&db, &["delete", id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("deleted 3 record(s)"), "{stdout}");
    let (code, stdout, stderr) = run(&db, &["stats", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let stats: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(stats["total"], 0);
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn delete_does_not_create_a_missing_database() {
    let root = temp_dir("delete-missing");
    let db = root.join("never.redb");
    let (code, stdout, stderr) = run(&db, &["delete", "deadbeef"]);
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stderr.contains("database not indexed"), "{stderr}");
    assert!(!db.exists(), "delete created the database");
    let _ = fs::remove_dir_all(root);
}
