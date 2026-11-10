//! Acceptance for the delta transfer rules of `Automorph::GenerateResultDiff`.
//!
//! Every case is synthetic and hand-computed: the reference blends inverse
//! distances and compares against nifly's epsilon, so the expected numbers are
//! closed-form instead of recorded from a reference run.

use std::collections::{BTreeMap, BTreeSet};

use sculpto_mesh::delta_conform::{
    DeltaConformError, DeltaOptions, Neighbor, ProximityCache, ProximityOptions, generate_delta,
};

fn build_cache(body: &[[f32; 3]], garment: &[[f32; 3]]) -> ProximityCache {
    ProximityCache::build(body, garment, &ProximityOptions::default(), None)
        .expect("the fixture cache should build")
}

fn diffs(entries: &[(usize, [f32; 3])]) -> BTreeMap<usize, [f32; 3]> {
    entries.iter().copied().collect()
}

fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
    for (axis, (left, right)) in actual.iter().zip(&expected).enumerate() {
        assert!(
            (left - right).abs() < 1e-6,
            "axis {axis}: {actual:?} is not {expected:?}"
        );
    }
}

fn assert_moved(delta: &BTreeMap<usize, [f32; 3]>, vertex: usize, expected: [f32; 3]) {
    let actual = delta
        .get(&vertex)
        .copied()
        .unwrap_or_else(|| panic!("garment vertex {vertex} should move: {delta:?}"));
    assert_close(actual, expected);
}

#[test]
fn two_movers_blend_by_inverse_distance() {
    let body = [[1.0, 0.0, 0.0], [4.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let cache = build_cache(&body, &garment);

    assert_eq!(cache.body_vertices(), 2);
    assert_eq!(cache.garment_vertices(), 1);
    let neighbors: &[Neighbor] = cache
        .neighbors(0)
        .expect("the only garment vertex has neighbours");
    assert_eq!(neighbors.len(), 2);
    assert!((neighbors[0].distance - 1.0).abs() < 1e-6);
    assert!((neighbors[1].distance - 4.0).abs() < 1e-6);
    assert_eq!(neighbors[0].reference_x, 1.0);
    assert_eq!(cache.neighbors(1), None);

    let body_diffs = diffs(&[(0, [1.0, 0.0, 0.0]), (1, [0.0, 1.0, 0.0])]);
    let delta = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    // Inverse distances 1 and 1/4 normalise to 0.8 and 0.2.
    assert_moved(&delta, 0, [0.8, 0.2, 0.0]);
    assert_eq!(delta.len(), 1);
}

#[test]
fn exact_match_uses_weight_1000() {
    let body = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let cache = build_cache(&body, &garment);
    let body_diffs = diffs(&[(0, [1.0, 0.0, 0.0]), (1, [0.0, 1.0, 0.0])]);

    let delta = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    // The exact match weighs 1000, not an infinite 1/distance, so it wins but
    // does not erase the neighbour one unit away.
    assert_moved(&delta, 0, [1000.0 / 1001.0, 1.0 / 1001.0, 0.0]);
}

#[test]
fn closest_vertex_without_delta_zeroes_the_result() {
    let garment = [[0.0, 0.0, 0.0]];
    let cache = build_cache(&[[1.0, 0.0, 0.0], [3.0, 0.0, 0.0]], &garment);
    // Only the far body vertex moved.
    let delta = generate_delta(
        &cache,
        &diffs(&[(1, [6.0, 0.0, 0.0])]),
        &DeltaOptions::default(),
    )
    .expect("a valid cache and diff set should transfer");
    assert!(
        delta.is_empty(),
        "the closest neighbour did not move, so the vertex stays: {delta:?}"
    );

    // Without the unmoved close neighbour the same offset is copied outright.
    let cache = build_cache(&[[3.0, 0.0, 0.0]], &garment);
    let delta = generate_delta(
        &cache,
        &diffs(&[(0, [6.0, 0.0, 0.0])]),
        &DeltaOptions::default(),
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [6.0, 0.0, 0.0]);
}

#[test]
fn radius_limits_the_neighbours() {
    let body = [[1.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let cache = ProximityCache::build(&body, &garment, &ProximityOptions { radius: 5.0 }, None)
        .expect("the fixture cache should build");
    assert_eq!(cache.neighbors(0).expect("neighbours are cached").len(), 1);

    let body_diffs = diffs(&[(0, [2.0, 0.0, 0.0]), (1, [9.0, 0.0, 0.0])]);
    let delta = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [2.0, 0.0, 0.0]);
    assert_eq!(delta.len(), 1);
}

#[test]
fn zero_radius_follows_noncoincident_body_and_retains_native_record_minima() {
    let body = [[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
    let garment = [[0.1, 0.0, 0.0]];
    let cache =
        ProximityCache::build(&body, &garment, &ProximityOptions { radius: 0.0 }, None).unwrap();
    let row = cache.neighbors(0).unwrap();
    // Native visits child 1, then the closer root 0; it keeps both record minima.
    assert_eq!(
        row.iter()
            .map(|neighbor| neighbor.vertex)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let diffs = BTreeMap::from([(0, [1.0, 0.0, 0.0]), (1, [3.0, 0.0, 0.0])]);
    let output = generate_delta(&cache, &diffs, &DeltaOptions::default()).unwrap();
    let inverse_far = 1.0 / (0.9_f32 * 0.9 + 1.0).sqrt();
    let expected = (10.0 + 3.0 * inverse_far) / (10.0 + inverse_far);
    assert!((output[&0][0] - expected).abs() < 1e-6);
    assert_ne!(output[&0][0], 1.0);
    let positive =
        ProximityCache::build(&body, &garment, &ProximityOptions { radius: 0.01 }, None).unwrap();
    assert!(positive.neighbors(0).unwrap().is_empty());
}

#[test]
fn zero_radius_exclusion_happens_after_native_query() {
    let body = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let garment = [[1.1, 0.0, 0.0]];
    let excluded = BTreeSet::from([0]);
    let cache = ProximityCache::build(
        &body,
        &garment,
        &ProximityOptions { radius: 0.0 },
        Some(&excluded),
    )
    .unwrap();
    assert!(
        cache
            .neighbors(0)
            .unwrap()
            .iter()
            .all(|neighbor| neighbor.vertex != 0)
    );
    let capped = generate_delta(
        &cache,
        &BTreeMap::from([(1, [2.0, 0.0, 0.0])]),
        &DeltaOptions {
            max_results: 1,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(capped[&0], [2.0, 0.0, 0.0]);
}

#[test]
fn excluded_body_vertices_leave_the_cache() {
    let body = [[1.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let excluded = BTreeSet::from([0usize]);
    let cache = ProximityCache::build(
        &body,
        &garment,
        &ProximityOptions { radius: 25.0 },
        Some(&excluded),
    )
    .expect("the fixture cache should build");
    assert_eq!(cache.neighbors(0).expect("neighbours are cached").len(), 1);

    let body_diffs = diffs(&[(0, [2.0, 0.0, 0.0]), (1, [9.0, 0.0, 0.0])]);
    let delta = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [9.0, 0.0, 0.0]);
}

#[test]
fn exclusion_applies_before_the_neighbour_cap() {
    let body = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [3.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let excluded = BTreeSet::from([0usize]);
    let cache = ProximityCache::build(
        &body,
        &garment,
        &ProximityOptions::default(),
        Some(&excluded),
    )
    .expect("the fixture cache should build");

    let body_diffs = diffs(&[
        (0, [100.0, 0.0, 0.0]),
        (1, [7.0, 0.0, 0.0]),
        (2, [9.0, 0.0, 0.0]),
    ]);
    let options = DeltaOptions {
        max_results: 1,
        ..DeltaOptions::default()
    };
    let delta = generate_delta(&cache, &body_diffs, &options)
        .expect("a valid cache and diff set should transfer");
    // The excluded nearest vertex is gone before the cap picks one neighbour.
    assert_moved(&delta, 0, [7.0, 0.0, 0.0]);
}

#[test]
fn neighbour_cap_keeps_only_the_nearest() {
    let body = [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    let cache = build_cache(&body, &garment);
    let body_diffs = diffs(&[(0, [2.0, 0.0, 0.0]), (1, [100.0, 0.0, 0.0])]);

    let capped = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            max_results: 1,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&capped, 0, [2.0, 0.0, 0.0]);

    let blended = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    // Inverse distances 1 and 1/2 normalise to 2/3 and 1/3.
    assert_moved(
        &blended,
        0,
        [2.0 * (1.0 / 1.5) + 100.0 * (0.5 / 1.5), 0.0, 0.0],
    );
}

#[test]
fn no_squeeze_gates_the_x_axis() {
    let body = [[-1.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, -1.0]];
    let cache = build_cache(&body, &garment);
    let body_diffs = diffs(&[(0, [2.0, 1.0, 3.0])]);

    let free = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    assert_moved(&free, 0, [2.0, 1.0, 3.0]);

    // The body vertex sits on the negative X side, so a positive X offset pushes
    // the garment into the body: only the other axes survive.
    let squeezed = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            no_squeeze: true,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&squeezed, 0, [0.0, 1.0, 3.0]);

    // Matching signs pass the gate.
    let outward = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let options = DeltaOptions {
        no_squeeze: true,
        ..DeltaOptions::default()
    };
    let delta = generate_delta(&outward, &diffs(&[(0, [2.0, 0.0, 0.0])]), &options)
        .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [2.0, 0.0, 0.0]);

    // Zero is not a side: the gate also drops the axis at the body midline.
    let midline = build_cache(&[[0.0, 0.0, 0.0]], &[[0.0, 0.0, 1.0]]);
    let delta = generate_delta(&midline, &diffs(&[(0, [5.0, 0.0, 1.0])]), &options)
        .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [0.0, 0.0, 1.0]);
}

#[test]
fn axis_gates_drop_components() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let body_diffs = diffs(&[(0, [2.0, 3.0, 4.0])]);

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            axis_x: false,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [0.0, 3.0, 4.0]);

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            axis_y: false,
            axis_z: false,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [2.0, 0.0, 0.0]);

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            axis_x: false,
            axis_y: false,
            axis_z: false,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert!(
        delta.is_empty(),
        "every axis is gated, so nothing moves: {delta:?}"
    );
}

#[test]
fn mask_scales_and_locks_vertices() {
    let body = [[1.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let cache = build_cache(&body, &garment);
    let body_diffs = diffs(&[(0, [4.0, 0.0, 0.0])]);

    let free = generate_delta(&cache, &body_diffs, &DeltaOptions::default())
        .expect("a valid cache and diff set should transfer");
    assert_moved(&free, 0, [4.0, 0.0, 0.0]);
    assert_moved(&free, 1, [4.0, 0.0, 0.0]);

    let masked = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![1.0, 0.5]),
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    // A locked vertex keeps its position and is absent from the delta set.
    assert!(
        !masked.contains_key(&0),
        "a locked vertex must not move: {masked:?}"
    );
    assert_moved(&masked, 1, [2.0, 0.0, 0.0]);
}

#[test]
fn solid_mode_applies_the_per_axis_extremes() {
    let body = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
    let garment = body;
    let cache = ProximityCache::build(&body, &garment, &ProximityOptions { radius: 0.5 }, None)
        .expect("the fixture cache should build");
    let body_diffs = diffs(&[
        (0, [4.0, 0.0, -1.0]),
        (1, [-2.0, 5.0, -3.0]),
        (2, [0.0, 0.0, 7.0]),
    ]);

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            solid_mode: true,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    // X: +4 with -2, Y: +5 only, Z: +7 with -3 collapse into one move.
    assert_eq!(delta.len(), 3);
    for vertex in 0..3 {
        assert_moved(&delta, vertex, [2.0, 5.0, 4.0]);
    }
}

#[test]
fn solid_mode_applies_the_mask_twice() {
    let body = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [20.0, 0.0, 0.0]];
    let garment = body;
    let cache = ProximityCache::build(&body, &garment, &ProximityOptions { radius: 0.5 }, None)
        .expect("the fixture cache should build");
    let body_diffs = diffs(&[
        (0, [4.0, 0.0, -1.0]),
        (1, [-2.0, 5.0, -3.0]),
        (2, [0.0, 0.0, 7.0]),
    ]);

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![0.5, 0.0, 1.0]),
            solid_mode: true,
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");

    // Collection contributes [2, 0, -0.5] and [-2, 5, -3]; the locked vertex
    // adds nothing, so the extremes are X 0, Y 5, Z -3. The mask then scales the
    // applied move again, halving it for the half-masked vertex
    // (Automorph.cpp:621-622 and 663-664).
    assert_eq!(delta.len(), 2);
    assert_moved(&delta, 0, [0.0, 2.5, -1.5]);
    assert_moved(&delta, 1, [0.0, 5.0, -3.0]);
    assert!(
        !delta.contains_key(&2),
        "a locked vertex must not move: {delta:?}"
    );
}

#[test]
fn transform_rotates_the_generated_move() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let body_diffs = diffs(&[(0, [2.0, 3.0, 4.0])]);
    // Column-major: X and Y swap, Z negates.
    let transform = [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]];

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            transform: Some(transform),
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [3.0, 2.0, -4.0]);

    // The mask scales the move before the transform.
    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![0.5]),
            transform: Some(transform),
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    assert_moved(&delta, 0, [1.5, 1.0, -2.0]);
}

#[test]
fn negligible_move_is_dropped_before_the_transform() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let body_diffs = diffs(&[(0, [0.00005, 0.0, 0.0])]);
    let amplify = [[1000.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    let delta = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            transform: Some(amplify),
            ..DeltaOptions::default()
        },
    )
    .expect("a valid cache and diff set should transfer");
    // The reference tests its epsilon before transforming, so the amplified
    // 0.05 is never produced.
    assert!(
        delta.is_empty(),
        "a move below the nifly epsilon is dropped: {delta:?}"
    );
}

#[test]
fn invalid_radius_is_refused() {
    let body = [[1.0, 0.0, 0.0]];
    let garment = [[0.0, 0.0, 0.0]];
    for radius in [-1.0, f32::NAN, f32::INFINITY] {
        let error = ProximityCache::build(&body, &garment, &ProximityOptions { radius }, None)
            .expect_err("a broken radius must not build a cache");
        assert!(
            matches!(error, DeltaConformError::InvalidRadius { .. }),
            "radius {radius} gave {error:?}"
        );
    }
}

#[test]
fn empty_inputs_are_refused() {
    let options = ProximityOptions::default();
    let error = ProximityCache::build(&[], &[[0.0, 0.0, 0.0]], &options, None)
        .expect_err("a body without vertices cannot be bound");
    assert_eq!(error, DeltaConformError::EmptyBody);

    let error = ProximityCache::build(&[[0.0, 0.0, 0.0]], &[], &options, None)
        .expect_err("a garment without vertices cannot be conformed");
    assert_eq!(error, DeltaConformError::EmptyGarment);
}

#[test]
fn out_of_range_exclusion_is_refused() {
    let excluded = BTreeSet::from([1usize]);
    let error = ProximityCache::build(
        &[[0.0, 0.0, 0.0]],
        &[[0.0, 0.0, 0.0]],
        &ProximityOptions::default(),
        Some(&excluded),
    )
    .expect_err("an exclusion outside the body is a caller error");
    assert_eq!(
        error,
        DeltaConformError::ExcludedOutOfRange {
            index: 1,
            body_vertices: 1
        }
    );
}

#[test]
fn non_finite_positions_are_refused() {
    let options = ProximityOptions::default();
    let error = ProximityCache::build(&[[f32::NAN, 0.0, 0.0]], &[[0.0, 0.0, 0.0]], &options, None)
        .expect_err("a non-finite body position cannot be indexed");
    assert_eq!(
        error,
        DeltaConformError::NonFiniteBodyPosition { vertex: 0 }
    );

    let error = ProximityCache::build(
        &[[0.0, 0.0, 0.0]],
        &[[0.0, f32::INFINITY, 0.0]],
        &options,
        None,
    )
    .expect_err("a non-finite garment position cannot be conformed");
    assert_eq!(
        error,
        DeltaConformError::NonFiniteGarmentPosition { vertex: 0 }
    );
}

#[test]
fn invalid_options_are_refused() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let body_diffs = diffs(&[(0, [1.0, 0.0, 0.0])]);

    let error = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            max_results: 0,
            ..DeltaOptions::default()
        },
    )
    .expect_err("a zero cap cannot blend anything");
    assert_eq!(error, DeltaConformError::ZeroNeighborCap);

    let error = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![0.0, 0.0]),
            ..DeltaOptions::default()
        },
    )
    .expect_err("a mask must cover exactly the garment vertices");
    assert_eq!(
        error,
        DeltaConformError::MaskLength {
            expected: 1,
            actual: 2
        }
    );

    let error = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![1.5]),
            ..DeltaOptions::default()
        },
    )
    .expect_err("a mask weight above one is not a lock");
    assert_eq!(
        error,
        DeltaConformError::MaskWeight {
            vertex: 0,
            weight: 1.5
        }
    );

    let error = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            mask: Some(vec![f32::NAN]),
            ..DeltaOptions::default()
        },
    )
    .expect_err("a non-finite mask weight is not a lock");
    assert!(
        matches!(error, DeltaConformError::MaskWeight { vertex: 0, .. }),
        "a NaN weight gave {error:?}"
    );

    let broken = [[f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let error = generate_delta(
        &cache,
        &body_diffs,
        &DeltaOptions {
            transform: Some(broken),
            ..DeltaOptions::default()
        },
    )
    .expect_err("a non-finite transform cannot move anything");
    assert_eq!(
        error,
        DeltaConformError::NonFiniteTransform { column: 0, row: 0 }
    );
}

#[test]
fn invalid_body_diffs_are_refused() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);

    let error = generate_delta(
        &cache,
        &diffs(&[(1, [1.0, 0.0, 0.0])]),
        &DeltaOptions::default(),
    )
    .expect_err("a body offset outside the body is a caller error");
    assert_eq!(
        error,
        DeltaConformError::DiffOutOfRange {
            index: 1,
            body_vertices: 1
        }
    );

    let error = generate_delta(
        &cache,
        &diffs(&[(0, [f32::NAN, 0.0, 0.0])]),
        &DeltaOptions::default(),
    )
    .expect_err("a non-finite body offset cannot be blended");
    assert_eq!(error, DeltaConformError::NonFiniteDiff { vertex: 0 });
}

#[test]
fn empty_diff_set_moves_nothing() {
    let cache = build_cache(&[[1.0, 0.0, 0.0]], &[[0.0, 0.0, 0.0]]);
    let delta = generate_delta(&cache, &BTreeMap::new(), &DeltaOptions::default())
        .expect("a slider without offsets is a no-op, not a failure");
    assert!(delta.is_empty(), "nothing moved: {delta:?}");
}
