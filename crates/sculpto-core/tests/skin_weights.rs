//! Authored influence rows through the model: construction, split, projection.
//!
//! The property under test is that a source row keeps every influence and its
//! exact value from construction through splitting, while the normalized view is
//! a projection that never rewrites the stored rows.

use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::skin::{SkinWeightError, SkinWeights};

/// Three triangles in three partitions, one row per vertex; the middle vertex
/// authors five influences, the last one a single influence.
fn five_influence_mesh() -> MeshAsset {
    let positions = (0..5).map(|index| [index as f32, 0.0, 0.0]).collect();
    MeshAsset {
        name: "witness".into(),
        positions,
        triangles: vec![[0, 1, 2], [1, 2, 3], [2, 3, 4]],
        triangle_partitions: vec![PartitionId(0), PartitionId(1), PartitionId(2)],
        skin: Some(Skin {
            bones: (0..6).map(|bone| format!("bone{bone}")).collect(),
            bind_inverse: Vec::new(),
            influences: SkinWeights::new(
                vec![0, 1, 6, 9, 12, 13],
                vec![0, 2, 3, 4, 1, 2, 3, 4, 5, 1, 2, 3, 4],
                vec![
                    1.0, 0.5, 0.25, 0.125, 0.125, 2.0e-16, 0.6, 0.2, 0.2, 0.4, 0.3, 0.2, 0.1,
                ],
                6,
            )
            .expect("five rows, one with five influences"),
        }),
        ..MeshAsset::default()
    }
}

#[test]
fn a_five_influence_row_survives_construction() {
    let mesh = five_influence_mesh();
    mesh.validate()
        .expect("a five-influence body is a valid model");
    let skin = mesh.skin.as_ref().expect("skin");
    assert_eq!(skin.influences.rows(), 5);
    let row = skin.influences.row(1).expect("vertex 1");
    assert_eq!(row.joints, &[2, 3, 4, 1, 2]);
    assert_eq!(row.weights[4], 2.0e-16);
    assert!((skin.influences.row_sum(1).expect("vertex 1") - 1.0).abs() < 1.0e-6);
}

#[test]
fn splitting_keeps_every_influence_of_the_gathered_rows() {
    let mesh = five_influence_mesh();
    let shapes = mesh.split_for_limit(3).expect("splits into sub-shapes");
    assert_eq!(shapes.len(), 3, "one shape per partition");
    for shape in &shapes {
        shape.validate().expect("every split shape is valid");
        let skin = shape.skin.as_ref().expect("a split keeps the skin");
        assert_eq!(skin.influences.rows(), shape.positions.len());
        assert_eq!(skin.bones, mesh.skin.as_ref().expect("skin").bones);
    }

    let middle = &shapes[1];
    let skin = middle.skin.as_ref().expect("skin");
    // Partition 1 gathers vertices 1..=3; the five-influence row is now local 0.
    let row = skin.influences.row(0).expect("local vertex 0");
    assert_eq!(row.joints, &[2, 3, 4, 1, 2]);
    assert_eq!(row.weights[4], 2.0e-16);
}

#[test]
fn normalization_is_a_projection_not_a_rewrite() {
    let mut mesh = five_influence_mesh();
    let stored = mesh
        .skin
        .as_ref()
        .expect("skin")
        .influences
        .row(3)
        .expect("vertex 3")
        .weights
        .to_vec();
    let skin = mesh.skin.as_ref().expect("skin");
    let lane = skin
        .influences
        .normalized_four_lane(3)
        .expect("the row fits four lanes");
    let sum: f32 = lane.weights.iter().sum();
    assert!((sum - 1.0).abs() < 1.0e-6);
    assert_eq!(
        mesh.skin
            .as_ref()
            .expect("skin")
            .influences
            .row(3)
            .expect("vertex 3")
            .weights,
        stored.as_slice(),
        "the derived view leaves the stored row alone"
    );

    mesh.normalize_weights()
        .expect("an explicit rewrite succeeds");
    let rewritten = mesh
        .skin
        .as_ref()
        .expect("skin")
        .influences
        .row_sum(3)
        .expect("vertex 3");
    assert!((rewritten - 1.0).abs() < 1.0e-6);
}

#[test]
fn a_row_above_four_lanes_is_refused_by_the_projection() {
    let mesh = five_influence_mesh();
    let skin = mesh.skin.as_ref().expect("skin");
    assert_eq!(
        skin.influences.four_lane(1),
        Err(SkinWeightError::TooManyInfluences {
            row: 1,
            influences: 5
        })
    );
    assert!(skin.influences.four_lane(2).is_ok());
}

#[test]
fn malformed_rows_are_refused_at_construction() {
    let palette = 3;
    assert_eq!(
        SkinWeights::new(vec![1, 1], Vec::new(), Vec::new(), palette),
        Err(SkinWeightError::OffsetsStart)
    );
    assert_eq!(
        SkinWeights::new(vec![0, 2, 1], vec![0, 0, 0], vec![1.0; 3], palette),
        Err(SkinWeightError::OffsetsOrder {
            row: 2,
            previous: 2,
            offset: 1
        })
    );
    assert!(matches!(
        SkinWeights::new(vec![0, 1], vec![9], vec![1.0], palette),
        Err(SkinWeightError::PaletteIndex { joint: 9, .. })
    ));
    assert!(matches!(
        SkinWeights::new(vec![0, 1], vec![0], vec![f32::NAN], palette),
        Err(SkinWeightError::NonFinite { .. })
    ));
    assert!(matches!(
        SkinWeights::new(vec![0, 1], vec![0], vec![-0.25], palette),
        Err(SkinWeightError::Negative { .. })
    ));
    assert!(matches!(
        SkinWeights::new(vec![0, 1], vec![0], vec![0.0], palette),
        Err(SkinWeightError::BadSum { .. })
    ));
    assert!(matches!(
        SkinWeights::new(vec![0, 1, 1], vec![0], vec![1.0], palette),
        Err(SkinWeightError::EmptyRow { .. })
    ));
}

#[test]
fn a_row_shorter_than_the_arena_is_a_skin_length_defect() {
    let mut mesh = five_influence_mesh();
    mesh.skin.as_mut().expect("skin").influences =
        SkinWeights::from_quads(&[[0, 0, 0, 0]; 4], &[[1.0, 0.0, 0.0, 0.0]; 4]).expect("four rows");
    assert_eq!(
        mesh.validate(),
        Err(sculpto_core::mesh::MeshError::SkinLengthMismatch)
    );
}

#[test]
fn a_stored_sum_below_one_is_data_and_the_unit_view_reports_it() {
    let mut mesh = five_influence_mesh();
    mesh.skin.as_mut().expect("skin").influences =
        SkinWeights::from_quads(&[[0, 1, 0, 0]; 5], &[[0.5, 0.25, 0.0, 0.0]; 5])
            .expect("five rows");
    mesh.validate()
        .expect("a stored sum of 0.75 is structural data");
    assert_eq!(
        mesh.validate_unit_weights(),
        Err(sculpto_core::mesh::MeshError::WeightSumOutOfRange {
            vertex: 0,
            sum: 0.75
        })
    );
}

#[test]
fn a_small_positive_row_normalizes_without_reciprocal_overflow() {
    let tiny = f32::from_bits(1);
    let mut weights =
        SkinWeights::new(vec![0, 1], vec![0], vec![tiny], 1).expect("positive finite source row");
    let evaluated: Vec<_> = weights
        .normalized_row(0)
        .expect("normalized view")
        .collect();
    assert_eq!(evaluated, [(0, 1.0)]);
    assert_eq!(
        weights
            .normalized_four_lane(0)
            .expect("packed view")
            .weights,
        [1., 0., 0., 0.]
    );
    assert_eq!(weights.row(0).expect("source row").weights, [tiny]);
    weights.normalize().expect("explicit normalization");
    assert_eq!(weights.row(0).expect("edited row").weights, [1.]);
}
