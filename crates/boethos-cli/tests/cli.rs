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
fn face_classification_validates_targets_and_survives_reindex() {
    let root = temp_dir("face-classification");
    fs::write(root.join("character.jslot"), "{}").expect("preset");
    fs::write(root.join("reference.png"), []).expect("image");
    let db = root.with_extension("redb");
    let index = ["index", "--root", root.to_str().expect("root")];
    let (code, _, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");
    let (code, records, stderr) = run(&db, &["list", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let records: Vec<serde_json::Value> = serde_json::from_str(&records).expect("assets");
    let asset_id = |category| {
        records
            .iter()
            .find(|asset| asset["category"] == category)
            .expect("category")["id"]
            .as_str()
            .expect("id")
    };
    let id = asset_id("face_preset");
    let (code, _, stderr) = run(&db, &["favorite", id]);
    assert_eq!(code, 0, "{stderr}");
    let face = |asset_id, hair_color| {
        serde_json::json!({
            "id": asset_id,
            "sex": "female",
            "race": "nord",
            "hair_color": hair_color,
            "hairstyle": "braid",
            "hair_source": "ks-hairdos",
            "hairdo": "sample-braid",
        })
    };
    let manifest = root.with_extension("jsonl");
    let classify = [
        "classify-faces",
        "--manifest",
        manifest.to_str().expect("manifest"),
        "--json",
    ];
    fs::write(
        &manifest,
        format!(
            "{}\n{}\n",
            face(id, "dark"),
            face(asset_id("image"), "dark")
        ),
    )
    .expect("bad manifest");
    let (code, _, stderr) = run(&db, &classify);
    assert_eq!(code, 2);
    assert!(stderr.contains("expected face or face_preset"), "{stderr}");
    let (code, initial, stderr) = run(&db, &["get", id, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!initial.contains("race:nord"));

    fs::write(&manifest, format!("{}\n", face(id, "dark"))).expect("manifest");
    let (code, _, stderr) = run(&db, &classify);
    assert_eq!(code, 0, "{stderr}");
    fs::write(&manifest, format!("{}\n", face(id, "light"))).expect("new labels");
    let (code, _, stderr) = run(&db, &classify);
    assert_eq!(code, 0, "{stderr}");
    let (code, _, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");
    let (code, classified, stderr) = run(
        &db,
        &[
            "list",
            "--sex",
            "female",
            "--tag",
            "race:nord",
            "--tag",
            "favorite",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let classified: Vec<serde_json::Value> = serde_json::from_str(&classified).expect("assets");
    assert_eq!(classified.len(), 1);
    assert_eq!(classified[0]["id"], id);
    let tags = classified[0]["tags"].as_array().expect("tags");
    assert!(tags.iter().any(|tag| tag == "hair-color:light"));
    assert!(tags.iter().any(|tag| tag == "hairstyle:braid"));
    assert!(tags.iter().any(|tag| tag == "hair-source:ks-hairdos"));
    assert!(!tags.iter().any(|tag| tag == "hair-color:dark"));
    let (code, report, stderr) = run(&db, &classify);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&report).expect("report");
    assert_eq!(report["unchanged"], 1);
    let _ = fs::remove_file(manifest);
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
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
fn favorite_survives_reindex_and_can_be_removed() {
    let root = temp_dir("favorite");
    let preset = root.join("SliderPresets/Emily.xml");
    fs::create_dir_all(root.join("SliderPresets")).expect("preset directory");
    fs::write(&preset, "<SliderPresets/>").expect("preset");
    let db = root.with_extension("redb");
    let index = ["index", "--root", root.to_str().expect("root"), "--json"];
    let (code, _, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");

    let (code, output, stderr) = run(&db, &["list", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let assets: serde_json::Value = serde_json::from_str(&output).expect("assets");
    let id = assets[0]["id"].as_str().expect("id");
    for requested in [&id[..8], id] {
        let (code, _, stderr) = run(&db, &["favorite", requested]);
        assert_eq!(code, 0, "{stderr}");
    }

    fs::write(
        &preset,
        "<SliderPresets><Preset name=\"Emily\"/></SliderPresets>",
    )
    .expect("updated preset");
    let (code, output, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");
    let report: serde_json::Value = serde_json::from_str(&output).expect("index report");
    assert_eq!(report["updated"], 1);

    let favorites = ["list", "--tag", "favorite", "--json"];
    let (code, output, stderr) = run(&db, &favorites);
    assert_eq!(code, 0, "{stderr}");
    let assets: serde_json::Value = serde_json::from_str(&output).expect("favorites");
    assert_eq!(assets[0]["id"], id);
    assert_eq!(assets[0]["tags"], serde_json::json!(["favorite"]));

    for _ in 0..2 {
        let (code, _, stderr) = run(&db, &["favorite", id, "--remove"]);
        assert_eq!(code, 0, "{stderr}");
    }
    let (code, output, stderr) = run(&db, &favorites);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(output.trim(), "[]");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn preview_links_the_selected_preset_and_outfit_across_reindex() {
    let root = temp_dir("preset-previews");
    let presets = root.join("SliderPresets");
    fs::create_dir_all(&presets).expect("preset directory");
    fs::write(presets.join("First.xml"), "<SliderPresets/>").expect("first");
    fs::write(presets.join("Second.xml"), "<SliderPresets/>").expect("second");
    let outfits = root.join("outfits");
    fs::create_dir_all(&outfits).expect("outfit directory");
    fs::write(outfits.join("Cuirass.nif"), b"NIF").expect("outfit fixture");
    fs::write(root.join("Review.png"), b"image").expect("image fixture");
    fs::write(root.join("Cuirass.png"), b"outfit image").expect("outfit image fixture");
    let db = root.with_extension("redb");
    let index = ["index", "--root", root.to_str().expect("root")];
    let (code, _, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");

    let (code, output, stderr) = run(&db, &["list", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let rows: Vec<serde_json::Value> = serde_json::from_str(&output).expect("assets");
    let id = |name| {
        rows.iter()
            .find(|asset| asset["name"] == name)
            .expect("asset")["id"]
            .as_str()
            .expect("id")
    };
    let first = id("First.xml");
    let second = id("Second.xml");
    let image = id("Review.png");
    let cuirass = id("Cuirass.nif");
    let cuirass_image = id("Cuirass.png");
    let (code, _, stderr) = run(&db, &["preview", first, second]);
    assert_eq!(code, 2);
    assert!(stderr.contains("expected image"), "{stderr}");
    let (code, _, stderr) = run(&db, &["preview", image, cuirass_image]);
    assert_eq!(code, 2);
    assert!(stderr.contains("expected preset or outfit"), "{stderr}");
    let links = [(first, image), (cuirass, cuirass_image)];
    for (subject, image) in links {
        for _ in 0..2 {
            let (code, output, stderr) = run(&db, &["preview", &subject[..8], image, "--json"]);
            assert_eq!(code, 0, "{stderr}");
            let previews: Vec<serde_json::Value> = serde_json::from_str(&output).expect("previews");
            assert_eq!(previews.len(), 1);
            assert_eq!(previews[0]["id"], image);
        }
    }
    let (code, _, stderr) = run(&db, &index);
    assert_eq!(code, 0, "{stderr}");
    for (subject, image) in links {
        let (code, output, stderr) = run(&db, &["preview", subject, "--json"]);
        assert_eq!(code, 0, "{stderr}");
        let previews: Vec<serde_json::Value> = serde_json::from_str(&output).expect("previews");
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0]["id"], image);
    }
    let (code, output, stderr) = run(&db, &["preview", second, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(output.trim(), "[]");
    let _ = fs::remove_file(db);
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

#[test]
fn garment_tags_filter_extracted_underwear() {
    let root = temp_dir("garment-tags");
    fs::write(root.join("female-cbbe-bra.obj"), b"bra").expect("write");
    fs::write(root.join("female-3ba-panty.obj"), b"panty").expect("write");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            root.to_str().expect("root"),
            "--source",
            "underwear-extract",
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, stdout, stderr) = run(&db, &["list", "--category", "outfit"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 3, "{stdout}");
    let (code, stdout, stderr) = run(
        &db,
        &["list", "--tag", "wear:underwear", "--tag", "sex:female"],
    );
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 3, "{stdout}");
    assert!(stdout.contains("female-cbbe-bra.obj"), "{stdout}");
    assert!(stdout.contains("female-3ba-panty.obj"), "{stdout}");
    let (code, stdout, stderr) = run(&db, &["list", "--tag", "wear:bottom", "--tag", "fit:tight"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout.lines().count(), 2, "{stdout}");
    assert!(stdout.contains("female-3ba-panty.obj"), "{stdout}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn imports_body_part_manifest() {
    let root = temp_dir("body-parts");
    let manifest = root.join("parts.jsonl");
    fs::write(
        &manifest,
        concat!(
            "{\"name\":\"Feet\",\"shape\":\"Feet\",\"sex\":\"female\",\"family\":\"cbbe-3ba\",\"source\":\"from-skyrim-community\",\"archive\":\"/assets/pack.7z\",\"entry\":\"meshes/femalefeet_0.nif\",\"vertices\":8876,\"triangles\":16288}\n",
            "{\"name\":\"Hands\",\"shape\":\"Hands\",\"sex\":\"female\",\"family\":\"cbbe-3ba\",\"source\":\"from-skyrim-community\",\"archive\":\"/assets/pack.7z\",\"entry\":\"meshes/femalehands_0.nif\",\"vertices\":6374,\"triangles\":11776}\n"
        ),
    )
    .expect("manifest");
    let empty = root.join("empty");
    fs::create_dir_all(&empty).expect("empty root");
    let db = root.with_extension("redb");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            empty.to_str().expect("empty"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    assert!(stdout.contains("inserted 2"), "{stdout}");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    assert!(stdout.contains("unchanged 2"), "{stdout}");
    let (code, stdout, stderr) = run(&db, &["list", "--category", "body-part", "--name", "feet"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("femalefeet_0.nif"), "{stdout}");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "list",
            "--category",
            "body-part",
            "--sex",
            "female",
            "--family",
            "cbbe-3ba",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(rows.as_array().expect("array").len(), 2);
    assert_eq!(rows[0]["vertices"], 8876);
    assert!(
        rows[0]["tags"]
            .as_array()
            .expect("tags")
            .iter()
            .any(|tag| tag == "part:feet")
    );
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn refuses_malformed_body_part_manifest_before_writing() {
    let root = temp_dir("bad-parts");
    let manifest = root.join("parts.jsonl");
    fs::write(
        &manifest,
        concat!(
            "{\"name\":\"Feet\",\"shape\":\"Feet\",\"source\":\"test\",\"path\":\"/assets/feet.nif\",\"vertices\":1,\"triangles\":1}\n",
            "{\"name\":\"Hands\",\"shape\":\"Hands\",\"source\":\"test\",\"entry\":\"meshes/hands.nif\",\"vertices\":1,\"triangles\":1}\n"
        ),
    )
    .expect("manifest");
    let db = root.with_extension("redb");
    let _ = fs::remove_file(&db);
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 2, "{stdout} {stderr}");
    assert!(stderr.contains("parts.jsonl:2"), "{stderr}");
    assert!(!db.exists(), "pre-parse must not open the store");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn body_part_import_refuses_missing_database_and_empty_manifest() {
    let root = temp_dir("parts-contract");
    let empty_manifest = root.join("empty.jsonl");
    fs::write(&empty_manifest, "").expect("empty manifest");
    let valid_manifest = root.join("valid.jsonl");
    fs::write(
        &valid_manifest,
        "{\"name\":\"Feet\",\"shape\":\"Feet\",\"source\":\"test\",\"path\":\"/assets/feet.nif\",\"vertices\":1,\"triangles\":1}\n",
    )
    .expect("valid manifest");
    let db = root.with_extension("redb");
    let _ = fs::remove_file(&db);
    let (code, _, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            valid_manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("database not indexed"), "{stderr}");
    assert!(!db.exists(), "missing database must not be created");

    let empty = root.join("empty");
    fs::create_dir_all(&empty).expect("empty root");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            empty.to_str().expect("empty"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, _, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            empty_manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no body-part records"), "{stderr}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn body_part_import_reports_duplicate_and_remeasured_rows() {
    let root = temp_dir("parts-duplicates");
    let manifest = root.join("parts.jsonl");
    let line = "{\"name\":\"Feet\",\"shape\":\"Feet\",\"source\":\"test\",\"path\":\"/assets/feet.nif\",\"vertices\":8876,\"triangles\":16288}\n";
    fs::write(&manifest, format!("{line}{line}")).expect("manifest");
    let empty = root.join("empty");
    fs::create_dir_all(&empty).expect("empty root");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            empty.to_str().expect("empty"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    assert!(
        stdout.contains("inserted 1") && stdout.contains("unchanged 1"),
        "{stdout}"
    );
    fs::write(
        &manifest,
        "{\"name\":\"Feet\",\"shape\":\"Feet\",\"source\":\"test\",\"path\":\"/assets/feet.nif\",\"vertices\":8877,\"triangles\":16288}\n",
    )
    .expect("manifest");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    assert!(stdout.contains("updated 1"), "{stdout}");
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn imports_raw_part_manifest() {
    let root = temp_dir("raw-parts");
    let manifest = root.join("parts.jsonl");
    fs::write(
        &manifest,
        "{\"name\":\"female-mannequin-figure-01\",\"part\":\"figure-01\",\"category\":\"mesh\",\"source\":\"tripo3d-parts\",\"path\":\"/parts/female-mannequin-figure-01.glb\",\"source_sha256\":\"597dfcf0\",\"vertices\":197966,\"triangles\":383838,\"tags\":[\"figure:01\"]}\n",
    )
    .expect("manifest");
    let empty = root.join("empty");
    fs::create_dir_all(&empty).expect("empty root");
    let db = root.with_extension("redb");
    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--root",
            empty.to_str().expect("empty"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 0, "{stderr}");
    let (code, stdout, stderr) = run(
        &db,
        &[
            "import-raw-parts",
            "--manifest",
            manifest.to_str().expect("manifest"),
        ],
    );
    assert_eq!(code, 0, "{stdout} {stderr}");
    assert!(stdout.contains("inserted 1"), "{stdout}");
    let (code, stdout, stderr) = run(
        &db,
        &["list", "--category", "mesh", "--tag", "raw:part", "--json"],
    );
    assert_eq!(code, 0, "{stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let row = &rows.as_array().expect("array")[0];
    assert_eq!(row["shape"], "figure-01");
    assert_eq!(row["source_sha256"], "597dfcf0");
    assert_eq!(row["vertices"], 197966);
    let _ = fs::remove_file(db);
    let _ = fs::remove_dir_all(root);
}

fn actor_glb() -> Vec<u8> {
    let json = r#"{
        "nodes": [{"name": "Root"}, {"name": "Hip"}, {"name": "Spine"}],
        "skins": [{"joints": [1, 2]}],
        "accessors": [
            {"count": 3, "max": [1.5]},
            {"count": 3, "max": [1.5]},
            {"count": 2, "max": [0.5]}
        ],
        "animations": [
            {
                "name": "Walk",
                "channels": [
                    {"sampler": 0, "target": {"node": 1, "path": "rotation"}},
                    {"sampler": 1, "target": {"node": 2, "path": "translation"}}
                ],
                "samplers": [{"input": 0, "output": 0}, {"input": 1, "output": 2}]
            },
            {
                "name": "RootOnly",
                "channels": [{"sampler": 0, "target": {"node": 0, "path": "translation"}}],
                "samplers": [{"input": 2, "output": 0}]
            }
        ]
    }"#;
    let mut chunk = json.as_bytes().to_vec();
    while !chunk.len().is_multiple_of(4) {
        chunk.push(b' ');
    }
    let length = 12 + 8 + chunk.len();
    let mut bytes = Vec::with_capacity(length);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(length as u32).to_le_bytes());
    bytes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    bytes.extend_from_slice(&chunk);
    bytes
}

#[test]
fn clips_reads_animations_without_a_database_and_a_category_lists_them() {
    let root = temp_dir("clips");
    let actor = root.join("actor.glb");
    fs::write(&actor, actor_glb()).expect("write");
    let db = root.with_extension("redb");

    let (code, stdout, stderr) = run(
        &db,
        &["clips", "--glb", actor.to_str().expect("path"), "--json"],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["palette_joints"], 2);
    let clips = document["clips"].as_array().expect("clips");
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0]["name"], "Walk");
    assert_eq!(clips[0]["channels"], 2);
    assert_eq!(clips[0]["keyframes"], 3);
    assert_eq!(clips[0]["duration_seconds"], 1.5);
    assert_eq!(clips[0]["bones"], serde_json::json!(["Hip", "Spine"]));
    assert_eq!(clips[0]["palette_coverage"], 1.0);
    assert!(!db.exists(), "clips must not create a database");

    let (code, stdout, stderr) = run(&db, &["clips", "--glb", actor.to_str().expect("path")]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(
        stdout.contains("2 clips, 2 palette joints"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("Walk\tchannels=2\tkeys=3"),
        "stdout: {stdout}"
    );

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
    assert_eq!(report["clips"], 2);
    assert_eq!(report["inserted"], 3);

    let (code, stdout, stderr) = run(&db, &["list", "--category", "clip", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 2);
    let walk = rows
        .iter()
        .find(|row| row["name"] == "Walk")
        .expect("Walk row");
    assert_eq!(walk["clip"]["channels"], 2);

    let id = walk["id"].as_str().expect("id");
    let (code, stdout, stderr) = run(&db, &["get", id]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(stdout.contains("channels: 2"), "stdout: {stdout}");
    assert!(
        stdout.contains("palette:  100.0% of 2 skin joints"),
        "stdout: {stdout}"
    );

    fs::write(root.join("broken.glb"), b"not a glb").expect("write");
    let (code, _, stderr) = run(
        &db,
        &[
            "clips",
            "--glb",
            root.join("broken.glb").to_str().expect("path"),
        ],
    );
    assert_eq!(code, 2);
    assert!(stderr.contains("glb"), "stderr: {stderr}");

    let (code, _, stderr) = run(&db, &["clips"]);
    assert_eq!(code, 2);
    assert!(
        stderr.contains("clips requires --glb PATH"),
        "stderr: {stderr}"
    );
    let _ = fs::remove_file(&db);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn index_file_needs_a_source_and_indexes_only_the_named_path() {
    let root = temp_dir("single-file");
    let keep = root.join("keep.nif");
    let skip = root.join("skip.nif");
    fs::write(&keep, b"KEEP").expect("keep");
    fs::write(&skip, b"SKIP").expect("skip");
    let db = root.with_extension("redb");

    let (code, _, stderr) = run(
        &db,
        &[
            "index",
            "--file",
            keep.to_str().expect("keep"),
            "--no-archives",
        ],
    );
    assert_eq!(code, 2);
    assert!(
        stderr.contains("--file requires --source"),
        "stderr: {stderr}"
    );

    let (code, stdout, stderr) = run(
        &db,
        &[
            "index",
            "--file",
            keep.to_str().expect("keep"),
            "--source",
            "downloads",
            "--no-archives",
            "--json",
        ],
    );
    assert_eq!(code, 0, "stderr: {stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(report["files"], 1);
    assert_eq!(report["inserted"], 1);
    assert_eq!(report["paths"][0], keep.to_str().expect("keep"));

    let (code, stdout, stderr) = run(&db, &["list", "--json"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let rows: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "keep.nif");
    assert_eq!(rows[0]["source"], "downloads");
    let _ = fs::remove_file(&db);
    let _ = fs::remove_dir_all(root);
}
