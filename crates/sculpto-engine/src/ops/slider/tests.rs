//! Collision inputs and outputs retain their declared coordinate frames. @req DECL0013

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sculpto_core::mesh::{MeshAsset, PartitionId};
use sculpto_nif::write::se_shape::write_se_shape;
use serde_json::json;

use super::{FitParams, NifDocument, PlacedShape, finish_fit, target_positions};
use crate::obj::{read_obj_positions, write_obj};

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
