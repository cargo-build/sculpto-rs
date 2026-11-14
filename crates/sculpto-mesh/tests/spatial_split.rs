//! Spatial X-gap splitting for generated multi-figure exports.

use sculpto_mesh::spatial_split::{SplitError, split_x};

#[test]
fn splits_three_separated_triangles_in_x_order() {
    let positions = [
        [0.0, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [0.2, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.1, 0.0, 0.0],
        [10.2, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.1, 0.0, 0.0],
        [20.2, 0.0, 0.0],
    ];
    let triangles = [[0, 1, 2], [3, 4, 5], [6, 7, 8]];
    let parts = split_x(&positions, &[], &[], &triangles).expect("split");
    assert_eq!(parts.len(), 3);
    for (index, part) in parts.iter().enumerate() {
        assert_eq!(part.positions.len(), 3);
        assert_eq!(part.triangles, [[0, 1, 2]]);
        assert!((part.min[0] - index as f32 * 10.0).abs() < 1e-5);
        assert!((part.max[0] - (index as f32 * 10.0 + 0.2)).abs() < 1e-5);
    }
}

#[test]
fn carries_normals_and_uvs_with_reindexed_vertices() {
    let positions = [
        [0.0, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [0.2, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.1, 0.0, 0.0],
        [10.2, 0.0, 0.0],
    ];
    let normals = [[0.0, 0.0, 1.0]; 6];
    let uvs = [
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
        [0.0, 0.0],
        [1.0, 0.0],
        [0.0, 1.0],
    ];
    let parts = split_x(&positions, &normals, &uvs, &[[0, 1, 2], [3, 4, 5]]).expect("split");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].normals, normals[..3]);
    assert_eq!(parts[1].uvs, uvs[3..]);
}

#[test]
fn refuses_a_triangle_that_crosses_the_gap() {
    let positions = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    assert_eq!(
        split_x(&positions, &[], &[], &[[0, 1, 2]]),
        Err(SplitError::CrossingTriangle { triangle: 0 })
    );
}

#[test]
fn refuses_a_mesh_without_a_gap() {
    let positions = [[0.0, 0.0, 0.0], [0.0, 0.1, 0.0], [0.0, 0.0, 0.1]];
    assert_eq!(
        split_x(&positions, &[], &[], &[[0, 1, 2]]),
        Err(SplitError::NoGap)
    );
}
