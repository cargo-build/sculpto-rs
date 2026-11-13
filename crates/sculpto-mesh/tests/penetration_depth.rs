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

/// The same `y = 0` square shrunk to one unit, so a nearest body point can fall
/// on its edge and leave the penetrating vertex off the outward normal.
const SMALL_PLANE_POSITIONS: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0],
];
const SMALL_PLANE_FACES: [[u32; 3]; 2] = [[0, 2, 1], [0, 3, 2]];

/// A garment triangle `0.1` behind the `y = 0` plane body, wound outwards.
const PENETRATING_PLANE: [[f32; 3]; 3] = [[2.0, -0.1, 2.0], [2.1, -0.1, 2.0], [2.0, -0.1, 2.1]];
const PENETRATING_FACE: [[u32; 3]; 1] = [[0, 2, 1]];

/// The same triangle plus a shell `0.5` above the body over all three contact
/// points: the garment's own surface stands between them and the outside.
const SHELLED_PLANE: [[f32; 3]; 6] = [
    [2.0, -0.1, 2.0],
    [2.1, -0.1, 2.0],
    [2.0, -0.1, 2.1],
    [0.0, 0.5, 0.0],
    [5.0, 0.5, 0.0],
    [0.0, 0.5, 5.0],
];
const SHELLED_FACES: [[u32; 3]; 2] = [[0, 2, 1], [3, 4, 5]];

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
fn layer_follows_the_garment_normal_and_reversing_it_hides_the_depth() {
    // Three vertices 0.1 behind the cube's `+x` face, whose normal is `+x`:
    // the outward winding faces the same way and the reversed one does not.
    let vertices = [[0.9, 0.0, 0.0], [0.9, 0.1, 0.0], [0.9, 0.0, 0.1]];
    let visible = depth_report(&vertices, &[[0, 1, 2]], &CUBE_POSITIONS, &CUBE_FACES);
    assert_eq!(visible.inside, 3);
    assert_eq!(visible.visible.count, 3);
    assert_eq!(visible.inner.count, 0);
    close(visible.visible.max, 0.1);
    close(visible.visible.mean, 0.1);
    close(visible.visible.p90, 0.1);
    let failing = visible.visible_verdict(0.05);
    assert!(!failing.passed);
    assert_eq!(failing.violations.len(), 3);
    close(failing.violations[0].1, 0.1);

    let reversed = depth_report(&vertices, &[[0, 2, 1]], &CUBE_POSITIONS, &CUBE_FACES);
    assert_eq!(reversed.visible.count, 0);
    assert_eq!(reversed.inner.count, 3);
    close(reversed.inner.max, 0.1);
    let hidden = reversed.visible_verdict(0.05);
    assert!(hidden.passed);
    assert!(hidden.violations.is_empty());
    // The layer split classifies the same penetration; it does not remove it.
    assert_eq!(reversed.inside, 3);
    assert_eq!(reversed.max, visible.max);
}

#[test]
fn the_tolerance_boundary_passes_at_the_bound_and_fails_above_it() {
    // A garment triangle 0.1 above the plane body, wound so its own normal is
    // the body's `+y`: a visible layer at the depth it is measured on.
    let vertices = [[2.0, -0.1, 2.0], [2.1, -0.1, 2.0], [2.0, -0.1, 2.1]];
    let report = depth_report(&vertices, &[[0, 2, 1]], &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.visible.count, 3);
    assert_eq!(report.inner.count, 0);
    close(report.visible.max, 0.1);

    let depth = report.visible.max;
    let at_bound = report.visible_verdict(depth);
    assert!(at_bound.passed, "a depth at the tolerance passes");
    assert!(at_bound.violations.is_empty());
    let above = report.visible_verdict(depth - 1e-3);
    assert!(!above.passed, "a depth epsilon above the tolerance fails");
    assert_eq!(above.violations.len(), 3);
}

#[test]
fn a_clear_garment_passes_any_tolerance_with_zero_depth() {
    let garment = [[2.0, 5.0, 2.0], [3.0, 5.0, 2.0], [2.0, 5.0, 3.0]];
    let report = depth_report(&garment, &[[0, 1, 2]], &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.visible.count, 0);
    assert_eq!(report.inner.count, 0);
    assert_eq!(report.visible.max, 0.0);
    let verdict = report.visible_verdict(0.0);
    assert!(verdict.passed);
    assert!(verdict.violations.is_empty());
}

#[test]
fn a_vertex_without_a_face_counts_as_visible() {
    // No garment faces means no vertex normals; an unreadable facing must not
    // hide the depth from the verdict.
    let garment = [[0.9, 0.0, 0.0], [0.9, 0.1, 0.0], [0.9, 0.0, 0.1]];
    let report = depth_report(&garment, &[], &CUBE_POSITIONS, &CUBE_FACES);
    assert_eq!(report.inside, 3);
    assert_eq!(report.visible.count, 3);
    assert_eq!(report.inner.count, 0);
    assert!(!report.visible_verdict(0.05).passed);
}

#[test]
fn a_contact_shell_hides_the_penetrating_plane() {
    // Without the shell the same plane is exposed: nothing is between the body
    // contact point and the outside, so it stays graded.
    let exposed = depth_report(
        &PENETRATING_PLANE,
        &PENETRATING_FACE,
        &PLANE_POSITIONS,
        &PLANE_FACES,
    );
    assert_eq!(exposed.inside, 3);
    assert_eq!(exposed.visible.count, 3);
    assert_eq!(exposed.inner.count, 0);
    assert_eq!(exposed.hidden.count, 0);
    assert_eq!(exposed.visible_occluded.count, 3);
    close(exposed.visible_occluded.max, 0.1);
    let failing = exposed.visible_verdict(0.05);
    assert!(!failing.passed);
    assert_eq!(failing.violations.len(), 3);

    let covered = depth_report(
        &SHELLED_PLANE,
        &SHELLED_FACES,
        &PLANE_POSITIONS,
        &PLANE_FACES,
    );
    assert_eq!(covered.inside, 3);
    // The facing split is unchanged: the diagnosis still says outer layer.
    assert_eq!(covered.visible.count, 3);
    assert_eq!(covered.inner.count, 0);
    close(covered.visible.max, 0.1);
    assert_eq!(covered.hidden.count, 3);
    close(covered.hidden.max, 0.1);
    assert_eq!(covered.visible_occluded.count, 0);
    assert_eq!(covered.visible_occluded.max, 0.0);
    // The depth measurement is unchanged; only the graded set moves.
    close(covered.max, 0.1);
    let passing = covered.visible_verdict(0.0);
    assert!(passing.passed);
    assert!(passing.violations.is_empty());
}

#[test]
fn an_occluded_inner_layer_stays_out_of_the_graded_set() {
    // The same shell over a reversed (inward-facing) plane: the facing test and
    // the occlusion test both withhold it, and neither substitutes for the other.
    let reversed_faces = [[0, 1, 2], [3, 4, 5]];
    let report = depth_report(
        &SHELLED_PLANE,
        &reversed_faces,
        &PLANE_POSITIONS,
        &PLANE_FACES,
    );
    assert_eq!(report.inside, 3);
    assert_eq!(report.visible.count, 0);
    assert_eq!(report.inner.count, 3);
    assert_eq!(report.hidden.count, 3);
    assert_eq!(report.visible_occluded.count, 0);
    assert!(report.visible_verdict(0.0).passed);
}

#[test]
fn the_contact_points_own_surface_never_occludes() {
    // The triangle passes through the body contact point itself: that hit is the
    // contact, inside `OCCLUSION_EPSILON`, not a surface above it.
    let garment = [[2.0, -0.1, 2.0], [2.0, 0.0, 2.0], [2.5, 0.2, 2.5]];
    let report = depth_report(&garment, &[[0, 1, 2]], &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.inside, 1);
    assert_eq!(report.contacts, 2);
    assert_eq!(report.visible.count, 1);
    assert_eq!(report.hidden.count, 0);
    assert_eq!(report.visible_occluded.count, 1);
    close(report.visible_occluded.max, 0.1);
    assert_eq!(report.visible_verdict(0.05).violations.len(), 1);
}

#[test]
fn a_shared_edge_tie_between_foreign_surfaces_is_hidden_and_repeats() {
    // Two foreign triangles share the edge the ray crosses, so both hits land in
    // the tie band; neither is the vertex's own, so the vertex stays hidden.
    let garment = [
        [5.0, -0.1, 5.0], // 0: inside by 0.1
        [5.0, -0.1, 5.2], // 1
        [5.0, -0.2, 5.1], // 2: inside by 0.2
        [4.5, 0.3, 5.0],  // 3: shared edge of the covering pair
        [5.5, 0.3, 5.0],  // 4: crosses the vertex-0 ray at its midpoint
        [5.0, 0.9, 6.0],  // 5
        [5.0, 0.9, 4.0],  // 6
    ];
    let faces = [[0, 2, 1], [3, 4, 5], [3, 4, 6]];
    let report = depth_report(&garment, &faces, &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(report.inside, 3);
    assert_eq!(report.visible.count, 3);
    assert_eq!(report.hidden.count, 3);
    assert_eq!(report.visible_occluded.count, 0);
    assert!(
        report
            .hidden
            .depths
            .iter()
            .any(|&(vertex, depth)| vertex == 0 && (depth - 0.1).abs() < 1e-6)
    );
    assert!(report.visible_verdict(0.0).passed);
    assert_eq!(
        report,
        depth_report(&garment, &faces, &PLANE_POSITIONS, &PLANE_FACES)
    );
}

#[test]
fn a_surface_within_the_occlusion_epsilon_is_the_contact_itself() {
    // The same cover 5e-5 above the body is the contact, not a layer; at 2e-4 it
    // is a surface above it. `OCCLUSION_EPSILON` is the declared boundary.
    let garment = |cover_height: f32| {
        [
            [2.0, -0.1, 2.0],
            [2.1, -0.1, 2.0],
            [2.0, -0.1, 2.1],
            [0.0, cover_height, 0.0],
            [5.0, cover_height, 0.0],
            [0.0, cover_height, 5.0],
        ]
    };
    let faces = [[0, 2, 1], [3, 4, 5]];
    let at_contact = depth_report(&garment(5e-5), &faces, &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(at_contact.hidden.count, 0);
    assert_eq!(at_contact.visible_occluded.count, 3);

    let above = depth_report(&garment(2e-4), &faces, &PLANE_POSITIONS, &PLANE_FACES);
    assert_eq!(above.hidden.count, 3);
    assert_eq!(above.visible_occluded.count, 0);
}

#[test]
fn a_foreign_surface_hides_only_when_it_is_nearer_than_the_own_surface() {
    // A one-unit body puts the contact point on its edge, so the penetrating
    // vertex is off the ray and its own triangle can be met above the contact.
    // The ray meets the own triangle at 0.2: a foreign surface at 0.15 is in
    // front of it and hides the vertex, the same surface at 0.25 is behind it.
    let garment = |cover_height: f32| {
        [
            [2.0, -0.1, 0.5],         // 0: inside by 0.1, lateral offset from the contact
            [1.0, 0.2, 0.5],          // 1: own triangle, crossed by the ray at 0.2
            [1.0, 0.2, 1.5],          // 2
            [0.5, cover_height, 0.0], // 3: foreign cover
            [1.5, cover_height, 0.0], // 4
            [1.0, cover_height, 1.0], // 5
        ]
    };
    let faces = [[0, 1, 2], [3, 4, 5]];
    let nearer = depth_report(
        &garment(0.15),
        &faces,
        &SMALL_PLANE_POSITIONS,
        &SMALL_PLANE_FACES,
    );
    assert_eq!(nearer.inside, 1);
    assert_eq!(nearer.visible.count, 1);
    assert_eq!(nearer.hidden.count, 1);
    assert_eq!(nearer.visible_occluded.count, 0);
    close(nearer.hidden.max, 0.1);

    let behind = depth_report(
        &garment(0.25),
        &faces,
        &SMALL_PLANE_POSITIONS,
        &SMALL_PLANE_FACES,
    );
    assert_eq!(behind.inside, 1);
    assert_eq!(behind.hidden.count, 0);
    assert_eq!(behind.visible_occluded.count, 1);
    close(behind.visible_occluded.max, 0.1);
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
