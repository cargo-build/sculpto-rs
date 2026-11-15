//! Focused synthetic tests of `dress`: subdivision counts, the contact band on
//! a plane, a cup over a bump, the body-poke repair, and every refusal.
//!
//! Controls are hand-computable: a flat plane has an exact normal and standoff,
//! so a contact solve either lands in the band or the test says by how much.

use sculpto_mesh::accumulated_surface::AccumulatedSurface;
use sculpto_mesh::dress::{
    DressError, DressParams, MAX_CUTS, MAX_ITERATIONS, MAX_POKE_PASSES, MAX_ROUNDS,
    SEAM_WELD_EPSILON, dress, dress_against, midpoint_subdivide, seam_weld,
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

/// Edges one triangle uses: the mesh's open border.
fn boundary_edges(triangles: &[[u32; 3]]) -> usize {
    let mut uses: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for face in triangles {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            *uses.entry((a.min(b), a.max(b))).or_insert(0) += 1;
        }
    }
    uses.values().filter(|&&count| count == 1).count()
}

/// Two triangles authored as separate pieces that share the diagonal of a unit
/// square: the seam copies are duplicated vertices, which is how the supplied
/// underwear is authored.
fn split_quad() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
    ];
    (positions, vec![[0, 1, 2], [3, 4, 5]])
}

#[test]
fn a_twin_seam_between_two_pieces_is_merged() {
    let (positions, triangles) = split_quad();
    let (welded, faces, report) = seam_weld(&positions, &triangles, SEAM_WELD_EPSILON);
    assert_eq!(
        (welded.len(), faces.len()),
        (4, 2),
        "the two duplicated seam vertices become one each"
    );
    assert_eq!(report.vertex_map, vec![0, 1, 2, 3, 0, 2], "raw to merged");
    assert_eq!(report.groups, vec![vec![0, 4], vec![2, 5]]);
    assert_eq!((report.components_before, report.components_after), (2, 1));
    assert_eq!(report.merged_vertices, 2);
    assert_eq!(report.merged_pairs, 2);
    assert_eq!(
        (report.boundary_edges_before, report.boundary_edges_after),
        (6, 4),
        "the shared diagonal stops being two open edges"
    );
    assert_eq!(report.dropped_faces, 0, "no face collapsed");

    // The merged seam is one edge, so one cut gives it one shared midpoint
    // instead of two copies that drift apart in the solve.
    let (subdivided, cut) = midpoint_subdivide(&welded, &faces, 1);
    assert_eq!(
        (subdivided.len(), cut.len()),
        (9, 8),
        "four merged vertices plus five edge midpoints"
    );
    assert_eq!(
        subdivided
            .iter()
            .filter(|position| **position == [0.5, 0.5, 0.0])
            .count(),
        1,
        "the seam midpoint is shared, not duplicated"
    );
    assert_eq!(boundary_edges(&cut), 8, "the square's border, cut twice");
}

#[test]
fn an_authored_dart_inside_one_piece_stays_open() {
    // One connected piece with two coincident vertices: the panty hip slit and
    // the two bra pinches have exactly this structure.
    let positions = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
    ];
    let triangles = vec![[0u32, 1, 2], [2, 3, 4]];
    let (welded, faces, report) = seam_weld(&positions, &triangles, SEAM_WELD_EPSILON);
    assert_eq!(
        (welded.len(), faces.len()),
        (5, 2),
        "an intra-piece coincidence is an authored feature, not a seam"
    );
    assert!(report.groups.is_empty());
    assert_eq!(report.vertex_map, vec![0, 1, 2, 3, 4]);
    assert_eq!((report.components_before, report.components_after), (1, 1));
    assert_eq!(report.boundary_edges_before, report.boundary_edges_after);
}

#[test]
fn a_seam_beyond_the_tolerance_stays_open() {
    // The whole second piece sits ten tolerances away: no pair merges.
    let far = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [10.0 * SEAM_WELD_EPSILON, 0.0, 0.0],
        [1.0 + 10.0 * SEAM_WELD_EPSILON, 0.0, 0.0],
        [1.0 + 10.0 * SEAM_WELD_EPSILON, 1.0, 0.0],
    ];
    let (welded, _, report) = seam_weld(&far, &[[0u32, 1, 2], [3, 4, 5]], SEAM_WELD_EPSILON);
    assert_eq!(welded.len(), 6, "ten tolerances apart is two pieces");
    assert!(report.groups.is_empty());
    assert_eq!(report.components_before, 2);

    // The rule is a distance, not bit equality: half a tolerance still merges.
    let near = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.5 * SEAM_WELD_EPSILON, 0.0, 0.0],
        [1.0 + 0.5 * SEAM_WELD_EPSILON, 0.0, 0.0],
        [1.0 + 0.5 * SEAM_WELD_EPSILON, 1.0, 0.0],
    ];
    let (welded, _, report) = seam_weld(&near, &[[0u32, 1, 2], [3, 4, 5]], SEAM_WELD_EPSILON);
    assert_eq!(welded.len(), 3, "all three seam pairs merge");
    assert_eq!(report.groups, vec![vec![0, 3], vec![1, 4], vec![2, 5]]);
}

#[test]
fn the_seam_merge_is_deterministic() {
    // A twin seam and a garment with nothing to merge: the scan order must not
    // make either report depend on the run.
    for (positions, triangles) in [split_quad(), patch(4, 0.0, 1.0)] {
        let first = seam_weld(&positions, &triangles, SEAM_WELD_EPSILON);
        let second = seam_weld(&positions, &triangles, SEAM_WELD_EPSILON);
        assert_eq!(first.2, second.2, "the same input reports the same merge");
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
    }
}

#[test]
fn dressing_a_twin_seam_leaves_no_tear() {
    let body = patch(4, 0.0, 2.0);
    let garment = split_quad();
    let options = DressParams {
        cuts: 1,
        ..params()
    };
    let (dressed, faces, report) = dress((&garment.0, &garment.1), (&body.0, &body.1), &options)
        .expect("a twin-seam patch dresses");
    assert_eq!(report.seam_weld.welded_vertices, 4);
    assert_eq!(
        report.source_vertices, 6,
        "the report keeps the source count"
    );
    assert_eq!((dressed.len(), faces.len()), (9, 8));
    assert_eq!(
        boundary_edges(&faces),
        8,
        "the square keeps one closed border: the seam is not a crack"
    );
    // A merged copy has no vertex of its own: both source ids name one dressed
    // vertex, so the solve cannot separate them.
    assert_eq!(
        report.seam_weld.vertex_map[0],
        report.seam_weld.vertex_map[4]
    );
    assert_eq!(dressed[0], dressed[report.seam_weld.vertex_map[4] as usize]);
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

/// Outward-wound cube of half-extent `half`.
fn cube(half: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let positions = [
        [-half, -half, -half],
        [half, -half, -half],
        [half, half, -half],
        [-half, half, -half],
        [-half, -half, half],
        [half, -half, half],
        [half, half, half],
        [-half, half, half],
    ]
    .to_vec();
    let triangles = vec![
        [4u32, 5, 6],
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
    (positions, triangles)
}

/// One sheet facing `+x` at `x`.
fn sheet(x: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![[x, 0.0, 0.0], [x, 0.1, 0.0], [x, 0.0, 0.1]],
        vec![[0u32, 1, 2]],
    )
}

#[test]
fn a_layered_dress_resolves_covered_vertices_against_the_outermost_member() {
    // The sheet sits at x = 0.30: behind the panty shell (-0.20 from its +x
    // face) and behind the body (-0.10 from its own). The outermost-member
    // rule must let the panty own it, so the layered dress seats it outside
    // the panty at the declared band — while the same sheet dressed against
    // the body alone lands at the body's band, inside the panty, which is the
    // covered-crease failure `dress_against` exists to remove.
    let (body, body_faces) = cube(0.4);
    let (panty, panty_faces) = cube(0.5);
    let mut accumulated =
        AccumulatedSurface::from_body(&body, &body_faces).expect("the body seeds the stack");
    accumulated
        .push_layer("panty", 10, &panty, &panty_faces)
        .expect("a valid layer");
    let (garment, faces) = sheet(0.30);
    let options = DressParams {
        clearance: 0.05,
        contact: 0.01,
        ..params()
    };
    let (positions, _, report) =
        dress_against((&garment, &faces), &accumulated, &options).expect("a layered dress");
    let projections = report.projections.expect("a layered report projects");
    assert_eq!(projections.surface, "accumulated[body, panty]");
    assert_eq!(
        projections.members[0].vertices, 0,
        "the body owns nothing the panty covers"
    );
    assert_eq!(
        projections.members[1].vertices, 3,
        "the panty owns the sheet"
    );
    assert_eq!(projections.vertex_members, vec![1, 1, 1]);
    for point in &positions {
        assert!(
            point[0] >= 0.5 && point[0] <= 0.5 + options.clearance + 1e-4,
            "the sheet seats outside the panty in the band: {point:?}"
        );
    }
    let (single_positions, _, single) =
        dress((&garment, &faces), (&body, &body_faces), &options).expect("a plain dress");
    assert!(
        single.projections.is_none(),
        "a single-body dress reports no projections"
    );
    for point in &single_positions {
        assert!(
            point[0] <= 0.4 + options.clearance,
            "the body-only dress lands inside the panty: {point:?}"
        );
    }
}

#[test]
fn a_layered_dress_grades_each_region_against_its_own_member() {
    // A body cube at the origin and a closed panty cube centred at x = 3: one
    // garment of two sheets, one floating over each region. The sheet over
    // bare body resolves against the body and the sheet behind the panty
    // region against the panty — the legs-versus-hips choice of a real
    // two-layer dress, with both sheets seated at their own member's band.
    let (body, body_faces) = cube(0.4);
    let (mut panty, panty_faces) = cube(0.4);
    for point in &mut panty {
        point[0] += 3.0;
    }
    let mut accumulated =
        AccumulatedSurface::from_body(&body, &body_faces).expect("the body seeds the stack");
    accumulated
        .push_layer("panty", 10, &panty, &panty_faces)
        .expect("a valid layer");
    let (near, _) = sheet(0.60);
    let (far, _) = sheet(3.60);
    let garment = [near, far].concat();
    let faces = vec![[0u32, 1, 2], [3, 4, 5]];
    let options = DressParams {
        clearance: 0.05,
        contact: 0.01,
        ..params()
    };
    let (positions, _, report) =
        dress_against((&garment, &faces), &accumulated, &options).expect("a layered dress");
    let projections = report.projections.expect("a layered report projects");
    assert_eq!(
        projections.vertex_members,
        vec![0, 0, 0, 1, 1, 1],
        "each sheet resolves against its own region's member"
    );
    assert_eq!(projections.members[0].vertices, 3);
    assert_eq!(projections.members[1].vertices, 3);
    for point in &positions[0..3] {
        assert!(
            (point[0] - 0.45).abs() < 0.01,
            "the bare-body sheet seats at the body's band: {point:?}"
        );
    }
    for point in &positions[3..6] {
        assert!(
            (point[0] - 3.45).abs() < 0.01,
            "the panty-region sheet seats at the panty's band: {point:?}"
        );
    }
}

/// An open shell above the body: two triangles at `x = 0.5` spanning
/// `y ∈ [0.8, 1.6]`, `z ∈ [-0.5, 0.5]`, wound to face `+x`. Its hem at
/// `y = 0.8` is an authored opening — the case the closed-cube layered tests
/// cannot reach.
fn open_shell() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [0.5, 0.8, -0.5],
            [0.5, 0.8, 0.5],
            [0.5, 1.6, 0.5],
            [0.5, 1.6, -0.5],
        ],
        vec![[0u32, 2, 1], [0, 3, 2]],
    )
}

#[test]
fn an_open_hem_owns_only_what_it_covers() {
    // Two garment sheets against body + open shell: the upper one lies under
    // the shell's own surface and must resolve against the shell, the lower one
    // sits inside the body below the shell's hem — 0.2 behind the hem
    // triangle's extended plane but 0.71 away from the hem itself, where a
    // rim's half-space stops being a surface.
    let (body, body_faces) = cube(0.4);
    let (shell, shell_faces) = open_shell();
    let mut accumulated =
        AccumulatedSurface::from_body(&body, &body_faces).expect("the body seeds the stack");
    accumulated
        .push_layer("shell", 10, &shell, &shell_faces)
        .expect("an open shell is a valid layer");
    assert_eq!(accumulated.surface_label(), "accumulated[body, shell]");

    let garment = [
        [0.3, 1.0, 0.0],
        [0.3, 1.1, 0.0],
        [0.3, 1.0, 0.1],
        // Below the hem, inside the body.
        [0.3, 0.2, 0.0],
        [0.3, 0.3, 0.0],
        [0.3, 0.2, 0.1],
    ]
    .to_vec();
    let faces = vec![[0u32, 1, 2], [3, 4, 5]];
    let options = DressParams {
        clearance: 0.05,
        contact: 0.01,
        ..params()
    };
    let (positions, _, report) =
        dress_against((&garment, &faces), &accumulated, &options).expect("a layered dress");

    let projections = report.projections.expect("a layered report projects");
    assert_eq!(
        projections.vertex_members,
        vec![1, 1, 1, 0, 0, 0],
        "the shell owns its own covered sheet; the body owns the sheet below its hem"
    );
    assert_eq!(
        report.max_floating_gap, 0.0,
        "no vertex floats outside the band"
    );
    assert_eq!(
        report.max_penetration, 0.0,
        "no vertex is left behind the band"
    );
    for point in &positions[0..3] {
        assert!(
            (point[0] - 0.51).abs() < 5e-3,
            "the shell-covered sheet seats at the shell's contact: {point:?}"
        );
    }
    for point in &positions[3..6] {
        // 0.41 is the body's `+x`, `+y` or `+z` face plus the declared contact:
        // the body's band, never the shell's 0.51.
        let standoff = point
            .iter()
            .fold(0.0f32, |widest, value| widest.max(value.abs()));
        assert!(
            (standoff - 0.41).abs() < 5e-3,
            "the sheet below the hem seats at the body's contact, never the hem's: {point:?}"
        );
    }
}
