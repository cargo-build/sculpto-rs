//! A01 acceptance: the argv surface and the command surface must produce the
//! same artifacts *and* the same numbers.
//!
//! Two tiers. The committed fixture under `tests/fixtures/tier1` needs no host,
//! so `bake` and `export.obj` are compared unconditionally; the real HIMBO body
//! lives in a private BodySlide home and stays env-gated (`SCULPTO_BODYSLIDE_HOME`,
//! tier 2 in `doc/test-tiers.toml`). `outfit.fit` only has the host-gated case:
//! it reads a body and a garment shape from one NIF, and the writer still emits
//! one shape per file.
//!
//! The fixture's own bytes are pinned to their generator: the NIF comes from
//! `sculpto_nif::write::se_shape::write_se_shape` and the `.osd` from the layout
//! documented in `sculpto_core::shape_data`, so a stale blob fails instead of
//! silently feeding a test. Regenerate with `FIXTURE_UPDATE=1`.

use std::path::{Path, PathBuf};
use std::process::Command;

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::se_shape::write_se_shape;

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("SCULPTO_BODYSLIDE_HOME").map(PathBuf::from)
}

/// The portable fixture root; its layout is a BodySlide home.
fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tier1")
}

fn work_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sculpto-a01-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn run_ok(command: &mut Command) -> String {
    let output = command.output().expect("spawn");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is UTF-8")
}

/// The `result` member of the one envelope `sculpto run` prints.
fn dispatched(stdout: &str) -> serde_json::Value {
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("one JSON envelope on stdout");
    assert_eq!(value["ok"], true, "the command failed: {value}");
    value["result"].clone()
}

/// The Tier-1 source shape: a unit quad with UVs and normals.
fn fixture_asset() -> MeshAsset {
    MeshAsset {
        name: "Tier1 Body".into(),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        triangle_partitions: vec![PartitionId(0), PartitionId(0)],
        ..MeshAsset::default()
    }
}

/// The Tier-1 morph container: one block, two offsets, the layout
/// `sculpto_core::shape_data` decodes. There is no container writer yet, so the
/// test emits the documented bytes.
fn container_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x00DSO");
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    let name = b"Tier1Widen";
    bytes.push(u8::try_from(name.len()).expect("name fits"));
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    for (index, delta) in [(1_u16, [0.5_f32, 0.0, 0.0]), (2_u16, [1.0, 0.0, 0.0])] {
        bytes.extend_from_slice(&index.to_le_bytes());
        for component in delta {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    bytes
}

#[test]
fn tier1_fixture_matches_its_generator() {
    let home = fixture_home();
    let generated = [
        (
            "ShapeData/Tier1/Body.nif",
            write_se_shape(&fixture_asset()).expect("SE writes"),
        ),
        ("ShapeData/Tier1/Body.osd", container_bytes()),
    ];
    let update = std::env::var_os("FIXTURE_UPDATE").is_some();
    for (relative, bytes) in generated {
        let path = home.join(relative);
        if update {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
            std::fs::write(&path, &bytes).expect("write fixture");
        }
        let committed = std::fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error}; run with FIXTURE_UPDATE=1 to generate it",
                path.display()
            )
        });
        assert_eq!(
            committed,
            bytes,
            "{} is stale; run with FIXTURE_UPDATE=1",
            path.display()
        );
    }
}

#[test]
fn tier1_bake_matches_between_surfaces() {
    let home = fixture_home();
    let dir = work_dir("tier1-bake");
    let argv_out = dir.join("bake-argv.obj");
    let run_out = dir.join("bake-run.obj");
    let project = home.join("SliderSets/Tier1.osp");
    let presets = home.join("SliderPresets/Tier1.xml");

    let argv_stdout = run_ok(sculpto().args([
        "bake",
        "--project",
        project.to_str().unwrap(),
        "--presets",
        presets.to_str().unwrap(),
        "--set",
        "Tier1 Body",
        "--preset",
        "Tier1 Default",
        "--endpoint",
        "big",
        "--shape",
        "Tier1 Body",
        "--out",
        argv_out.to_str().unwrap(),
    ]));
    let baked = serde_json::json!({
        "project": project, "presets": presets, "set": "Tier1 Body",
        "preset": "Tier1 Default", "endpoint": "big", "shape": "Tier1 Body",
        "out": run_out,
    });
    let result = dispatched(&run_ok(sculpto().args([
        "run",
        "--cmd",
        "bake",
        "--input",
        &baked.to_string(),
    ])));

    assert_eq!(
        read(&argv_out),
        read(&run_out),
        "bake artifacts differ between argv and command surfaces"
    );
    // The same numbers, as each surface prints them.
    assert_eq!(
        argv_stdout.trim(),
        format!(
            "{}: {} vertices, {} triangles, {} sliders, {} unresolved, {} deferred",
            argv_out.display(),
            result["vertices"],
            result["triangles"],
            result["sliders"],
            result["unresolved"],
            result["deferred"]
        ),
        "bake numbers differ between argv and command surfaces"
    );
    assert_eq!(result["vertices"], 4);
    assert_eq!(result["triangles"], 2);
    assert_eq!(result["sliders"], 1);
    assert_eq!(result["unresolved"], 0);
    assert_eq!(result["deferred"], 0);
    // The morph moved the vertices it addresses, so an identical artifact is not
    // two copies of the source file.
    assert_ne!(
        read(&argv_out),
        {
            let source = dir.join("source.obj");
            run_ok(sculpto().args([
                "export-obj",
                "--nif",
                home.join("ShapeData/Tier1/Body.nif").to_str().unwrap(),
                "--shape",
                "Tier1 Body",
                "--out",
                source.to_str().unwrap(),
            ]));
            read(&source)
        },
        "the bake wrote the unbaked shape"
    );
}

#[test]
fn tier1_export_obj_matches_between_surfaces() {
    let home = fixture_home();
    let dir = work_dir("tier1-export");
    let nif = home.join("ShapeData/Tier1/Body.nif");
    let argv_out = dir.join("export-argv.obj");
    let run_out = dir.join("export-run.obj");

    let argv_stdout = run_ok(sculpto().args([
        "export-obj",
        "--nif",
        nif.to_str().unwrap(),
        "--shape",
        "Tier1 Body",
        "--out",
        argv_out.to_str().unwrap(),
    ]));
    let exported = serde_json::json!({
        "nif": nif, "shape": "Tier1 Body", "out": run_out,
    });
    let result = dispatched(&run_ok(sculpto().args([
        "run",
        "--cmd",
        "export.obj",
        "--input",
        &exported.to_string(),
    ])));

    assert_eq!(
        read(&argv_out),
        read(&run_out),
        "export.obj artifacts differ between argv and command surfaces"
    );
    assert_eq!(
        argv_stdout.trim(),
        format!(
            "{}: {} vertices, {} triangles (shape `{}`)",
            argv_out.display(),
            result["vertices"],
            result["triangles"],
            result["shape"].as_str().expect("shape is a string")
        ),
        "export.obj numbers differ between argv and command surfaces"
    );
    assert_eq!(result["vertices"], 4);
    assert_eq!(result["triangles"], 2);
}

#[test]
fn argv_and_run_produce_identical_artifacts() {
    let Some(home) = home() else {
        eprintln!("skipped: SCULPTO_BODYSLIDE_HOME is not set");
        return;
    };
    let project = home.join("SliderSets/HIMBO.osp");
    let presets = home.join("SliderPresets/HIMBO.xml");
    let set = "HIMBO Body - Vanilla Nevernude";
    let preset = "HIMBO Mike";
    let source = home.join("ShapeData/HIMBO/HIMBO Body - Vanilla.nif");

    let dir = std::env::temp_dir().join(format!("sculpto-a01-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let argv_bake = dir.join("bake-argv.obj");
    let run_bake = dir.join("bake-run.obj");
    let argv_export = dir.join("export-argv.obj");
    let run_export = dir.join("export-run.obj");
    let argv_fit = dir.join("fit-argv.obj");
    let run_fit = dir.join("fit-run.obj");

    run_ok(sculpto().args([
        "bake",
        "--project",
        project.to_str().unwrap(),
        "--presets",
        presets.to_str().unwrap(),
        "--set",
        set,
        "--preset",
        preset,
        "--endpoint",
        "big",
        "--out",
        argv_bake.to_str().unwrap(),
    ]));
    let baked = serde_json::json!({
        "project": project, "presets": presets, "set": set, "preset": preset,
        "endpoint": "big", "out": run_bake,
    });
    run_ok(sculpto().args(["run", "--cmd", "bake", "--input", &baked.to_string()]));
    assert_eq!(
        read(&argv_bake),
        read(&run_bake),
        "bake artifacts differ between argv and command surfaces"
    );

    run_ok(sculpto().args([
        "export-obj",
        "--nif",
        source.to_str().unwrap(),
        "--shape",
        "HIMBO - Body",
        "--out",
        argv_export.to_str().unwrap(),
    ]));
    let exported = serde_json::json!({
        "nif": source, "shape": "HIMBO - Body", "out": run_export,
    });
    run_ok(sculpto().args([
        "run",
        "--cmd",
        "export.obj",
        "--input",
        &exported.to_string(),
    ]));
    assert_eq!(
        read(&argv_export),
        read(&run_export),
        "export.obj artifacts differ between argv and command surfaces"
    );

    let body = read(&argv_export);
    assert!(!body.is_empty());
    run_ok(sculpto().args([
        "garment",
        "fit",
        "--nif",
        source.to_str().unwrap(),
        "--shape",
        "HIMBO - Boxers",
        "--body-shape",
        "HIMBO - Body",
        "--body",
        argv_export.to_str().unwrap(),
        "--out",
        argv_fit.to_str().unwrap(),
    ]));
    let fitted = serde_json::json!({
        "nif": source, "shape": "HIMBO - Boxers", "body_shape": "HIMBO - Body",
        "body": argv_export, "out": run_fit,
    });
    run_ok(sculpto().args(["run", "--cmd", "outfit.fit", "--input", &fitted.to_string()]));
    assert_eq!(
        read(&argv_fit),
        read(&run_fit),
        "outfit.fit artifacts differ between argv and command surfaces"
    );
}
