//! Acceptance for the reference bone-weight copy.
//!
//! Every case is synthetic and hand-computed from
//! `OutfitProject::CopyBoneWeights` (`OutfitProject.cpp:2820-2928`) and
//! `ProjectOperations::CopyBoneWeights` (`ProjectOperations.cpp:649-800`): one
//! sparse weight field per copied bone, inverse-distance blended over the
//! proximity cache, masked on both sides, then normalized and packed.

use sculpto_mesh::delta_conform::DeltaConformError;
use sculpto_mesh::garment::{Influences, TransferError, TransferOptions, transfer};
use sculpto_mesh::weight_norm::{MAX_INFLUENCES, SUM_TOLERANCE, WeightError};

fn influences(pairs: &[(&str, f32)]) -> Influences {
    pairs
        .iter()
        .map(|(bone, weight)| ((*bone).to_string(), *weight))
        .collect()
}

fn close(actual: f32, expected: f32, label: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{label}: {actual} is not {expected}"
    );
}

fn mass(influences: &Influences) -> f32 {
    influences.values().sum()
}

#[test]
fn transfer_splits_equal_distances_three_quarters_to_one_quarter() {
    let body = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]];
    let weights = [
        influences(&[("Chest", 1.0), ("Spine", 0.0)]),
        influences(&[("Chest", 0.5), ("Spine", 0.5)]),
    ];
    let garment = [[0.0, 0.0, 0.0]];
    let transferred = transfer(&garment, &body, &weights, &TransferOptions::default())
        .expect("the copy should answer");
    // Inverse distances are both one: Chest (1 + 0.5) / 2, Spine (0 + 0.5) / 2.
    close(transferred[0]["Chest"], 0.75, "Chest");
    close(transferred[0]["Spine"], 0.25, "Spine");
    close(mass(&transferred[0]), 1.0, "sum");
}

#[test]
fn transfer_cancels_a_bone_missing_from_the_nearest_entry() {
    let body = [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]];
    let weights = [
        influences(&[("Chest", 1.0)]),
        influences(&[("Chest", 0.5), ("Spine", 0.5)]),
    ];
    let garment = [[0.0, 0.0, 0.0]];
    let transferred = transfer(&garment, &body, &weights, &TransferOptions::default())
        .expect("the copy should answer");
    // The nearest body vertex has no Spine entry, so Spine cancels for the whole
    // vertex (Automorph.cpp:605-609) instead of blending from the farther one.
    assert_eq!(
        transferred[0]
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["Chest"]
    );
    close(transferred[0]["Chest"], 1.0, "Chest");
}

#[test]
fn transfer_skips_a_farther_missing_entry() {
    let body = [[1.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
    let weights = [
        influences(&[("Chest", 1.0), ("Hips", 1.0)]),
        influences(&[("Chest", 0.0)]),
    ];
    let garment = [[0.0, 0.0, 0.0]];
    let transferred = transfer(&garment, &body, &weights, &TransferOptions::default())
        .expect("the copy should answer");
    // Chest sees both fields (inverse distances 1 and 0.25) and lands on 0.8;
    // Hips sees only the nearest, so its farther absent entry is skipped rather
    // than counted as an explicit zero. The 4:5 ratio survives normalization.
    close(transferred[0]["Chest"], 4.0 / 9.0, "Chest");
    close(transferred[0]["Hips"], 5.0 / 9.0, "Hips");
}

#[test]
fn transfer_excludes_masked_body_vertices_before_the_cap() {
    let body = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let weights = [
        influences(&[("Chest", 0.2), ("Spine", 0.8)]),
        influences(&[("Chest", 0.8), ("Spine", 0.2)]),
    ];
    let garment = [[0.0, 0.0, 0.0]];
    let blended = transfer(&garment, &body, &weights, &TransferOptions::default())
        .expect("the copy should answer");
    // Inverse distances 1 and 0.5: (0.2 + 0.4) / 1.5 and (0.8 + 0.1) / 1.5.
    close(blended[0]["Chest"], 0.4, "unmasked Chest");
    close(blended[0]["Spine"], 0.6, "unmasked Spine");

    let masked = TransferOptions {
        body_mask: Some(vec![1.0, 0.0]),
        max_results: 1,
        ..TransferOptions::default()
    };
    let excluded = transfer(&garment, &body, &weights, &masked).expect("the copy should answer");
    // The cap keeps one neighbour and the exclusion ran before it, so the kept
    // neighbour is the second body vertex (Automorph.cpp:227-246).
    close(excluded[0]["Chest"], 0.8, "masked Chest");
    close(excluded[0]["Spine"], 0.2, "masked Spine");
}

#[test]
fn transfer_keeps_a_masked_garment_vertex() {
    let body = [[0.0, 0.0, 0.0]];
    let weights = [influences(&[("Chest", 1.0)])];
    let garment = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let current = vec![
        influences(&[("Hips", 0.7), ("Spine", 0.3)]),
        influences(&[("Hips", 0.4)]),
    ];
    let options = TransferOptions {
        garment_mask: Some(vec![1.0, 0.0]),
        current: Some(current.clone()),
        ..TransferOptions::default()
    };
    let transferred =
        transfer(&garment, &body, &weights, &options).expect("the copy should answer");
    // The masked vertex is never zeroed, normalized or packed (OutfitProject.cpp:2891).
    assert_eq!(transferred[0], current[0]);
    close(transferred[1]["Hips"], 0.4, "locked Hips");
    close(transferred[1]["Chest"], 0.6, "copied Chest");
}

#[test]
fn transfer_scales_copied_mass_around_a_locked_bone() {
    let body = [[0.0, 0.0, 0.0]];
    let weights = [influences(&[("Chest", 0.9)])];
    let garment = [[0.1, 0.0, 0.0]];
    let spine = 0.3f32;
    let options = TransferOptions {
        copy: vec!["Chest".to_string()],
        current: Some(vec![influences(&[("Spine", spine)])]),
        ..TransferOptions::default()
    };
    let transferred =
        transfer(&garment, &body, &weights, &options).expect("the copy should answer");
    assert_eq!(transferred[0]["Spine"].to_bits(), spine.to_bits());
    close(transferred[0]["Chest"], 0.7, "scaled Chest");
    assert_eq!(transferred[0].len(), 2);
    close(mass(&transferred[0]), 1.0, "sum");
}

#[test]
fn transfer_with_one_neighbour_reproduces_the_nearest_regression() {
    let body = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let weights = [
        influences(&[("Chest", 2.0)]),
        influences(&[("Chest", 0.25), ("Hips", 0.75)]),
    ];
    let garment = [[0.1, 0.0, 0.0], [0.9, 0.0, 0.0]];
    let options = TransferOptions {
        max_results: 1,
        ..TransferOptions::default()
    };
    let transferred =
        transfer(&garment, &body, &weights, &options).expect("the copy should answer");
    // The published nearest copy: Chest 2 becomes 1, the second vertex keeps the
    // nearest body weights unchanged because they already sum to one.
    assert_eq!(transferred[0], influences(&[("Chest", 1.0)]));
    assert_eq!(
        transferred[1],
        influences(&[("Chest", 0.25), ("Hips", 0.75)])
    );
}

#[test]
fn transfer_output_is_capped_and_sums_to_one() {
    let body = [[0.0, 0.0, 0.0]];
    let weights = [influences(&[
        ("A", 0.1),
        ("B", 0.2),
        ("C", 0.3),
        ("D", 0.1),
        ("E", 0.1),
        ("F", 0.2),
    ])];
    let garment = [[0.0, 0.0, 0.0]];
    let transferred = transfer(&garment, &body, &weights, &TransferOptions::default())
        .expect("the copy should answer");
    let packed = &transferred[0];
    assert_eq!(packed.len(), MAX_INFLUENCES);
    assert!(
        (mass(packed) - 1.0).abs() <= SUM_TOLERANCE,
        "{packed:?} does not sum to one"
    );
    assert_eq!(
        packed.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["A", "B", "C", "F"]
    );
}

#[test]
fn transfer_refuses_invalid_inputs() {
    let body = [[1.0, 0.0, 0.0]];
    let weights = [influences(&[("Chest", 1.0)])];
    let garment = [[0.0, 0.0, 0.0]];

    assert!(matches!(
        transfer(&[], &body, &weights, &TransferOptions::default()),
        Err(TransferError::EmptyGarment)
    ));
    assert!(matches!(
        transfer(&garment, &body, &[], &TransferOptions::default()),
        Err(TransferError::LengthMismatch {
            field: "body_weights",
            ..
        })
    ));
    let no_neighbours = TransferOptions {
        max_results: 0,
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &no_neighbours),
        Err(TransferError::ZeroMaxResults)
    ));
    let negative_radius = TransferOptions {
        radius: -1.0,
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &negative_radius),
        Err(TransferError::Proximity(
            DeltaConformError::InvalidRadius { .. }
        ))
    ));

    let short_mask = TransferOptions {
        garment_mask: Some(vec![0.0, 1.0]),
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &short_mask),
        Err(TransferError::LengthMismatch {
            field: "garment_mask",
            expected: 1,
            actual: 2,
        })
    ));
    let out_of_range = TransferOptions {
        garment_mask: Some(vec![1.5]),
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &out_of_range),
        Err(TransferError::Mask { .. })
    ));
    let masked_without_current = TransferOptions {
        garment_mask: Some(vec![1.0]),
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &masked_without_current),
        Err(TransferError::MissingCurrentWeights { vertex: 0 })
    ));
    let short_current = TransferOptions {
        current: Some(vec![influences(&[("Hips", 0.5)]); 2]),
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &short_current),
        Err(TransferError::LengthMismatch {
            field: "current_weights",
            ..
        })
    ));

    let negative_weight = [influences(&[("Chest", -1.0)])];
    assert!(matches!(
        transfer(
            &garment,
            &body,
            &negative_weight,
            &TransferOptions::default()
        ),
        Err(TransferError::Influence {
            field: "body_weights",
            vertex: 0,
            error: WeightError::NegativeWeight { .. }
        })
    ));
    let non_finite = [influences(&[("Chest", f32::NAN)])];
    assert!(matches!(
        transfer(&garment, &body, &non_finite, &TransferOptions::default()),
        Err(TransferError::Influence {
            field: "body_weights",
            error: WeightError::NonFiniteWeight { .. },
            ..
        })
    ));

    let duplicate_selection = TransferOptions {
        copy: vec!["Chest".to_string()],
        normalizable: vec!["Chest".to_string()],
        ..TransferOptions::default()
    };
    assert!(matches!(
        transfer(&garment, &body, &weights, &duplicate_selection),
        Err(TransferError::Normalize(WeightError::DuplicateBone { .. }))
    ));

    let unweighted_body = [Influences::new()];
    assert!(matches!(
        transfer(
            &garment,
            &body,
            &unweighted_body,
            &TransferOptions::default()
        ),
        Err(TransferError::Vertex {
            vertex: 0,
            error: WeightError::MissingUnlockedMass { .. }
        })
    ));
}

#[test]
fn regression_unnormalized_multiple_bones_keep_their_proportions() {
    let weights = [influences(&[("Chest", 2.0), ("Spine", 0.25)])];
    let options = TransferOptions {
        max_results: 1,
        ..Default::default()
    };
    let result = transfer(&[[0.1, 0.0, 0.0]], &[[0.0; 3]], &weights, &options).unwrap();
    close(result[0]["Chest"], 8.0 / 9.0, "legacy Chest proportion");
    close(result[0]["Spine"], 1.0 / 9.0, "legacy Spine proportion");
}

#[test]
fn regression_one_neighbor_retains_small_positive_influences() {
    let weights = [influences(&[("Chest", 0.9995), ("Spine", 0.0005)])];
    let options = TransferOptions {
        max_results: 1,
        ..Default::default()
    };
    let result = transfer(&[[0.1, 0.0, 0.0]], &[[0.0; 3]], &weights, &options).unwrap();
    assert_eq!(result[0], weights[0]);
}

#[test]
fn regression_mask_cannot_preserve_an_underweight_input_as_valid_output() {
    let options = TransferOptions {
        garment_mask: Some(vec![1.0]),
        current: Some(vec![influences(&[("Chest", 0.5)])]),
        ..Default::default()
    };
    assert!(
        transfer(
            &[[0.0; 3]],
            &[[0.0; 3]],
            &[influences(&[("Chest", 1.0)])],
            &options
        )
        .is_err()
    );
}

#[test]
fn regression_mask_cannot_preserve_five_positive_influences_as_valid_output() {
    let options = TransferOptions {
        garment_mask: Some(vec![0.5]),
        current: Some(vec![influences(&[
            ("A", 0.2),
            ("B", 0.2),
            ("C", 0.2),
            ("D", 0.2),
            ("E", 0.2),
        ])]),
        ..Default::default()
    };
    assert!(
        transfer(
            &[[0.0; 3]],
            &[[0.0; 3]],
            &[influences(&[("Chest", 1.0)])],
            &options
        )
        .is_err()
    );
}
