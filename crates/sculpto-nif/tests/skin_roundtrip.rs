//! A real skinned body through the SE writer and back.
//!
//! Private-host tier: the body NIF is reached through `SCULPTO_BIJIN_NIF`; the
//! test skips without it, so `just gate` stays portable.
//!
//! The accepted Bijin body authors five influences on five vertices. M3a keeps
//! those rows raw, and the four-lane SE writer refuses them by name instead of
//! truncating; the lossy native-four conversion and its report belong to the
//! follow-up slice. This test records that staging refusal and keeps the
//! four-lane round-trip assertions on a synthetic body, so the coverage does not
//! disappear with the refusal.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::SkinWeights;
use sculpto_nif::write::header::NifWriteError;
use sculpto_nif::write::se_shape::write_se_shape;
use sculpto_nif::write::{InfluencePolicy, write_shape_with_policy};
use sculpto_nif::write::header::NifTarget;
use sculpto_nif::{NifShape, read};

fn synthetic_body() -> MeshAsset {
    let quad = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    MeshAsset {
        name: "Synthetic".into(),
        positions: quad.to_vec(),
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: vec!["Root".into(), "Spine".into()],
            bind_inverse: Vec::new(),
            influences: SkinWeights::from_quads(&[[0, 1, 0, 0]; 3], &[[0.75, 0.25, 0.0, 0.0]; 3])
                .expect("four-lane rows"),
        }),
        ..MeshAsset::default()
    }
}

#[test]
fn a_four_lane_body_roundtrips_through_the_se_writer() {
    let source = synthetic_body();
    source
        .validate()
        .expect("the synthetic body is a valid model");

    let written = write_se_shape(&source).expect("the body writes");
    let reread = read(&written).expect("our SE NIF parses");
    let back = NifShape::from(&MeshAsset::from(&reread.shapes[0]));

    let original = source.skin.as_ref().expect("authored skin");
    let copied = back.skin.as_ref().expect("skin read back");
    assert_eq!(copied.bones, original.bones);
    assert_eq!(copied.influences.rows(), original.influences.rows());
    for vertex in 0..original.influences.rows() {
        let (actual, expected) = (
            copied.influences.row(vertex).expect("read row"),
            original.influences.row(vertex).expect("authored row"),
        );
        assert_eq!(
            actual.joints, expected.joints,
            "palette order survives the packed round trip"
        );
        for (left, right) in actual.weights.iter().zip(expected.weights) {
            assert!((left - right).abs() <= 1e-3, "f16 weight drift");
        }
    }
    for (actual, expected) in copied.bind_inverse.iter().zip(&original.bind_inverse) {
        for column in 0..4 {
            for row in 0..4 {
                assert!(
                    (actual[column][row] - expected[column][row]).abs() <= 1e-4,
                    "bind matrix drift"
                );
            }
        }
    }
    assert_eq!(back.triangles, source.triangles);
}

#[test]
fn bijin_native_weights_roundtrip_with_an_explicit_four_lane_report() {
    let Ok(path) = std::env::var("SCULPTO_BIJIN_NIF") else {
        eprintln!("SCULPTO_BIJIN_NIF not set; skipping the raw Bijin influence control");
        return;
    };
    let bytes = std::fs::read(&path).expect("Bijin NIF is readable");
    let document = read(&bytes).expect("Bijin NIF parses");
    let source = document
        .shapes
        .into_iter()
        .max_by_key(|shape| shape.positions.len())
        .expect("a shape");
    let skin = source.skin.as_ref().expect("the body is skinned");
    assert_eq!(skin.bones.len(), 22, "the accepted Bijin palette");
    let widest = (0..skin.influences.rows())
        .filter_map(|row| skin.influences.row(row).map(|row| row.joints.len()))
        .max()
        .expect("a vertex");
    assert_eq!(widest, 5, "the accepted body authors five influences");

    let mut mesh = MeshAsset::from(&source);
    assert_eq!(
        write_se_shape(&mesh),
        Err(NifWriteError::TooManyInfluences {
            vertex: 818,
            influences: 5
        }),
        "the four-lane writer refuses a five-influence row instead of truncating"
    );
    mesh.tangents.clear(); // LE shape encoding has no tangent block.
    for target in [NifTarget::Le, NifTarget::Se] {
        let output = write_shape_with_policy(&mesh, target, InfluencePolicy::NativeFour)
            .expect("explicit native conversion writes the accepted body");
        let dropped_vertices: Vec<_> = output
            .dropped_influences
            .iter()
            .map(|drop| drop.vertex)
            .collect();
        assert_eq!(dropped_vertices, [818, 960, 2821, 2849, 2851]);
        let reread = read(&output.bytes).expect("the encoded body parses");
        let decoded = reread.shapes[0].skin.as_ref().expect("skin is present");
        assert_eq!(decoded.bones, skin.bones);
        assert_eq!(decoded.influences.rows(), skin.influences.rows());
        if target == NifTarget::Le {
            assert_eq!(decoded.influences, skin.influences);
            continue;
        }
        for drop in &output.dropped_influences {
            let row = skin.influences.row(drop.vertex).expect("source row");
            let encoded = decoded.influences.row(drop.vertex).expect("encoded row");
            let (joint, weight) = row
                .joints
                .iter()
                .zip(row.weights)
                .min_by(|(_, left), (_, right)| left.total_cmp(right))
                .expect("the weakest source influence");
            assert_eq!(drop.bone, skin.bones[*joint as usize]);
            assert_eq!(drop.weight, *weight);
            assert_eq!(encoded.joints.len(), 4);
            assert!(!encoded.joints.contains(joint));
        }
    }
}
