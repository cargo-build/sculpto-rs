//! The shared surface-distance query the weight copy radius and the actor
//! export both read. @req DECL0013

use sculpto_mesh::check::surface_distances;

/// One triangle in the z=0 plane: every distance below is analytic.
fn triangle() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
        vec![[0, 1, 2]],
    )
}

#[test]
fn distances_are_measured_to_the_surface_not_to_a_vertex() {
    let (positions, faces) = triangle();
    let queries = [
        [0.0, 0.0, 0.0],  // on a corner
        [0.5, 0.5, 0.0],  // inside the triangle: zero
        [0.5, 0.5, 2.0],  // straight above the interior
        [-2.0, 0.5, 0.0], // outside the fitted edge
    ];
    let distances = surface_distances(&queries, &positions, &faces);
    let measured: Vec<f32> = distances
        .iter()
        .map(|hit| hit.expect("every query has a comparable triangle"))
        .collect();
    assert!(measured[0] < 1e-6, "a corner is on the surface");
    // The triangle's interior is part of the surface, so its centre is at zero
    // even though no corner sits there.
    assert!(measured[1] < 1e-6, "the interior is at distance zero");
    assert!(
        (measured[2] - 2.0).abs() < 1e-5,
        "a point above the interior is 2 units away: {}",
        measured[2]
    );
    assert!(
        (measured[3] - 2.0).abs() < 1e-5,
        "a point beside the fitted edge is 2 units away: {}",
        measured[3]
    );
}

#[test]
fn an_empty_surface_has_no_distance_rather_than_a_zero() {
    let distances = surface_distances(&[[0.0, 0.0, 1.0]], &[], &[]);
    assert_eq!(distances, vec![None], "no triangle, no distance");
}
