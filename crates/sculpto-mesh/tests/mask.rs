//! Native mask semantics: welded borders precede adjacency, and transfer uses facet corners.

use sculpto_mesh::mask::{self, MaskError, VertexMask};

fn square() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ],
        vec![[0, 1, 2], [1, 3, 2]],
    )
}

#[test]
fn invalid_mask_values_and_edits_are_refused_without_mutation() {
    [f32::NAN, f32::INFINITY, -0.1, 1.1]
        .into_iter()
        .for_each(|value| {
            assert!(matches!(
                VertexMask::from_values(vec![value]),
                Err(MaskError::Value { vertex: 0, .. })
            ));
        });
    let mut mask = VertexMask::zeros(2);
    assert!(matches!(mask.set(2, 0.5), Err(MaskError::Index { .. })));
    assert!(mask.set(0, f32::NAN).is_err());
    assert_eq!(mask.as_slice(), &[0.0, 0.0]);
    assert!(mask.get(2).is_none());
}

#[test]
fn invert_clear_sparse_mask_and_freedom_share_one_convention() {
    let mut mask = VertexMask::from_values(vec![0.0, 0.25, 1.0]).unwrap();
    assert_eq!(
        mask.masked().into_iter().collect::<Vec<_>>(),
        vec![(1, 0.25), (2, 1.0)]
    );
    assert_eq!(mask.freedom(), vec![1.0, 0.75, 0.0]);
    mask.invert();
    assert_eq!(mask.as_slice(), &[1.0, 0.75, 0.0]);
    mask.clear();
    assert!(mask.freedom().is_empty());
    assert!(mask.masked().is_empty());
    assert!(VertexMask::zeros(0).is_empty());
}

#[test]
fn grow_and_shrink_walk_one_adjacency_ring() {
    let (positions, faces) = square();
    let mut mask = VertexMask::from_values(vec![1.0, 0.0, 0.0, 0.0]).unwrap();
    assert_eq!(mask.grow(&positions, &faces).unwrap(), 2);
    assert_eq!(mask.as_slice(), &[1.0, 1.0, 1.0, 0.0]);
    assert_eq!(mask.shrink(&positions, &faces).unwrap(), 2);
    assert_eq!(mask.as_slice(), &[1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn welded_border_is_completed_before_any_adjacency_growth() {
    let positions = [
        [1.0, 0.0, 0.0],
        [1.0 + 5e-7, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
    ];
    let faces = [[0, 2, 3], [1, 2, 3]];
    let mut mask = VertexMask::from_values(vec![1.0, 0.0, 0.0, 0.0]).unwrap();
    assert_eq!(mask.grow(&positions, &faces).unwrap(), 1);
    assert_eq!(mask.as_slice(), &[1.0, 1.0, 0.0, 0.0]);
    assert_eq!(mask.grow(&positions, &faces).unwrap(), 2);
    assert_eq!(mask.as_slice(), &[1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn smooth_uses_a_snapshot_and_keeps_sub_epsilon_changes_out() {
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let mut mask = VertexMask::from_values(vec![1.0, 0.0, 0.0]).unwrap();
    assert_eq!(mask.smooth(&positions, &[[0, 1, 2]], 1e-8).unwrap(), 0);
    assert_eq!(mask.as_slice(), &[1.0, 0.0, 0.0]);
    assert_eq!(mask.smooth(&positions, &[[0, 1, 2]], 0.5).unwrap(), 3);
    assert_eq!(mask.as_slice(), &[0.5, 0.25, 0.25]);
}

#[test]
fn welded_smoothing_preserves_native_peer_only_averaging() {
    let positions = [
        [1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
    ];
    let mut mask = VertexMask::from_values(vec![1.0, 0.0, 0.0, 0.0]).unwrap();
    mask.smooth(&positions, &[[0, 2, 3], [1, 2, 3]], 0.5)
        .unwrap();
    assert_eq!(&mask.as_slice()[..2], &[0.0, 0.5]);
    assert!((mask.as_slice()[2] - 1.0 / 6.0).abs() < 1e-6);
    assert!((mask.as_slice()[3] - 1.0 / 6.0).abs() < 1e-6);
}

#[test]
fn transfer_copies_the_closest_corner_not_the_globally_nearest_vertex() {
    let positions = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [0.0, 10.0, 0.0],
        [6.0, 3.0, 0.1],
    ];
    let source = VertexMask::from_values(vec![0.1, 0.9, 0.5, 0.2]).unwrap();
    let output = mask::transfer_mask(
        &positions,
        &[[0, 1, 2]],
        &source,
        &[[6.0, 3.0, 0.1], [6.0, 3.0, 2.0]],
        0.5,
    )
    .unwrap();
    assert_eq!(output.mask.as_slice(), &[0.9, 0.0]);
    assert_eq!(output.matched, vec![true, false]);
}

#[test]
fn transfer_reports_invalid_facets_and_handles_empty_meshes() {
    let source = VertexMask::from_values(vec![0.2, 0.8, 1.0]).unwrap();
    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let result = mask::transfer_mask(
        &positions,
        &[[0, 1, 2], [0, 1, 9]],
        &source,
        &[[0.0, 0.0, 0.0]],
        1.0,
    )
    .unwrap();
    assert_eq!(result.skipped_triangles, 1);
    assert_eq!(result.mask.as_slice(), &[0.2]);
    let empty =
        mask::transfer_mask(&[], &[], &VertexMask::zeros(0), &[[0.0, 0.0, 0.0]], 0.0).unwrap();
    assert_eq!(empty.matched, vec![false]);
    assert_eq!(empty.mask.as_slice(), &[0.0]);
}

#[test]
fn invalid_geometry_parameters_are_explicit_errors() {
    let (positions, faces) = square();
    let mut mask = VertexMask::zeros(4);
    assert!(matches!(
        mask.grow(&positions[..3], &faces),
        Err(MaskError::Length { .. })
    ));
    assert!(matches!(
        mask.grow(&positions, &[[0, 1, 99]]),
        Err(MaskError::Triangle { .. })
    ));
    assert!(matches!(
        mask.smooth(&positions, &faces, 2.0),
        Err(MaskError::Strength(_))
    ));
    assert!(matches!(
        mask::transfer_mask(&positions, &faces, &mask, &positions, -1.0),
        Err(MaskError::Distance(_))
    ));
    let mut invalid = positions;
    invalid[0][0] = f32::NAN;
    assert!(matches!(
        mask.grow(&invalid, &faces),
        Err(MaskError::Position { vertex: 0 })
    ));
}

#[test]
fn invariance_metric_checks_fully_locked_vertices_only() {
    let mask = VertexMask::from_values(vec![1.0, 0.5]).unwrap();
    let before = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
    assert_eq!(
        mask::masked_vertex_max_delta(&before, &[[0.0, 0.0, 0.0], [3.0, 0.0, 0.0]], &mask).unwrap(),
        0.0
    );
    let value =
        mask::masked_vertex_max_delta(&before, &[[0.1, 0.0, 0.0], [3.0, 0.0, 0.0]], &mask).unwrap();
    assert!((value - 0.1).abs() < 1e-6);
}
