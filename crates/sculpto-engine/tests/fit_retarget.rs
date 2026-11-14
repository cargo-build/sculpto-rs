//! Cross-preset retarget (`--body-ref`) contract: triangle-frame transport onto
//! the requested body, identity reproduction, typed refusals and provenance.
//!
//! Synthetic NIF + OBJ fixtures only; no host fixture is required. The garment
//! and an embedded body reference are authored into one two-shape NIF, and the
//! external reference / target bodies are OBJs written in the shared display
//! frame, so the legacy and retarget paths bind the same reference geometry.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_engine::{dispatch, obj};
use sculpto_nif::write::se_bundle::write_se_bundle;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "sculpto-fit-retarget-{}-{}",
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

/// A unit quad in the source (Z-up) frame at the given height.
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

/// The two-shape garment NIF plus the external reference and target OBJs.
struct Scene {
    nif: PathBuf,
    reference: PathBuf,
    identity: PathBuf,
    morphed: PathBuf,
}

fn scene(fixture: &Fixture) -> Scene {
    // Garment sits 0.5 above the authored body reference: a real thickness the
    // retarget must preserve and the legacy self-bind must not invent.
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
    // A morph that tilts the control surface: vertex 2 rises by 2 units.
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

fn fit(
    fixture: &Fixture,
    scene: &Scene,
    extra: Value,
) -> Result<Value, sculpto_engine::CommandError> {
    let mut input = json!({
        "nif": scene.nif,
        "shape": "Garment",
        "body": scene.morphed,
        "out": fixture.path("out.obj"),
    });
    if let (Some(base), Some(extra)) = (input.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    dispatch("outfit.fit", &input)
}

fn positions(path: &Path) -> Vec<[f32; 3]> {
    obj::read_obj_positions(path).expect("read positions")
}

fn max_delta(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    assert_eq!(a.len(), b.len(), "vertex count differs");
    a.iter()
        .zip(b)
        .map(|(left, right)| {
            let d = [left[0] - right[0], left[1] - right[1], left[2] - right[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        })
        .fold(0.0, f32::max)
}

fn file_sha256(path: &Path) -> String {
    let bytes = std::fs::read(path).expect("read file");
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Target == reference must reproduce the authored garment, bit-for-bit.
#[test]
fn retarget_identity_reproduces_the_authored_garment() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);
    let out = fixture.path("identity-fit.obj");
    let result = fit(
        &fixture,
        &scene,
        json!({ "body": scene.identity, "body_ref": scene.reference, "out": out }),
    )
    .expect("identity retarget");
    assert_eq!(result["vertices"], 4);

    // The authored garment, exported by the product, is the ground truth. The
    // fit writer emits positions + faces only (no UV records), so the identity
    // claim is on the geometry: every vertex must round-trip bit-for-bit.
    let authored = fixture.path("authored.obj");
    dispatch(
        "export.obj",
        &json!({ "nif": scene.nif, "shape": "Garment", "out": authored }),
    )
    .expect("export authored garment");
    assert_eq!(
        positions(&out),
        positions(&authored),
        "target == reference must reproduce the authored garment bit-for-bit"
    );
    assert_eq!(
        positions(&out),
        quad(0.5),
        "identity must equal the authored quad exactly"
    );
    assert!(max_delta(&positions(&out), &quad(0.5)) == 0.0);
}

/// The retargeted output differs from the legacy conform on the same morphed
/// body, and it moves when the target moves (no silent reuse of the binding).
#[test]
fn retarget_differs_from_legacy_and_follows_the_target() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    let legacy_out = fixture.path("legacy.obj");
    fit(
        &fixture,
        &scene,
        json!({ "body_shape": "Body", "out": legacy_out }),
    )
    .expect("legacy fit");

    let retarget_out = fixture.path("retarget.obj");
    fit(
        &fixture,
        &scene,
        json!({ "body_ref": scene.reference, "out": retarget_out }),
    )
    .expect("retarget fit");

    let legacy = positions(&legacy_out);
    let retarget = positions(&retarget_out);
    assert!(
        max_delta(&legacy, &retarget) > 1e-3,
        "retarget must differ from the legacy conform"
    );
    assert_ne!(
        std::fs::read(&legacy_out).expect("legacy bytes"),
        std::fs::read(&retarget_out).expect("retarget bytes"),
        "retarget wrote the legacy artifact"
    );

    // A different target must produce a different garment.
    let identity_out = fixture.path("identity.obj");
    fit(
        &fixture,
        &scene,
        json!({ "body": scene.identity, "body_ref": scene.reference, "out": identity_out }),
    )
    .expect("identity fit");
    assert!(
        max_delta(&positions(&identity_out), &retarget) > 1e-3,
        "the garment did not follow the target body"
    );
}

/// Reference/target topology refusal names both vertex and triangle counts.
#[test]
fn reference_target_topology_mismatch_names_both_counts() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    // Three vertices against the four-vertex reference.
    let short = fixture.path("short.obj");
    std::fs::write(&short, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("short body");
    let error = fit(
        &fixture,
        &scene,
        json!({ "body": short, "body_ref": scene.reference }),
    )
    .expect_err("vertex count mismatch must refuse");
    assert_eq!(error.code(), "reference_target_topology_mismatch");
    assert_eq!(error.kind().as_str(), "refused");
    let message = error.to_string();
    assert!(message.contains("4 vertices / 2 triangles"), "{message}");
    assert!(message.contains("3 vertices / 1 triangles"), "{message}");

    // Same vertex count, different ordered triangles.
    let retri = fixture.path("retri.obj");
    obj::write_obj(&retri, &quad(0.0), &[[0, 1, 3], [1, 2, 3]]).expect("retri body");
    let error = fit(
        &fixture,
        &scene,
        json!({ "body": retri, "body_ref": scene.reference }),
    )
    .expect_err("triangle mismatch must refuse");
    assert_eq!(error.code(), "reference_target_topology_mismatch");
    let message = error.to_string();
    assert!(message.contains("2 triangles"), "{message}");
}

/// Without `--body-ref` the legacy topology refusal names the conversion path.
#[test]
fn legacy_topology_mismatch_names_body_ref() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);
    let short = fixture.path("short.obj");
    std::fs::write(&short, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").expect("short body");
    let error = fit(
        &fixture,
        &scene,
        json!({ "body": short, "body_shape": "Body" }),
    )
    .expect_err("legacy topology mismatch must refuse");
    assert_eq!(error.code(), "topology_mismatch");
    assert_eq!(error.kind().as_str(), "refused");
    let message = error.to_string();
    assert!(message.contains("--body-ref"), "{message}");
    assert!(message.contains("3 vertices"), "{message}");
    assert!(message.contains("4"), "{message}");
}

/// Provenance rows and transport method for both modes.
#[test]
fn provenance_rows_and_transport_in_both_modes() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    let legacy = fit(&fixture, &scene, json!({ "body_shape": "Body" })).expect("legacy fit");
    assert_eq!(legacy["reference_source"], "embedded_nif");
    assert_eq!(legacy["transport_method"], "nearest_vertex_offset");
    assert_eq!(legacy["reference_vertices"], 4);
    assert_eq!(legacy["reference_triangles"], 2);
    assert_eq!(legacy["target_vertices"], 4);
    assert_eq!(legacy["target_triangles"], Value::Null);
    assert_eq!(legacy["reference_path"], scene.nif.display().to_string());
    assert_eq!(legacy["target_body_sha256"], file_sha256(&scene.morphed));
    assert_eq!(
        legacy["reference_body_sha256"]
            .as_str()
            .expect("digest")
            .len(),
        64
    );

    let retarget = fit(&fixture, &scene, json!({ "body_ref": scene.reference })).expect("retarget");
    assert_eq!(retarget["reference_source"], "external_reference");
    assert_eq!(retarget["transport_method"], "surface_triangle_frame");
    assert_eq!(retarget["reference_vertices"], 4);
    assert_eq!(retarget["reference_triangles"], 2);
    assert_eq!(retarget["target_vertices"], 4);
    assert_eq!(retarget["target_triangles"], 2);
    assert_eq!(
        retarget["reference_path"],
        scene.reference.display().to_string()
    );
    assert_eq!(retarget["target_path"], scene.morphed.display().to_string());
    assert_eq!(
        retarget["reference_body_sha256"],
        file_sha256(&scene.reference)
    );
    assert_eq!(retarget["target_body_sha256"], file_sha256(&scene.morphed));
    assert_ne!(
        retarget["reference_body_sha256"],
        retarget["target_body_sha256"]
    );
}

/// Missing garment and body shapes are refused with the available list.
#[test]
fn missing_shapes_list_available_names() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    let error = fit(
        &fixture,
        &scene,
        json!({ "shape": "Nope", "body_shape": "Body" }),
    )
    .expect_err("missing garment shape");
    assert_eq!(error.code(), "missing_shape");
    assert_eq!(error.kind().as_str(), "not_found");
    let message = error.to_string();
    assert!(
        message.contains("Garment") && message.contains("Body"),
        "{message}"
    );

    let error =
        fit(&fixture, &scene, json!({ "body_shape": "Nope" })).expect_err("missing body shape");
    assert_eq!(error.code(), "missing_body_shape");
    assert_eq!(error.kind().as_str(), "not_found");
    let message = error.to_string();
    assert!(
        message.contains("Garment") && message.contains("Body"),
        "{message}"
    );
}

/// An empty control surface (vertices but no triangles) is a typed refusal.
#[test]
fn empty_control_surface_is_refused() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);
    // Both bodies carry vertices but no faces, so the topology check passes and
    // the binding itself refuses the empty control.
    let flat_ref = fixture.path("flat-ref.obj");
    obj::write_obj(&flat_ref, &quad(0.0), &[]).expect("faceless reference");
    let flat_body = fixture.path("flat-body.obj");
    obj::write_obj(&flat_body, &quad(0.0), &[]).expect("faceless body");
    let error = fit(
        &fixture,
        &scene,
        json!({ "body": flat_body, "body_ref": flat_ref }),
    )
    .expect_err("empty control must refuse");
    assert_eq!(error.code(), "empty_control");
    assert_eq!(error.kind().as_str(), "invalid_params");
}

/// `body_ref` needs no `body_shape`, but is refused on guarded paths.
#[test]
fn body_ref_scope_is_plain_only() {
    let fixture = Fixture::new();
    let scene = scene(&fixture);

    // No body_shape at all: the retarget still fits.
    fit(&fixture, &scene, json!({ "body_ref": scene.reference }))
        .expect("retarget without body_shape");

    // A guarded manifest request cannot carry body_ref.
    let manifest = fixture.path("manifest.json");
    std::fs::write(&manifest, "{}").expect("manifest");
    let error = fit(
        &fixture,
        &scene,
        json!({ "body_ref": scene.reference, "manifest": manifest }),
    )
    .expect_err("guarded body_ref must refuse");
    assert_eq!(error.code(), "outfit_fit_body_ref_scope");
    assert_eq!(error.kind().as_str(), "refused");
}
