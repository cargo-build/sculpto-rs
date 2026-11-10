use sculpto_mesh::silhouette::{
    MeshRole, OrthographicFrame, ProjectionPlane, RasterSettings, SilhouetteError, compare,
};

const FACES: [[u32; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

fn square(half: f32, shift: f32) -> [[f32; 3]; 4] {
    [
        [shift - half, 0., -half],
        [shift + half, 0., -half],
        [shift + half, 0., half],
        [shift - half, 0., half],
    ]
}

fn frame() -> OrthographicFrame {
    OrthographicFrame::new(
        [[-2., 2.], [-2., 2.]],
        RasterSettings {
            resolution: 32,
            plane: ProjectionPlane::Xz,
        },
    )
    .expect("declared frame")
}

#[test]
fn identity_and_winding_use_the_same_triangle_union() {
    let mesh = square(0.5, 0.);
    let reversed = FACES.map(|[a, b, c]| [a, c, b]);
    let result = compare((&mesh, &FACES), (&mesh, &reversed), &frame()).expect("overlap");
    assert_eq!(result.reference_pixels, 64);
    assert_eq!(result.candidate_pixels, 64);
    assert_eq!(result.intersection_pixels, 64);
    assert_eq!(result.union_pixels, 64);
    assert_eq!(result.iou, 1.);
}

#[test]
fn inflation_is_measured_without_independent_rescaling() {
    let before = square(0.5, 0.);
    let after = square(1., 0.);
    let result = compare((&before, &FACES), (&after, &FACES), &frame()).expect("overlap");
    assert_eq!(result.reference_pixels, 64);
    assert_eq!(result.candidate_pixels, 256);
    assert_eq!(result.intersection_pixels, 64);
    assert_eq!(result.union_pixels, 256);
    assert_eq!(result.iou, 0.25);
}

#[test]
fn translation_is_measured_without_recentering() {
    let before = square(0.5, 0.);
    let after = square(0.5, 0.5);
    let result = compare((&before, &FACES), (&after, &FACES), &frame()).expect("overlap");
    assert_eq!(result.intersection_pixels, 32);
    assert_eq!(result.union_pixels, 96);
    assert_eq!(result.iou, 1. / 3.);
}

#[test]
fn collapsed_candidate_scores_zero_and_empty_reference_is_unmeasurable() {
    let before = square(0.5, 0.);
    let collapsed = [[0.; 3]; 4];
    let result = compare((&before, &FACES), (&collapsed, &FACES), &frame()).expect("overlap");
    assert_eq!(result.candidate_pixels, 0);
    assert_eq!(result.iou, 0.);
    assert!(matches!(
        compare((&collapsed, &FACES), (&before, &FACES), &frame()),
        Err(SilhouetteError::EmptyReference)
    ));
}

#[test]
fn one_view_cannot_certify_depth_fit() {
    let before = square(0.5, 0.);
    let after = before.map(|[x, _, z]| [x, 100., z]);
    assert_eq!(
        compare((&before, &FACES), (&after, &FACES), &frame())
            .expect("overlap")
            .iou,
        1.
    );
}

#[test]
fn invalid_frames_geometry_and_cropping_are_explicit_errors() {
    assert!(matches!(
        OrthographicFrame::new(
            [[-1., 1.], [-1., 1.]],
            RasterSettings {
                resolution: 0,
                ..RasterSettings::default()
            }
        ),
        Err(SilhouetteError::ZeroResolution)
    ));
    for bounds in [
        [[0., 0.], [-1., 1.]],
        [[2., 1.], [-1., 1.]],
        [[0., f64::INFINITY], [-1., 1.]],
    ] {
        assert!(matches!(
            OrthographicFrame::new(bounds, RasterSettings::default()),
            Err(SilhouetteError::InvalidBounds)
        ));
    }
    let mesh = square(0.5, 0.);
    let outside = square(3., 0.);
    assert!(matches!(
        compare((&mesh, &FACES), (&outside, &FACES), &frame()),
        Err(SilhouetteError::OutsideFrame {
            role: MeshRole::Candidate,
            ..
        })
    ));
    let mut broken = mesh;
    broken[1][1] = f32::NAN;
    assert!(matches!(
        compare((&broken, &FACES), (&mesh, &FACES), &frame()),
        Err(SilhouetteError::NonFiniteVertex {
            role: MeshRole::Reference,
            vertex: 1
        })
    ));
    assert!(matches!(
        compare((&mesh, &FACES), (&mesh, &[[0, 1, 4]]), &frame()),
        Err(SilhouetteError::FaceIndex {
            role: MeshRole::Candidate,
            triangle: 0,
            vertex: 4
        })
    ));
    assert!(matches!(
        compare((&mesh, &[]), (&mesh, &FACES), &frame()),
        Err(SilhouetteError::EmptyMesh {
            role: MeshRole::Reference
        })
    ));
    assert!(matches!(
        compare((&mesh, &FACES), (&[], &FACES), &frame()),
        Err(SilhouetteError::EmptyMesh {
            role: MeshRole::Candidate
        })
    ));
}
