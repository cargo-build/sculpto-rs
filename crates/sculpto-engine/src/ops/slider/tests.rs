//! Collision inputs and outputs retain their declared coordinate frames. @req DECL0013

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::json;

use super::{FitParams, NifDocument, PlacedShape, finish_fit, target_positions};
use crate::ErrorKind;
use crate::obj::{read_obj_positions, write_obj};
use crate::ops;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "sculpto-placed-collision-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).expect("unique fixture directory");
        Self(root)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove owned fixture directory");
    }
}

fn document(
    name: &str,
    positions: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
) -> (NifDocument, Vec<u8>) {
    let asset = MeshAsset {
        name: name.into(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    };
    let bytes = write_se_shape(&asset).expect("synthetic NIF");
    let mut document = sculpto_nif::read(&bytes).expect("read synthetic NIF");
    let half_turn = std::f32::consts::FRAC_1_SQRT_2;
    document.sources[0].local.translation = [10.0, 20.0, 30.0];
    document.sources[0].local.rotation = [0.0, 0.0, half_turn, half_turn];
    document.sources[0].local.scale = 2.0;
    (document, bytes)
}

#[test]
fn collision_scope_and_output_respect_nonidentity_placement() {
    let fixture = Fixture::new();
    let (body_doc, _) = document(
        "Body",
        vec![
            [-10.0, -10.0, 0.0],
            [10.0, -10.0, 0.0],
            [10.0, 10.0, 0.0],
            [-10.0, 10.0, 0.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    let (garment_doc, garment_bytes) = document(
        "Garment",
        vec![[0.0, -1.0, 0.5], [1.0, -1.0, 0.5], [0.0, 1.0, 0.5]],
        vec![[0, 1, 2]],
    );
    let garment_path = fixture.0.join("garment.nif");
    std::fs::write(&garment_path, garment_bytes).expect("garment NIF");
    let manifest_path = fixture.0.join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "outfit": "Placed garment",
            "nif": "garment.nif",
            "garment_shapes": [{
                "name": "Garment",
                "blender_cloth": [{"name": "cloth", "triangles": [0]}]
            }]
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest");

    let body = PlacedShape::named(&body_doc, "Body").expect("placed body");
    let garment = PlacedShape::named(&garment_doc, "Garment").expect("placed garment");
    let body_path = fixture.0.join("body.obj");
    write_obj(&body_path, &body.shape.positions, &body.shape.triangles).expect("body OBJ");
    let mut params = FitParams {
        nif: garment_path,
        shape: "Garment".into(),
        body_shape: "Body".into(),
        body: body_path,
        out: fixture.0.join("fitted.obj"),
        body_after: None,
        out_after: None,
        conform: None,
        slider: None,
        percent: None,
        body_nif: None,
        manifest: Some(manifest_path),
        collision: Some(fixture.0.join("collision.obj")),
        collision_frame: Some("source-scene".into()),
        final_push: None,
        final_push_tolerance: None,
    };
    let target = target_positions(&params, &body).expect("placed body target");
    for (actual, expected) in target[0].iter().zip([30.0, 0.0, 30.0]) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "body source frame: {target:?}"
        );
    }

    let check = |params: &FitParams, shift: f32| {
        let mut positions = target.clone();
        positions.extend(
            [
                [0.2 + shift, 0.0, 0.3],
                [0.7 + shift, 0.0, 0.3],
                [0.7 + shift, 0.0, 0.7],
                [0.2 + shift, 0.0, 0.7],
            ]
            .into_iter()
            .map(|point| body.placement.position_in_source(point)),
        );
        write_obj(
            params.collision.as_deref().expect("collision path"),
            &positions,
            &[[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
        )
        .expect("source-scene collision OBJ");
        let outcome = finish_fit(
            params,
            &garment_doc,
            &garment,
            &body,
            garment.source_positions(),
            target.clone(),
        )
        .expect("placed fit");
        let returned = read_obj_positions(&params.out).expect("body-mesh output");
        for (actual, expected) in returned.iter().zip(&garment.shape.positions) {
            for (actual, expected) in actual.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-4, "cloth output frame");
            }
        }
        outcome
            .regions
            .expect("region report")
            .cloth_intersection_pairs
    };

    assert!(check(&params, 0.0) > 0, "the placed hand crosses the cloth");
    params.out = fixture.0.join("clear.obj");
    assert_eq!(
        check(&params, 5.0),
        0,
        "moving the hand removes the contact"
    );
}

/// One shape in one NIF with the identity placement.
fn plain_document(name: &str, positions: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Vec<u8> {
    let asset = MeshAsset {
        name: name.into(),
        positions,
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        triangles,
        ..MeshAsset::default()
    };
    write_se_shape(&asset).expect("synthetic NIF")
}

/// A body quad in the `z = 0` plane and a garment quad one standoff above it.
///
/// Both shapes keep the identity placement: with the shared scale-2 fixture the
/// guarded repair clears the standoff and the fit takes the early-clear return,
/// which never reaches the final push stage.
fn final_push_scene(fixture: &Fixture) -> FitParams {
    let garment_bytes = plain_document(
        "Garment",
        vec![
            [-1.0, -1.0, 0.5],
            [1.0, -1.0, 0.5],
            [1.0, 1.0, 0.5],
            [-1.0, 1.0, 0.5],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    let garment_path = fixture.0.join("garment.nif");
    std::fs::write(&garment_path, garment_bytes).expect("garment NIF");
    let body_bytes = plain_document(
        "Body",
        vec![
            [-1.0, -1.0, 0.0],
            [1.0, -1.0, 0.0],
            [1.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0],
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    );
    let body_path = fixture.0.join("body.nif");
    std::fs::write(&body_path, body_bytes).expect("body NIF");
    let manifest_path = fixture.0.join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "outfit": "Final push garment",
            "nif": "garment.nif",
            "garment_shapes": [{"name": "Garment"}],
        }))
        .expect("manifest JSON"),
    )
    .expect("manifest");

    let body_obj = fixture.0.join("body.obj");
    std::fs::write(
        &body_obj,
        "v -1 -1 0\nv 1 -1 0\nv 1 1 0\nv -1 1 0\nf 1 2 3\nf 1 3 4\n",
    )
    .expect("body OBJ");
    FitParams {
        nif: garment_path,
        shape: "Garment".into(),
        body_shape: "Body".into(),
        body: body_obj,
        out: fixture.0.join("fitted.obj"),
        body_after: None,
        out_after: None,
        conform: None,
        slider: None,
        percent: None,
        body_nif: Some(body_path),
        manifest: Some(manifest_path),
        collision: None,
        collision_frame: None,
        final_push: None,
        final_push_tolerance: None,
    }
}

/// The returned positions and the guarded report of one `finish_fit` run.
fn guarded_fit(
    params: &FitParams,
) -> Result<(Vec<[f32; 3]>, sculpto_mesh::wrap::repair::FitRepairReport), String> {
    let document = sculpto_nif::read(&std::fs::read(&params.nif).expect("garment bytes"))
        .expect("garment document");
    let body_doc = sculpto_nif::read(
        &std::fs::read(params.body_nif.as_ref().expect("body NIF")).expect("body bytes"),
    )
    .expect("body document");
    let garment = PlacedShape::named(&document, &params.shape).expect("placed garment");
    let body = PlacedShape::named(&body_doc, &params.body_shape).expect("placed body");
    let target = target_positions(params, &body).expect("target positions");
    let outcome = finish_fit(
        params,
        &document,
        &garment,
        &body,
        garment.source_positions(),
        target,
    )
    .map_err(|error| error.to_string())?;
    let report = outcome
        .regions
        .expect("a manifest fit reports the ordinary region")
        .ordinary
        .expect("a manifest fit without cloth fits an ordinary arena");
    Ok((read_obj_positions(&params.out).expect("fitted OBJ"), report))
}

#[test]
fn the_stage_is_off_by_default_and_the_request_reaches_the_fit() {
    let fixture = Fixture::new();
    let base = final_push_scene(&fixture);
    let (without, report) = guarded_fit(&base).expect("fit without the stage");
    assert!(
        report.final_push.is_none(),
        "the stage must not run unless the caller asks"
    );

    let mut requested = base.clone();
    requested.out = fixture.0.join("pushed.obj");
    requested.final_push = Some(2);
    requested.final_push_tolerance = Some(0.05);
    let (with, report) = guarded_fit(&requested).expect("fit with the stage");
    let push = report.final_push.expect("the stage report");
    assert_eq!(push.budget, 2, "the caller's budget reaches the fit");
    assert_eq!(push.tolerance, 0.05, "the tolerance reaches the fit");
    assert_eq!(push.before_violations, 0, "this standoff grades nothing");
    assert_eq!(push.passes, 0, "nothing to push spends no pass");
    assert!(push.per_pass.is_empty());
    assert_eq!(
        with, without,
        "a vacuous opt-in report must not move the garment"
    );
}

#[test]
fn an_out_of_range_final_push_budget_or_tolerance_is_refused_by_name() {
    let fixture = Fixture::new();
    let mut params = final_push_scene(&fixture);
    params.final_push = Some(0);
    params.final_push_tolerance = Some(0.05);
    let error = guarded_fit(&params).expect_err("zero passes");
    assert!(error.contains("outside 1..=10"), "{error}");

    params.final_push = Some(11);
    let error = guarded_fit(&params).expect_err("over budget");
    assert!(error.contains("outside 1..=10"), "{error}");

    params.final_push = Some(2);
    params.final_push_tolerance = Some(-1.0);
    let error = guarded_fit(&params).expect_err("negative tolerance");
    assert!(error.contains("negative or not finite"), "{error}");
}

#[test]
fn a_half_declared_final_push_request_is_refused_by_name() {
    let fixture = Fixture::new();
    let mut params = final_push_scene(&fixture);
    params.final_push = Some(2);
    let error = params.final_push().expect_err("a budget alone");
    assert!(
        error.to_string().contains("`final_push_tolerance`"),
        "{error}"
    );

    params.final_push = None;
    params.final_push_tolerance = Some(0.05);
    let error = params.final_push().expect_err("a tolerance alone");
    assert!(error.to_string().contains("`final_push`"), "{error}");

    params.final_push = Some(2);
    let push = params
        .final_push()
        .expect("both halves")
        .expect("a request");
    assert_eq!(push.passes, 2);
    assert_eq!(push.tolerance, 0.05);
}

#[test]
fn the_unguarded_conform_path_refuses_the_final_push_stage() {
    let fixture = Fixture::new();
    let mut params = final_push_scene(&fixture);
    params.manifest = None;
    params.collision = None;
    params.body_nif = None;
    params.final_push = Some(2);
    params.final_push_tolerance = Some(0.05);
    let error = ops::garment_fit(&params).expect_err("no guarded fit");
    assert_eq!(error.kind(), ErrorKind::Refused);
    assert_eq!(error.code(), "outfit_fit_final_push");
    assert!(error.to_string().contains("needs a guarded fit"), "{error}");

    params.final_push = None;
    params.final_push_tolerance = None;
    let unrequested = ops::garment_fit(&params).expect_err("this scene has no conform inputs");
    assert_ne!(
        (unrequested.kind(), unrequested.code()),
        (ErrorKind::Refused, "outfit_fit_final_push"),
        "without a request the path is unchanged: {unrequested}"
    );
}
