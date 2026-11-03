//! Intake behaviour: the audit measurements, the route verdicts, and the
//! strip-interior repair.

use sculpto_mesh::classify::ClassifyError;
use sculpto_mesh::intake::{Route, audit, strip_interior};

const SEGMENTS: usize = 24;

fn tube(cx: f32, cy: f32, radius: f32, heights: &[f32]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    for (level, height) in heights.iter().enumerate() {
        for index in 0..SEGMENTS {
            let angle = std::f32::consts::TAU * index as f32 / SEGMENTS as f32;
            positions.push([
                cx + radius * angle.cos(),
                cy + radius * angle.sin(),
                *height,
            ]);
            if level > 0 {
                let current = (level * SEGMENTS + index) as u32;
                let next = (level * SEGMENTS + (index + 1) % SEGMENTS) as u32;
                let below = current - SEGMENTS as u32;
                let below_next = next - SEGMENTS as u32;
                triangles.push([below, below_next, current]);
                triangles.push([below_next, next, current]);
            }
        }
    }
    (positions, triangles)
}

fn merge(
    (mut left_positions, mut left_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
    (right_positions, right_triangles): (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let offset = left_positions.len() as u32;
    left_positions.extend(right_positions);
    left_triangles.extend(
        right_triangles
            .into_iter()
            .map(|[a, b, c]| [a + offset, b + offset, c + offset]),
    );
    (left_positions, left_triangles)
}

/// Torso (z 45..85, r 10), head (86..100, r 6), legs (0..45, r 4 at ±6).
fn mannequin() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    merge(
        merge(
            tube(0.0, 0.0, 10.0, &[45.0, 65.0, 85.0]),
            tube(0.0, 0.0, 6.0, &[86.0, 100.0]),
        ),
        merge(
            tube(-6.0, 0.0, 4.0, &[0.0, 22.0, 45.0]),
            tube(6.0, 0.0, 4.0, &[0.0, 22.0, 45.0]),
        ),
    )
}

fn audit_of(garment: (Vec<[f32; 3]>, Vec<[u32; 3]>)) -> sculpto_mesh::intake::IntakeReport {
    let (body, body_triangles) = mannequin();
    audit(&body, &body_triangles, &garment.0, &garment.1).expect("audit runs")
}

/// Merge two meshes and stitch quads between their top rings so they form
/// one connected component: a lining attached to its shell.
fn lined(
    shell: (Vec<[f32; 3]>, Vec<[u32; 3]>),
    lining: (Vec<[f32; 3]>, Vec<[u32; 3]>),
) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let lining_vertices = lining.0.len();
    let (positions, mut triangles) = merge(shell, lining);
    let outer_top = (positions.len() - lining_vertices) - SEGMENTS;
    let inner_top = positions.len() - SEGMENTS;
    for index in 0..SEGMENTS {
        let next = (index + 1) % SEGMENTS;
        triangles.push([
            (outer_top + index) as u32,
            (outer_top + next) as u32,
            (inner_top + index) as u32,
        ]);
        triangles.push([
            (outer_top + next) as u32,
            (inner_top + next) as u32,
            (inner_top + index) as u32,
        ]);
    }
    (positions, triangles)
}

#[test]
fn a_clean_open_tube_fits() {
    let report = audit_of(tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]));
    assert_eq!(report.route, Route::Fit);
    assert_eq!(report.major_components, 1);
    assert_eq!(report.max_interior_chains, 0, "one clean shell");
    assert!(
        report.openings.iter().any(|o| o.edges == SEGMENTS),
        "the hem and collar rings are the openings: {:?}",
        report.openings
    );
    assert!(report.evidence.iter().any(|line| line.contains("cloth")));
}

#[test]
fn render_seams_do_not_change_physical_intake() {
    let (positions, triangles) = tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]);
    let original = audit_of((positions.clone(), triangles.clone()));
    let split_positions: Vec<_> = triangles
        .iter()
        .flat_map(|triangle| triangle.map(|index| positions[index as usize]))
        .collect();
    let split_triangles: Vec<_> = (0..triangles.len() as u32)
        .map(|index| [index * 3, index * 3 + 1, index * 3 + 2])
        .collect();
    let split = audit_of((split_positions, split_triangles));
    assert_eq!(split.indexed_components, triangles.len());
    assert_eq!(split.topology_vertices, positions.len());
    assert_eq!(split.route, original.route);
    assert_eq!(split.major_components, original.major_components);
    assert_eq!(split.openings.len(), original.openings.len());
    assert_eq!(split.self_intersections, original.self_intersections);
}

#[test]
fn closed_render_shell_is_not_ready_cloth() {
    let positions = vec![
        [-1.0, -1.0, 50.0],
        [1.0, -1.0, 50.0],
        [0.0, 1.0, 50.0],
        [0.0, 0.0, 52.0],
    ];
    let triangles = vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [2, 0, 3]];
    let report = audit_of((positions, triangles));
    assert!(report.openings.is_empty());
    assert_eq!(
        report.route,
        Route::Repair {
            defect: "closed surface requires a cloth/control surface"
        }
    );
}

#[test]
fn nearby_layers_are_not_welded() {
    let pair = merge(
        tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]),
        tube(0.0, 0.0, 13.0001, &[42.0, 60.0, 80.0]),
    );
    let report = audit_of(pair);
    assert_eq!(report.topology_vertices, SEGMENTS * 6);
    assert_eq!(report.major_components, 2);
    assert_eq!(report.route, Route::SplitParts);
}

#[test]
fn coincident_layers_are_not_accepted_as_render_seams() {
    let layer = tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]);
    let report = audit_of(merge(layer.clone(), layer));
    assert_eq!(
        report.route,
        Route::Repair {
            defect: "coincident or collapsed faces"
        }
    );
}

#[test]
fn an_attached_inner_shell_means_interior_surfaces() {
    let report = audit_of(lined(
        tube(0.0, 0.0, 16.0, &[20.0, 40.0, 60.0]),
        tube(0.0, 0.0, 12.0, &[22.0, 40.0, 58.0]),
    ));
    assert_eq!(
        report.route,
        Route::Repair {
            defect: "interior surfaces"
        }
    );
    assert_eq!(report.major_components, 1, "the lining is attached");
    assert!(report.max_interior_chains >= 1);
    assert!(report.evidence.iter().any(|line| line.contains("interior")));
}

#[test]
fn offset_contours_are_not_misclassified_by_centroid_radius() {
    let (body, body_triangles) = mannequin();
    let garment = merge(
        tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]),
        tube(28.0, 0.0, 4.0, &[42.0, 60.0, 80.0]),
    );
    let report = audit(&body, &body_triangles, &garment.0, &garment.1).expect("audit runs");
    assert_eq!(report.max_interior_chains, 0);
    let (kept, report) =
        strip_interior(&body, &body_triangles, &garment.0, &garment.1).expect("strip runs");
    assert_eq!(report.removed, 0);
    assert_eq!(kept, garment.1);
}

#[test]
fn thin_nested_layers_remain_distinct_in_meter_scale() {
    let (body, body_triangles) = mannequin();
    let garment = lined(
        tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]),
        tube(0.0, 0.0, 12.8, &[43.0, 60.0, 79.0]),
    );
    let meters = |points: Vec<[f32; 3]>| {
        points
            .into_iter()
            .map(|point| point.map(|v| v * 0.02))
            .collect::<Vec<_>>()
    };
    let report = audit(
        &meters(body),
        &body_triangles,
        &meters(garment.0),
        &garment.1,
    )
    .expect("audit runs");
    assert!(report.max_interior_chains >= 1);
    assert_eq!(
        report.route,
        Route::Repair {
            defect: "interior surfaces"
        }
    );
}

#[test]
fn two_major_parts_split() {
    let pair = merge(
        tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]),
        tube(0.0, 0.0, 16.0, &[20.0, 40.0, 60.0]),
    );
    let report = audit_of(pair);
    assert_eq!(report.route, Route::SplitParts);
    assert_eq!(report.major_components, 2);
    assert!(
        report
            .shares
            .iter()
            .all(|share| (40.0..=60.0).contains(share))
    );
}

#[test]
fn a_slab_through_the_garment_is_a_rest_defect() {
    let (positions, triangles) = tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]);
    // A wide slab punched through the tube above the noise floor.
    let slab = (
        vec![
            [-20.0, -20.0, 61.0],
            [20.0, -20.0, 61.0],
            [20.0, 20.0, 61.0],
            [-20.0, 20.0, 61.0],
        ],
        vec![[0u32, 1, 2], [0, 2, 3]],
    );
    let report = audit_of(merge((positions, triangles), slab));
    assert_eq!(
        report.route,
        Route::Repair {
            defect: "self-intersections at rest"
        }
    );
    assert!(report.self_intersections > 0);
    assert!(
        report.self_intersections as f32 > 0.05 * report.triangles as f32,
        "gross, not band-corner noise: {} of {}",
        report.self_intersections,
        report.triangles
    );
}

#[test]
fn rejects_broken_inputs_like_classify() {
    let (body, body_triangles) = mannequin();
    let (garment, triangles) = tube(0.0, 0.0, 13.0, &[42.0, 80.0]);
    assert_eq!(
        audit(&body, &body_triangles, &[], &triangles).unwrap_err(),
        ClassifyError::EmptyGarment
    );
    assert_eq!(
        audit(&body, &body_triangles, &garment, &[]).unwrap_err(),
        ClassifyError::NoGarmentTriangles
    );
}

#[test]
fn strip_removes_the_lining_and_keeps_the_shell() {
    let (body, body_triangles) = mannequin();
    let shell = tube(0.0, 0.0, 16.0, &[20.0, 40.0, 60.0]);
    let lining = tube(0.0, 0.0, 12.0, &[22.0, 40.0, 58.0]);
    let garment = lined(shell.clone(), lining);
    let (kept, report) =
        strip_interior(&body, &body_triangles, &garment.0, &garment.1).expect("strip runs");
    assert!(report.removed > 0, "the lining is interior");
    assert!(
        report.kept >= shell.1.len(),
        "the outer shell survives: kept {} of at least {}",
        report.kept,
        shell.1.len()
    );
    let repaired = audit(&body, &body_triangles, &garment.0, &kept).expect("re-audit");
    assert_eq!(repaired.route, Route::Fit, "{:?}", repaired.evidence);
}

#[test]
fn strip_leaves_a_clean_garment_untouched() {
    let (body, body_triangles) = mannequin();
    let garment = tube(0.0, 0.0, 13.0, &[42.0, 60.0, 80.0]);
    let (kept, report) =
        strip_interior(&body, &body_triangles, &garment.0, &garment.1).expect("strip runs");
    assert_eq!(report.removed, 0);
    assert_eq!(kept, garment.1);
}
