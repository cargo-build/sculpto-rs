use sculpto_mesh::uv;

const BASE: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

#[test]
fn duplicate_containment_and_proper_overlap_have_positive_area() {
    [
        BASE,
        [[0.1, 0.1], [0.2, 0.1], [0.1, 0.2]],
        [[0.5, 0.0], [1.0, 0.5], [0.5, 1.0]],
    ]
    .into_iter()
    .for_each(|other| {
        let report = uv::qualify(&[BASE, other]).unwrap();
        assert_eq!(report.overlap_pairs, [[0, 1]], "{other:?}");
        assert!(!report.is_valid());
    });
}

#[test]
fn shared_edge_point_and_collinear_t_contact_have_no_area() {
    let cases = [
        (BASE, [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]),
        (
            [[0.0, 0.0], [0.5, 0.0], [0.0, 0.5]],
            [[0.5, 0.0], [1.0, 0.0], [1.0, 0.5]],
        ),
        (
            [[0.0, 0.5], [1.0, 0.5], [0.0, 1.0]],
            [[0.25, 0.5], [0.5, 0.5], [0.5, 0.0]],
        ),
    ];
    cases.into_iter().for_each(|(left, right)| {
        let report = uv::qualify(&[left, right]).unwrap();
        assert!(report.is_valid(), "{left:?} / {right:?}: {report:?}");
    });
}

#[test]
fn winding_and_corner_permutation_do_not_change_overlap() {
    let inside = [[0.1, 0.1], [0.2, 0.1], [0.1, 0.2]];
    [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ]
    .into_iter()
    .for_each(|order| {
        let triangle = order.map(|index| BASE[index]);
        assert_eq!(
            uv::qualify(&[inside, triangle]).unwrap().overlap_pairs,
            [[0, 1]]
        );
    });
}

#[test]
fn thin_positive_triangles_are_not_collapsed_by_an_epsilon() {
    let thin = [[0.0, 0.0], [1.0, 1e-30], [1.0, 0.0]];
    let report = uv::qualify(&[thin, thin]).unwrap();
    assert!(report.degenerate_triangles.is_empty());
    assert_eq!(report.overlap_pairs, [[0, 1]]);
}

#[test]
fn degenerate_and_bounds_diagnostics_retain_face_and_corner_ids() {
    let report = uv::qualify(&[
        BASE,
        [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]],
        [[-1.0, 0.0], [-1.0, 0.0], [-1.0, 0.0]],
    ])
    .unwrap();
    assert_eq!(report.degenerate_triangles, [1, 2]);
    assert_eq!(report.out_of_bounds_corners, [[2, 0], [2, 1], [2, 2]]);
    assert!(report.overlap_pairs.is_empty());
    assert!(!report.is_valid());
}

#[test]
fn nonfinite_error_is_indexed() {
    [f32::NAN, f32::INFINITY, f32::NEG_INFINITY]
        .into_iter()
        .for_each(|value| {
            let mut triangle = BASE;
            triangle[2][1] = value;
            let error = uv::qualify(&[BASE, triangle]).unwrap_err();
            assert_eq!((error.triangle, error.corner, error.axis), (1, 2, 1));
        });
}

#[test]
fn pairs_are_lexicographic_even_when_sweep_order_differs() {
    let triangles = [
        BASE.map(|[x, y]| [x + 0.25, y]),
        BASE,
        BASE.map(|[x, y]| [x + 0.1, y]),
    ];
    assert_eq!(
        uv::qualify(&triangles).unwrap().overlap_pairs,
        [[0, 1], [0, 2], [1, 2]]
    );
}
