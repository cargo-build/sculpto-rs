//! `sculpto export-nif` on the process boundary: one shape by default, every
//! shape with `--all-shapes`, and the registry surface that describes both.
//!
//! The source fixture is written with the real SE bundle writer, so the input
//! is a file the reader already accepts; the output is re-read instead of
//! compared to a golden.

use std::path::{Path, PathBuf};
use std::process::Command;

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::write::se_bundle::write_se_bundle;
use sculpto_nif::write::skin::identity;
use serde_json::{Value, json};

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("sculpto-cli-export-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn skinned(name: &str, base: f32, vertices: usize) -> MeshAsset {
    let positions = (0..vertices)
        .map(|vertex| [base + vertex as f32, f32::from(vertex as u8 % 2), 0.0])
        .collect::<Vec<_>>();
    let triangles = match vertices > 3 {
        true => vec![[0, 1, 2], [1, 2, 3]],
        false => vec![[0, 1, 2]],
    };
    MeshAsset {
        name: name.into(),
        normals: vec![[0.0, 0.0, 1.0]; vertices],
        uvs: (0..vertices)
            .map(|vertex| [vertex as f32, f32::from(vertex as u8 % 2)])
            .collect(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: vec![identity(); 2],
            influences: SkinWeights::from_quads(
                &vec![[0, 1, 0, 0]; vertices],
                &vec![[0.75, 0.25, 0.0, 0.0]; vertices],
            )
            .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

/// Two shapes, the smaller first: a first-shape-only export is visible.
fn source_nif(directory: &Path) -> PathBuf {
    let shapes = vec![skinned("Hands", 4.0, 3), skinned("Body", 0.0, 4)];
    let path = directory.join("source-se.nif");
    let bytes = write_se_bundle(&shapes).expect("the fixture writes as an SE bundle");
    std::fs::write(&path, bytes).expect("source bytes");
    path
}

/// Unskinned variant for the single-shape default: the checked-rig path needs
/// a reader-captured skeleton root the writers' flat bone nodes do not provide
/// (pre-existing, independent of the bundle surface).
fn unskinned_source_nif(directory: &Path) -> PathBuf {
    let shapes = vec![
        MeshAsset {
            skin: None,
            ..skinned("Hands", 4.0, 3)
        },
        MeshAsset {
            skin: None,
            ..skinned("Body", 0.0, 4)
        },
    ];
    let path = directory.join("source-unskinned.nif");
    let bytes = write_se_bundle(&shapes).expect("the fixture writes as an SE bundle");
    std::fs::write(&path, bytes).expect("source bytes");
    path
}

fn run_ok(args: &[&str]) -> Value {
    let output = sculpto().args(args).output().expect("run");
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("one JSON envelope");
    assert_eq!(value["ok"], true, "envelope: {value}");
    value["result"].clone()
}

/// The per-shape array the outcome must carry, in scene order.
fn expected_shapes() -> Value {
    json!([
        { "name": "Hands", "vertices": 3, "triangles": 1 },
        { "name": "Body", "vertices": 4, "triangles": 2 },
    ])
}

fn assert_output_shapes(out: &Path) {
    let bytes = std::fs::read(out).expect("output bytes");
    let document = sculpto_nif::read(&bytes).expect("the export re-reads");
    let counts: Vec<(String, usize, usize)> = document
        .shapes
        .iter()
        .map(|shape| {
            (
                shape.name.clone(),
                shape.positions.len(),
                shape.triangles.len(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        vec![("Hands".to_string(), 3, 1), ("Body".to_string(), 4, 2)],
        "the written file keeps every shape"
    );
}

#[test]
fn export_all_shapes() {
    let scratch = Scratch::new("all-se");
    let source = source_nif(scratch.path());
    let out = scratch.path().join("out-se.nif");
    let result = run_ok(&[
        "export-nif",
        "--nif",
        source.to_str().expect("path"),
        "--out",
        out.to_str().expect("path"),
        "--target",
        "se",
        "--all-shapes",
    ]);
    assert_eq!(result["shapes"], expected_shapes(), "per-shape counts");
    assert_eq!(result["vertices"], 7);
    assert_eq!(result["triangles"], 3);
    assert_eq!(result["target"], "se");
    assert_eq!(
        result["bytes"].as_u64().expect("bytes"),
        std::fs::metadata(&out).expect("file").len()
    );
    assert_output_shapes(&out);
}

#[test]
fn export_all_shapes_le() {
    let scratch = Scratch::new("all-le");
    let source = source_nif(scratch.path());
    let out = scratch.path().join("out-le.nif");
    let result = run_ok(&[
        "export-nif",
        "--nif",
        source.to_str().expect("path"),
        "--out",
        out.to_str().expect("path"),
        "--target",
        "le",
        "--all-shapes",
    ]);
    assert_eq!(result["shapes"], expected_shapes());
    assert_eq!(result["target"], "le");
    assert_output_shapes(&out);
}

#[test]
fn explicit_shapes_keep_the_list_order() {
    let scratch = Scratch::new("list");
    let source = source_nif(scratch.path());
    let out = scratch.path().join("out.nif");
    let result = run_ok(&[
        "export-nif",
        "--nif",
        source.to_str().expect("path"),
        "--out",
        out.to_str().expect("path"),
        "--target",
        "se",
        "--shapes",
        "Body",
        "--shapes",
        "Hands",
    ]);
    assert_eq!(
        result["shapes"],
        json!([
            { "name": "Body", "vertices": 4, "triangles": 2 },
            { "name": "Hands", "vertices": 3, "triangles": 1 },
        ])
    );
    let document = sculpto_nif::read(&std::fs::read(&out).expect("bytes")).expect("re-read");
    let names: Vec<&str> = document
        .shapes
        .iter()
        .map(|shape| shape.name.as_str())
        .collect();
    assert_eq!(names, vec!["Body", "Hands"]);
}

#[test]
fn the_default_stays_single_shape() {
    let scratch = Scratch::new("default");
    let source = unskinned_source_nif(scratch.path());
    let out = scratch.path().join("out.nif");
    let result = run_ok(&[
        "export-nif",
        "--nif",
        source.to_str().expect("path"),
        "--out",
        out.to_str().expect("path"),
        "--target",
        "se",
    ]);
    assert_eq!(result["vertices"], 4, "the largest shape, not the first");
    assert_eq!(result["triangles"], 2);
    assert!(result.get("shapes").is_none(), "single-shape outcome shape");
    let document = sculpto_nif::read(&std::fs::read(&out).expect("bytes")).expect("re-read");
    assert_eq!(document.shapes.len(), 1);
    assert_eq!(document.shapes[0].name, "Body");
}

#[test]
fn all_shapes_conflicts_with_shape() {
    let scratch = Scratch::new("conflict");
    let source = source_nif(scratch.path());
    let out = scratch.path().join("out.nif");
    let output = sculpto()
        .args([
            "export-nif",
            "--nif",
            source.to_str().expect("path"),
            "--out",
            out.to_str().expect("path"),
            "--target",
            "se",
            "--all-shapes",
            "--shape",
            "Body",
        ])
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2), "a usage error exits 2");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--all-shapes"), "{stderr}");
    assert!(!out.exists(), "a refused request writes no file");
}

#[test]
fn the_bundle_command_is_registered_and_dispatches() {
    let output = sculpto()
        .args(["commands", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success(), "commands --json failed");
    let listed: Value = serde_json::from_slice(&output.stdout).expect("catalogue JSON");
    let row = listed["commands"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["id"] == "export.bundle")
        .expect("export.bundle is catalogued");
    assert_eq!(row["implemented"], true);
    assert_eq!(row["mutates"], true);
    let schema = &row["input_schema"];
    assert!(
        schema["properties"]["shapes"].is_object(),
        "shapes parameter"
    );
    assert!(schema["properties"]["nif"].is_object());
    let required = schema["required"].as_array().expect("required list");
    for field in ["nif", "out", "target"] {
        assert!(
            required.iter().any(|value| value == field),
            "{field} required"
        );
    }

    let scratch = Scratch::new("registry");
    let source = source_nif(scratch.path());
    let out = scratch.path().join("out.nif");
    let input = json!({
        "nif": source.to_str().expect("path"),
        "out": out.to_str().expect("path"),
        "target": "se",
    });
    let result = run_ok(&[
        "run",
        "--cmd",
        "export.bundle",
        "--input",
        &input.to_string(),
    ]);
    assert_eq!(result["shapes"], expected_shapes());
    assert_output_shapes(&out);
}
