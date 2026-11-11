//! Mixed garment regions preserve their source correspondence. @req DECL0013

use sculpto_mesh::mask::VertexMask;
use sculpto_mesh::outfit::rigging::{RegionFitError, fit_rigged_regions};
use sculpto_mesh::outfit::{ClothDefect, ClothRegion};
use sculpto_mesh::wrap::{FitOptions, GapMode};

const BODY: [[f32; 3]; 4] = [
    [-2.0, -2.0, 0.0],
    [2.0, -2.0, 0.0],
    [2.0, 2.0, 0.0],
    [-2.0, 2.0, 0.0],
];
const BODY_FACES: [[u32; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

fn cloth(triangles: Option<Vec<usize>>) -> Vec<ClothRegion> {
    vec![ClothRegion {
        name: "loose".into(),
        triangles,
    }]
}

#[test]
fn compact_region_retains_settings_and_cloth_excludes_inside_counts() {
    let garment = [
        [4.0, 0.0, -1.0],
        [5.0, 0.0, -1.0],
        [4.0, 1.0, -1.0],
        [-0.5, -0.5, -0.2],
        [0.5, -0.5, -0.2],
        [0.0, 0.5, -0.2],
        [0.0, 0.0, -10.0],
    ];
    let triangles = [[0, 1, 2], [3, 4, 5]];
    let options = FitOptions {
        gap_mode: GapMode::Keep,
        target_gaps: vec![0.1; garment.len()],
        rigid_parts: vec![-1; garment.len()],
        ..FitOptions::default()
    };
    let (fitted, report) = fit_rigged_regions(
        (&garment, &triangles),
        (&BODY, &BODY_FACES),
        &cloth(Some(vec![0])),
        None,
        &options,
    )
    .expect("compact region settings retain source indices");
    assert_eq!(&fitted[..3], &garment[..3]);
    assert_eq!(
        fitted[6], garment[6],
        "unused source vertex must retain its position"
    );
    assert!(fitted[3..6].iter().all(|point| point[2] > 0.0));
    assert_eq!(report.ordinary.inside_after, 0);
    assert_eq!(report.ordinary.intersection_pairs, 0);
}

#[test]
fn coincident_seam_copies_and_explicit_locks_stay_fixed() {
    let garment = [
        [-0.5, -0.5, -0.2],
        [4.0, 0.0, -0.2],
        [4.0, 1.0, -0.2],
        [-0.5, -0.5, -0.2],
        [0.5, -0.5, -0.2],
        [0.0, 0.5, -0.2],
    ];
    let triangles = [[0, 1, 2], [3, 4, 5]];
    let lock = VertexMask::from_values(vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0]).expect("lock mask");
    let (fitted, _) = fit_rigged_regions(
        (&garment, &triangles),
        (&BODY, &BODY_FACES),
        &cloth(Some(vec![0])),
        Some(&lock),
        &FitOptions::default(),
    )
    .expect("seam-preserving fit");
    assert_eq!(&fitted[..3], &garment[..3]);
    assert_eq!(
        fitted[3], garment[3],
        "ordinary copy of the cloth seam must remain fixed"
    );
    assert_eq!(
        fitted[4], garment[4],
        "explicit source lock must survive compaction"
    );
}

#[test]
fn whole_cloth_selection_reports_contacts_without_fitting() {
    let garment = [[-0.5, -0.5, -0.5], [0.5, -0.5, 0.5], [0.0, 0.5, 0.5]];
    let triangles = [[0, 1, 2]];
    let (fitted, report) = fit_rigged_regions(
        (&garment, &triangles),
        (&BODY, &BODY_FACES),
        &cloth(None),
        None,
        &FitOptions::default(),
    )
    .expect("whole cloth selection");
    assert_eq!(fitted, garment);
    assert_eq!(report.ordinary_triangles, 0);
    assert_eq!(report.ordinary.inside_after, 0);
    assert!(report.cloth_intersection_pairs > 0);
    assert_eq!(report.cloth_intersection_pairs, report.intersection_pairs);
}

#[test]
fn invalid_selection_fails_before_compacting_geometry() {
    let garment = [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [0.0, 1.0, 1.0]];
    let error = fit_rigged_regions(
        (&garment, &[[0, 1, 2]]),
        (&BODY, &BODY_FACES),
        &cloth(Some(vec![1])),
        None,
        &FitOptions::default(),
    )
    .expect_err("source face 1 does not exist");
    assert!(matches!(
        error,
        RegionFitError::Selection(ClothDefect::TriangleOutOfRange { triangle: 1, .. })
    ));
}
