//! Focused synthetic tests of `dress`: subdivision counts, the contact band on
//! a plane, a cup over a bump, the body-poke repair, and every refusal.
//!
//! Controls are hand-computable: a flat plane has an exact normal and standoff,
//! so a contact solve either lands in the band or the test says by how much.

use sculpto_mesh::dress::{
    DressError, DressParams, MAX_CUTS, MAX_ITERATIONS, MAX_POKE_PASSES, MAX_ROUNDS, dress,
    midpoint_subdivide,
};

/// A square patch of `cells` × `cells` quads on `z`, wound so +Z faces out.
fn patch(cells: usize, z: f32, half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let step = 2.0 * half / cells as f32;
    let mut positions = Vec::new();
    for row in 0..=cells {
        for column in 0..=cells {
            positions.push([-half + column as f32 * step, -half + row as f32 * step, z]);
        }
    }
    let stride = cells + 1;
    let mut triangles = Vec::new();
    for row in 0..cells {
        for column in 0..cells {
            let corner = (row * stride + column) as u32;
            let next = corner + stride as u32;
            triangles.push([corner, corner + 1, next + 1]);
            triangles.push([corner, next + 1, next]);
        }
    }
    (positions, triangles)
}

fn params() -> DressParams {
    DressParams {
        cuts: 0,
        rounds: 0,
        relax: 0.0,
        poke_passes: 0,
        ..DressParams::default()
    }
}

/// Signed clearance of every vertex against the plane `z = 0`.
fn plane_standoffs(positions: &[[f32; 3]]) -> Vec<f32> {
    positions.iter().map(|position| position[2]).collect()
}

#[test]
fn subdivision_counts_and_shares_edges() {
    let triangle = (
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[0u32, 1, 2]],
    );
    let (vertices, faces) = midpoint_subdivide(&triangle.0, &triangle.1, 0);
    assert_eq!(
        (vertices.len(), faces.len()),
        (3, 1),
        "no cut changes nothing"
    );
    let (vertices, faces) = midpoint_subdivide(&triangle.0, &triangle.1, 1);
    assert_eq!(
        (vertices.len(), faces.len()),
        (6, 4),
        "one cut: three midpoints"
    );
    let (vertices, faces) = midpoint_subdivide(&triangle.0, &triangle.1, 2);
    assert_eq!(
        (vertices.len(), faces.len()),
        (15, 16),
        "two cuts: nine midpoints"
    );

    // A shared edge is one vertex, not two: the quad keeps a closed surface.
    let quad = patch(1, 0.0, 1.0);
    let (vertices, faces) = midpoint_subdivide(&quad.0, &quad.1, 1);
    assert_eq!((quad.0.len(), quad.1.len()), (4, 2));
    assert_eq!(
        (vertices.len(), faces.len()),
        (9, 8),
        "five shared edge midpoints"
    );
    // The input vertices keep their indices, so a garment identity map stays valid.
    for (index, position) in quad.0.iter().enumerate() {
        assert_eq!(vertices[index], *position);
    }
}

#[test]
fn a_floating_garment_comes_in_to_the_declared_clearance() {
    let body = patch(4, 0.0, 2.0);
    let garment = patch(2, 2.0, 0.5);
    let options = DressParams {
        clearance: 0.25,
        contact: 0.05,
        ..params()
    };
    let (dressed, faces, report) = dress((&garment.0, &garment.1), (&body.0, &body.1), &options)
        .expect("a flat body and a floating patch dress");
    assert_eq!(faces.len(), garment.1.len(), "no subdivision was asked for");
    for standoff in plane_standoffs(&dressed) {
        assert!(
            standoff <= 0.25,
            "standoff {standoff} is above the declared clearance"
        );
    }
    assert_eq!(
        report.max_floating_gap, 0.0,
        "nothing floats after the pull"
    );
    assert!(
        report.max_floating_gap_before > 1.5,
        "the patch started 2 units out"
    );
    assert_eq!(report.floating_before, garment.0.len());
    assert!(report.converged, "a flat pull converges");
    assert!(report.moved > 0);
}

#[test]
fn a_penetrating_garment_rises_out_of_the_body() {
    let body = patch(4, 0.0, 2.0);
    let garment = patch(2, -0.4, 0.5);
    let options = DressParams {
        clearance: 0.25,
        contact: 0.05,
        ..params()
    };
    let (dressed, _, report) = dress((&garment.0, &garment.1), (&body.0, &body.1), &options)
        .expect("a penetrating patch dresses");
    for standoff in plane_standoffs(&dressed) {
        assert!(
            standoff >= 0.05 - 1e-4,
            "standoff {standoff} stays below the contact standoff"
        );
    }
    assert_eq!(report.max_penetration, 0.0);
    assert_eq!(report.penetrating_before, garment.0.len());
    assert!(report.max_penetration_before > 0.39);
}

#[test]
fn a_cup_keeps_its_height_over_a_bump() {
    // A tent under a flat patch: the projection must lift the patch over the
    // bump instead of pressing it flat onto the surrounding plane, which is
    // what a cross-section envelope would do.
    let (mut body_positions, body_triangles) = patch(4, 0.0, 1.0);
    let stride = 5;
    body_positions[2 * stride + 2][2] = 0.6;
    let garment = patch(2, 0.2, 1.0);
    let options = DressParams {
        clearance: 2.0,
        contact: 0.05,
        ..params()
    };
    let (dressed, _, report) = dress(
        (&garment.0, &garment.1),
        (&body_positions, &body_triangles),
        &options,
    )
    .expect("a bumped body dresses");
    let peak = dressed
        .iter()
        .map(|position| position[2])
        .fold(f32::MIN, f32::max);
    assert!(
        peak > 0.55,
        "the cup collapsed onto the base plane: peak {peak}"
    );
    assert_eq!(report.max_penetration, 0.0);
    let lowest = dressed
        .iter()
        .map(|position| position[2])
        .fold(f32::MAX, f32::min);
    assert!(
        lowest >= 0.05 - 1e-4,
        "no vertex sank below the contact standoff"
    );
}

#[test]
fn a_body_spike_through_a_garment_facet_is_pushed_out() {
    // A narrow spike between the garment's corners: every garment vertex is
    // outside the body, so only the facet push can repair the crossing.
    let (mut body_positions, mut body_triangles) = patch(4, 0.0, 1.0);
    let stride = 5;
    let spike = (2 * stride + 2) as usize; // grid vertex (0.0, 0.0)
    body_positions[spike][2] = 1.0;
    let garment = patch(1, 0.5, 1.0);

    let quiet = DressParams {
        clearance: 2.0,
        contact: 0.05,
        poke_passes: 0,
        ..params()
    };
    let (_, _, without) = dress(
        (&garment.0, &garment.1),
        (&body_positions, &body_triangles),
        &quiet,
    )
    .expect("the spike fixture dresses");
    assert!(without.pokes_before > 0, "the fixture must start poking");
    assert!(
        without.pokes_after > 0,
        "with the repair off the poke stays: the fixture isolates the stage"
    );

    let repair = DressParams {
        poke_passes: 2,
        ..quiet
    };
    let (dressed, _, report) = dress(
        (&garment.0, &garment.1),
        (&body_positions, &body_triangles),
        &repair,
    )
    .expect("the spike fixture dresses with the repair");
    assert!(
        report.pokes_after < report.pokes_before,
        "the repair has to remove pokes: {} -> {}",
        report.pokes_before,
        report.pokes_after
    );
    assert!(
        report.pokes_after < without.pokes_after,
        "the repair has to beat the contact pass alone: {} vs {}",
        report.pokes_after,
        without.pokes_after
    );
    let lowest = dressed
        .iter()
        .map(|position| position[2])
        .fold(f32::MAX, f32::min);
    assert!(
        lowest > 0.0,
        "no part of the facet sank into the body: {lowest}"
    );
    body_triangles.truncate(body_triangles.len());
}

/// A smooth mound: a grid patch raised by a Gaussian, so every normal tilts
/// toward the cloth above it.
fn mound(cells: usize, half: f32, height: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, triangles) = patch(cells, 0.0, half);
    for position in &mut positions {
        let (x, y) = (position[0], position[1]);
        position[2] = height * (-(x * x + y * y) / 0.8).exp();
    }
    (positions, triangles)
}

#[test]
fn a_body_mound_through_a_garment_facet_is_removed_by_the_repair() {
    // The shape the supplied bra hits: a garment facet far coarser than the
    // body curvature under it, so the body crosses the facet while every
    // garment vertex stays in the declared band. Each repair round ends with a
    // contact pass that reopens part of the crossing, so the repair converges
    // over rounds rather than in one, and the requirement is a zero count.
    let body = mound(8, 2.0, 0.6);
    let garment = patch(1, 0.1, 0.8);
    let (dressed, _, report) = dress(
        (&garment.0, &garment.1),
        (&body.0, &body.1),
        &DressParams {
            cuts: 0,
            rounds: 0,
            relax: 0.0,
            ..DressParams::default()
        },
    )
    .expect("the mound fixture dresses with the default repair budget");
    assert!(report.pokes_before > 0, "the fixture must start poking");
    assert_eq!(
        report.pokes_after, 0,
        "the default budget has to leave no body poke: {} -> {}",
        report.pokes_before, report.pokes_after
    );
    assert_eq!(
        report.max_floating_gap, 0.0,
        "the lift stays inside the band"
    );
    assert_eq!(report.max_penetration, 0.0);
    let peak = dressed
        .iter()
        .map(|position| position[2])
        .fold(f32::MIN, f32::max);
    assert!(
        peak > 0.6,
        "the facet has to clear the mound instead of folding over it: peak {peak}"
    );
    let lowest = dressed
        .iter()
        .map(|position| position[2])
        .fold(f32::MAX, f32::min);
    assert!(
        lowest >= DressParams::default().contact - 1e-4,
        "no vertex sank below the contact standoff: {lowest}"
    );
}

#[test]
fn the_band_parameters_are_refused_by_name() {
    let body = patch(2, 0.0, 1.0);
    let garment = patch(1, 0.5, 0.5);
    let refuse = |options: DressParams| {
        dress((&garment.0, &garment.1), (&body.0, &body.1), &options)
            .expect_err("an out-of-range parameter is refused")
    };
    assert!(matches!(
        refuse(DressParams {
            clearance: 0.0,
            ..params()
        }),
        DressError::Clearance(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            clearance: f32::NAN,
            ..params()
        }),
        DressError::Clearance(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            contact: 0.3,
            clearance: 0.2,
            ..params()
        }),
        DressError::Contact { .. }
    ));
    assert!(matches!(
        refuse(DressParams {
            contact: -0.1,
            ..params()
        }),
        DressError::Contact { .. }
    ));
    assert!(matches!(
        refuse(DressParams {
            cuts: MAX_CUTS + 1,
            ..params()
        }),
        DressError::Cuts(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            iterations: 0,
            ..params()
        }),
        DressError::Iterations(0)
    ));
    assert!(matches!(
        refuse(DressParams {
            iterations: MAX_ITERATIONS + 1,
            ..params()
        }),
        DressError::Iterations(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            max_step: 0.0,
            ..params()
        }),
        DressError::MaxStep(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            relax: 1.5,
            ..params()
        }),
        DressError::Relax(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            rounds: MAX_ROUNDS + 1,
            ..params()
        }),
        DressError::Rounds(_)
    ));
    assert!(matches!(
        refuse(DressParams {
            poke_passes: MAX_POKE_PASSES + 1,
            ..params()
        }),
        DressError::PokePasses(_)
    ));
}

#[test]
fn malformed_geometry_is_refused_by_name() {
    let body = patch(2, 0.0, 1.0);
    let garment = patch(1, 0.5, 0.5);
    let options = params();
    let empty: (Vec<[f32; 3]>, Vec<[u32; 3]>) = (Vec::new(), Vec::new());
    assert!(matches!(
        dress((&empty.0, &empty.1), (&body.0, &body.1), &options),
        Err(DressError::EmptyGarment)
    ));
    assert!(matches!(
        dress((&garment.0, &garment.1), (&empty.0, &empty.1), &options),
        Err(DressError::EmptyBody)
    ));
    assert!(matches!(
        dress((&garment.0, &garment.1), (&body.0, &[]), &options),
        Err(DressError::EmptyBodyFaces)
    ));
    let mut broken = garment.0.clone();
    broken[1][0] = f32::NAN;
    assert!(matches!(
        dress((&broken, &garment.1), (&body.0, &body.1), &options),
        Err(DressError::NonFiniteGarmentPosition { vertex: 1 })
    ));
    let mut broken_body = body.0.clone();
    broken_body[2][2] = f32::INFINITY;
    assert!(matches!(
        dress((&garment.0, &garment.1), (&broken_body, &body.1), &options),
        Err(DressError::NonFiniteBodyPosition { vertex: 2 })
    ));
    assert!(matches!(
        dress((&garment.0, &[[0u32, 1, 9]]), (&body.0, &body.1), &options),
        Err(DressError::GarmentFaceIndex { vertex: 9, .. })
    ));
    assert!(matches!(
        dress(
            (&garment.0, &garment.1),
            (&body.0, &[[0u32, 1, 77]]),
            &options
        ),
        Err(DressError::BodyFaceIndex { vertex: 77, .. })
    ));
}
