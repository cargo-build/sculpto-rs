//! Shared topology permits contact only on the common geometric feature.

use sculpto_mesh::check::{self, WitnessKind};

fn assert_pair(positions: &[[f32; 3]], faces: &[[u32; 3]], expected: usize) {
    let witnesses = check::self_intersection_witnesses(positions, faces);
    assert_eq!(witnesses.len(), expected);
    assert_eq!(check::self_intersection_pairs(positions, faces), expected);
    assert_eq!(
        check::internally_crossing_triangles(positions, faces),
        match expected {
            0 => vec![],
            _ => vec![0, 1],
        }
    );
    assert!(witnesses.iter().all(|w| w.kind == WitnessKind::Crossing));
}

#[test]
fn opposite_edge_crossing_is_not_hidden_by_one_shared_vertex() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.25, 0.25, -1.0],
        [0.25, 0.25, 1.0],
    ];
    [1e-3, 1.0, 1000.0].into_iter().for_each(|scale| {
        let transformed = positions.map(|p| {
            [
                scale * (p[2] + 3.0),
                scale * (p[0] - 2.0),
                scale * (p[1] + 1.0),
            ]
        });
        [[[0, 1, 2], [0, 3, 4]], [[2, 1, 0], [4, 3, 0]]]
            .into_iter()
            .for_each(|faces| assert_pair(&transformed, &faces, 1));
    });
}

#[test]
fn contact_only_at_a_shared_vertex_is_legal() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [-1.0, 0.0, -1.0],
        [-1.0, 0.0, 1.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
    ];
    assert_pair(&positions, &[[0, 1, 2], [0, 3, 4]], 0);
    assert_pair(&positions, &[[0, 1, 2], [0, 5, 6]], 0);
}

#[test]
fn coplanar_overlap_beyond_a_shared_vertex_or_edge_is_reported() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [0.25, 0.25, 0.0],
    ];
    assert_pair(&positions, &[[0, 1, 2], [0, 3, 4]], 1);
    assert_pair(&positions, &[[0, 1, 2], [0, 1, 5]], 1);
}

#[test]
fn shared_edge_allows_coplanar_and_folded_neighbors() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.25, 0.25, 1e-8],
    ];
    [3, 4, 5]
        .into_iter()
        .for_each(|third| assert_pair(&positions, &[[0, 1, 2], [1, 0, third]], 0));
}

#[test]
fn overlapping_unshared_edges_are_not_a_legal_point_contact() {
    let positions = [
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
    ];
    assert_pair(&positions, &[[0, 1, 2], [0, 3, 4]], 1);
}

#[test]
fn exact_position_seams_preserve_the_shared_feature_decision() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.25, 0.25, -1.0],
        [0.25, 0.25, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
    ];
    assert_pair(&positions, &[[0, 1, 2], [3, 4, 5]], 1);
    assert_pair(&positions, &[[0, 1, 2], [3, 6, 7]], 0);
    assert_eq!(
        check::intersection_pairs(&positions, &[[0, 1, 2]], &positions, &[[3, 6, 7]]),
        1,
        "contacts between distinct meshes remain contacts"
    );
}

#[test]
fn tiny_crossing_and_separated_triangles_against_a_large_face() {
    let large = [
        [-100.0, -100.0, 0.0],
        [100.0, -100.0, 0.0],
        [0.0, 100.0, 0.0],
    ];
    let crossing = [[0.0, 0.0, -1e-9], [1e-6, 0.0, 1e-9], [0.0, 1e-6, 1e-9]];
    let separated = crossing.map(|[x, y, _]| [x, y, 1e-9]);
    let face = [[0, 1, 2]];
    [(&crossing, 1), (&separated, 0)]
        .into_iter()
        .for_each(|(small, expected)| {
            assert_eq!(
                check::intersection_pairs(small, &face, &large, &face),
                expected
            );
            assert_eq!(
                check::intersection_pairs(&large, &face, small, &face),
                expected
            );
        });
}
