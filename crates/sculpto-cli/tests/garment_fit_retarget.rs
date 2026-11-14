//! argv `garment fit --body-ref` retarget: the flag is optional, the legacy path
//! is unchanged without it, and the registry advertises the new parameter.
//!
//! Synthetic two-shape NIF + OBJ fixtures in test code; no host fixture needed.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::obj;
use sculpto_nif::write::se_bundle::write_se_bundle;
use serde_json::Value;

fn sculpto() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "sculpto-cli-retarget-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("unique retarget directory");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove owned retarget files");
    }
}

fn quad(height: f32) -> Vec<[f32; 3]> {
    vec![
        [0.0, 0.0, height],
        [2.0, 0.0, height],
        [2.0, 2.0, height],
        [0.0, 2.0, height],
    ]
}

fn quad_triangles() -> Vec<[u32; 3]> {
    vec![[0, 1, 2], [0, 2, 3]]
}

fn asset(name: &str, positions: Vec<[f32; 3]>) -> MeshAsset {
    let triangles = quad_triangles();
    MeshAsset {
        name: name.into(),
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        uvs: vec![[0.0, 0.0]; positions.len()],
        triangles: triangles.clone(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        positions,
        ..MeshAsset::default()
    }
}

struct Scene {
    nif: PathBuf,
    reference: PathBuf,
    identity: PathBuf,
    morphed: PathBuf,
}

fn scene(fixture: &Fixture) -> Scene {
    let garment = asset("Garment", quad(0.5));
    let body = asset("Body", quad(0.0));
    let nif = fixture.path("garment.nif");
    std::fs::write(
        &nif,
        write_se_bundle(&[garment, body]).expect("bundle writes"),
    )
    .expect("write NIF");
    let reference = fixture.path("reference.obj");
    obj::write_obj(&reference, &quad(0.0), &quad_triangles()).expect("reference OBJ");
    let identity = fixture.path("identity.obj");
    obj::write_obj(&identity, &quad(0.0), &quad_triangles()).expect("identity OBJ");
    let mut morph = quad(0.0);
    morph[2][2] = 2.0;
    let morphed = fixture.path("morphed.obj");
    obj::write_obj(&morphed, &morph, &quad_triangles()).expect("morphed OBJ");
    Scene {
        nif,
        reference,
        identity,
        morphed,
    }
}

fn run(args: &[&str]) -> (bool, String, String) {
    let output = sculpto().args(args).output().expect("spawn sculpto");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn positions(path: &Path) -> Vec<[f32; 3]> {
    obj::read_obj_positions(path).expect("read positions")
}

/// `--body-ref` fits without `--body-shape`, prints provenance, and reproduces
/// the authored garment bit-for-bit when target == reference.
#[test]
fn body_ref_fits_without_body_shape_and_identity_is_exact() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);
    let out = fixture.path("identity-fit.obj");
    let (ok, stdout, stderr) = run(&[
        "garment",
        "fit",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--body-ref",
        scene.reference.to_str().expect("ref"),
        "--body",
        scene.identity.to_str().expect("body"),
        "--out",
        out.to_str().expect("out"),
    ]);
    assert!(ok, "retarget failed: {stderr}");
    assert!(stdout.contains("reference: external_reference"), "{stdout}");
    assert!(
        stdout.contains("transport: surface_triangle_frame"),
        "{stdout}"
    );
    assert!(stdout.contains("sha256 "), "{stdout}");

    let authored = fixture.path("authored.obj");
    let (ok, _, stderr) = run(&[
        "export-obj",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--out",
        authored.to_str().expect("out"),
    ]);
    assert!(ok, "export failed: {stderr}");
    assert_eq!(
        positions(&out),
        positions(&authored),
        "target == reference must reproduce the authored garment bit-for-bit"
    );
}

/// The retargeted artifact differs from the legacy conform on the same body.
#[test]
fn retarget_differs_from_legacy_argv() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    let legacy = fixture.path("legacy.obj");
    let (ok, _, stderr) = run(&[
        "garment",
        "fit",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--body-shape",
        "Body",
        "--body",
        scene.morphed.to_str().expect("body"),
        "--out",
        legacy.to_str().expect("out"),
    ]);
    assert!(ok, "legacy fit failed: {stderr}");

    let retarget = fixture.path("retarget.obj");
    let (ok, _, stderr) = run(&[
        "garment",
        "fit",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--body-ref",
        scene.reference.to_str().expect("ref"),
        "--body",
        scene.morphed.to_str().expect("body"),
        "--out",
        retarget.to_str().expect("out"),
    ]);
    assert!(ok, "retarget fit failed: {stderr}");
    assert_ne!(
        positions(&legacy),
        positions(&retarget),
        "retarget must differ from the legacy conform"
    );
}

/// The registry advertises `body_ref` as an optional `outfit.fit` parameter and
/// no longer requires `body_shape`.
#[test]
fn registry_lists_body_ref_optional() {
    let (ok, stdout, stderr) = run(&["commands", "--json"]);
    assert!(ok, "commands failed: {stderr}");
    let value: Value = serde_json::from_str(&stdout).expect("commands JSON");
    let row = value["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .find(|row| row["id"] == "outfit.fit")
        .expect("outfit.fit row");
    let schema = &row["input_schema"];
    assert!(
        schema["properties"]["body_ref"].is_object(),
        "body_ref missing: {schema}"
    );
    let required = schema["required"].as_array().expect("required array");
    assert!(
        !required.iter().any(|field| field == "body_ref"),
        "body_ref must be optional: {required:?}"
    );
    assert!(
        !required.iter().any(|field| field == "body_shape"),
        "body_shape must be optional when body_ref is present: {required:?}"
    );
}

/// Legacy refusals are unchanged without `--body-ref`; the retarget adds its own
/// typed reference/target refusal.
#[test]
fn argv_refusals_are_typed() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);
    let short = fixture.path("short.obj");
    std::fs::write(&short, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("short body");
    let never = fixture.path("never.obj");

    // Legacy topology refusal names the conversion path.
    let (ok, _, stderr) = run(&[
        "garment",
        "fit",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--body-shape",
        "Body",
        "--body",
        short.to_str().expect("body"),
        "--out",
        never.to_str().expect("out"),
    ]);
    assert!(!ok, "legacy mismatch must refuse");
    assert!(stderr.contains("topology_mismatch"), "{stderr}");
    assert!(stderr.contains("--body-ref"), "{stderr}");
    assert!(!never.exists(), "a refused fit left a file behind");

    // Retarget reference/target refusal.
    let (ok, _, stderr) = run(&[
        "garment",
        "fit",
        "--nif",
        scene.nif.to_str().expect("nif"),
        "--shape",
        "Garment",
        "--body-ref",
        scene.reference.to_str().expect("ref"),
        "--body",
        short.to_str().expect("body"),
        "--out",
        never.to_str().expect("out"),
    ]);
    assert!(!ok, "retarget mismatch must refuse");
    assert!(
        stderr.contains("reference_target_topology_mismatch"),
        "{stderr}"
    );
    assert!(!never.exists(), "a refused fit left a file behind");
}
