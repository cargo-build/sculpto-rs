//! Penetration depth: sign, magnitude, statistics and input refusals.

use sculpto_mesh::check::{self, DepthReport};
use sculpto_mesh::wrap::FitError;

/// Outward-wound unit cube spanning `[-1, 1]` on each axis.
const CUBE_POSITIONS: [[f32; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];
const CUBE_FACES: [[u32; 3]; 12] = [
    [4, 5, 6],
    [4, 6, 7],
    [1, 0, 3],
    [1, 3, 2],
    [1, 2, 6],
    [1, 6, 5],
    [0, 4, 7],
    [0, 7, 3],
    [3, 7, 6],
    [3, 6, 2],
    [0, 1, 5],
    [0, 5, 4],
];

/// Square in the `y = 0` plane, normal `+y`.
const PLANE_POSITIONS: [[f32; 3]; 4] = [
    [-10.0, 0.0, -10.0],
    [10.0, 0.0, -10.0],
    [10.0, 0.0, 10.0],
    [-10.0, 0.0, 10.0],
];
const PLANE_FACES: [[u32; 3]; 2] = [[0, 2, 1], [0, 3, 2]];

fn depth_report(
    garment_positions: &[[f32; 3]],
    garment_faces: &[[u32; 3]],
    body_positions: &[[f32; 3]],
    body_faces: &[[u32; 3]],
) -> DepthReport {
    check::garment_depth_report(
        (garment_positions, garment_faces),
        (body_positions, body_faces),
    )
    .expect("valid geometry")
}

fn close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() < 1e-5,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn depth_is_the_distance_behind_the_nearest_triangle() {
    let garment = [
        [0.0, 0.0, 0.0], // cube centre: depth 1.0
        [0.9, 0.0, 0.0], // 0.1 behind the +x face
        [1.0, 0.0, 0.0], // on the surface: depth 0, still contact
        [1.2, 0.0, 0.0], // outside: no contact
    ];
    let report = depth_report(&garment, &[], &CUBE_POSITIONS, &CUBE_FACES);
    assert_eq!(report.vertices, 4);
    assert_eq!(report.inside, 2);
    assert_eq!(report.contacts, 3);
    assert_eq!(report.body_pairs, 0);
    assert_eq!(report.self_pairs, 0);
    close(report.max, 1.0);
    close(report.mean, 0.55);
    close(report.median, 0.55);
    assert_eq!(report.worst.len(), 2);
    assert_eq!(report.worst[0].0, 0);
    close(report.worst[0].1, 1.0);
    assert_eq!(report.worst[1].0, 1);
    close(report.worst[1].1, 0.1);
}

#[test]
fn statistics_and_bands_follow_the_measured_depths() {
    let garment = [
        [2.0, -0.02, 2.0],
        [2.0, 0.0, 2.0],
        [2.0, 0.03, 2.0],
        [2.0, -0.06, 2.0],
        [2.0, -0.12, 2.0],
        [2.0, -0.2, 2.0],
        [2.0, -0.35, 2.0],
        [2.0, -0.6, 2.0],
    ];
    let report = depth_report(&garment, &[], &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.inside, 6);
    // Six inside plus the on-surface and the 0.03-outside vertices.
    assert_eq!(report.contacts, 8);
    close(report.mean, 0.225);
    close(report.median, 0.16);
    close(report.p90, 0.475);
    close(report.p99, 0.5875);
    close(report.max, 0.6);
    assert_eq!(report.above, [5, 4, 3, 2, 1]);
    assert_eq!(
        report
            .worst
            .iter()
            .map(|&(vertex, _)| vertex)
            .collect::<Vec<_>>(),
        vec![7, 6, 5, 4, 3, 0]
    );
}

#[test]
fn a_garment_clear_of_the_body_reports_zeros() {
    let garment = [[2.0, 5.0, 2.0], [3.0, 5.0, 2.0], [2.0, 5.0, 3.0]];
    let report = depth_report(&garment, &[[0, 1, 2]], &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.contacts, 0);
    assert_eq!(report.inside, 0);
    assert_eq!(report.mean, 0.0);
    assert_eq!(report.median, 0.0);
    assert_eq!(report.p90, 0.0);
    assert_eq!(report.p99, 0.0);
    assert_eq!(report.max, 0.0);
    assert_eq!(report.above, [0; 5]);
    assert!(report.worst.is_empty());
    assert_eq!(report.body_pairs, 0);
    assert_eq!(report.self_pairs, 0);
}

#[test]
fn triangle_contacts_stay_with_the_existing_witness_owners() {
    let crossing = [[2.0, -0.1, 2.0], [2.0, 0.1, 2.0], [2.5, 0.1, 1.5]];
    let body = depth_report(&crossing, &[[0, 1, 2]], &PLANE_POSITIONS, &PLANE_FACES);
    assert!(body.body_pairs > 0);
    assert_eq!(body.self_pairs, 0);

    // The shared-vertex fold from `shared_contacts.rs`, lifted clear of the body.
    let folded = [
        [0.0, 5.0, 0.0],
        [1.0, 5.0, 0.0],
        [0.0, 6.0, 0.0],
        [0.25, 5.25, -1.0],
        [0.25, 5.25, 1.0],
    ];
    let self_default = depth_report(
        &folded,
        &[[0, 1, 2], [0, 3, 4]],
        &PLANE_POSITIONS,
        &PLANE_FACES,
    );
    assert_eq!(self_default.body_pairs, 0);
    assert_eq!(self_default.self_pairs, 1);
    assert_eq!(self_default.contacts, 0);
}

#[test]
fn malformed_input_is_refused_by_name() {
    let valid_garment = [[0.0, 0.0, 0.0]];
    let valid_body = CUBE_POSITIONS;
    assert_eq!(
        check::garment_depth_report((&[], &[]), (&valid_body, &CUBE_FACES)),
        Err(FitError::EmptyGarment)
    );
    assert_eq!(
        check::garment_depth_report((&valid_garment, &[]), (&[], &[])),
        Err(FitError::EmptyBody)
    );
    assert_eq!(
        check::garment_depth_report((&valid_garment, &[]), (&valid_body, &[])),
        Err(FitError::EmptyBodyFaces)
    );
    let not_finite_garment = [[f32::NAN, 0.0, 0.0]];
    assert_eq!(
        check::garment_depth_report((&not_finite_garment, &[]), (&valid_body, &CUBE_FACES)),
        Err(FitError::NonFiniteGarmentPosition { vertex: 0 })
    );
    let not_finite_body = [[0.0, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0]];
    assert_eq!(
        check::garment_depth_report((&valid_garment, &[]), (&not_finite_body, &CUBE_FACES)),
        Err(FitError::NonFiniteBodyPosition { vertex: 1 })
    );
    assert_eq!(
        check::garment_depth_report((&valid_garment, &[[0, 1, 9]]), (&valid_body, &CUBE_FACES)),
        Err(FitError::GarmentFaceIndex {
            triangle: 0,
            vertex: 1
        })
    );
    assert_eq!(
        check::garment_depth_report((&valid_garment, &[]), (&valid_body, &[[0, 1, 9]])),
        Err(FitError::BodyFaceIndex {
            triangle: 0,
            vertex: 9
        })
    );
}
