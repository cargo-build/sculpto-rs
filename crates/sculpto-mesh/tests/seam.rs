use sculpto_mesh::seam::{SeamAnchor, SeamError, SeamOptions, fit_seam};

fn options() -> SeamOptions {
    SeamOptions {
        collar_radius: 3.0,
        max_shift: 0.5,
    }
}

#[test]
fn exact_anchors_propagate_across_uv_aliases_without_changing_order() {
    let points = [
        [0., 0., 0.],
        [1., 0., 0.],
        [0., 1., 0.],
        [0., 0., 0.],
        [-1., 0., 0.],
        [0., -1., 0.],
    ];
    let faces = [[0, 1, 2], [3, 4, 5]];
    let fit = fit_seam(
        &points,
        &faces,
        &[SeamAnchor {
            vertex: 3,
            target: [0., 0., 0.25],
        }],
        &options(),
    )
    .unwrap();
    assert_eq!(fit.positions.len(), points.len());
    assert_eq!(fit.positions[0], [0., 0., 0.25]);
    assert_eq!(fit.positions[3], fit.positions[0]);
    assert!(fit.positions[1][2] > 0.0 && fit.positions[4][2] > 0.0);
    assert!(fit.max_shift <= 0.25);
    let repeated = fit_seam(
        &points,
        &faces,
        &[
            SeamAnchor {
                vertex: 0,
                target: [0., 0., 0.25],
            },
            SeamAnchor {
                vertex: 3,
                target: [0., 0., 0.25],
            },
        ],
        &options(),
    )
    .unwrap();
    assert_eq!(repeated, fit);
}

#[test]
fn collar_follows_graph_distance_and_leaves_outside_and_disconnected_vertices_exact() {
    let points = [
        [0., 0., 0.],
        [1., 0., 1.],
        [2., 0., 0.],
        [3., 0., -1.],
        [4., 0., 0.],
        [0.01, -0.0, 0.],
    ];
    let fit = fit_seam(
        &points,
        &[[0, 1, 2], [2, 3, 4]],
        &[SeamAnchor {
            vertex: 0,
            target: [0., 0., 0.25],
        }],
        &options(),
    )
    .unwrap();
    assert!(fit.positions[1][2] > points[1][2]);
    assert!(fit.positions[2][2] > 0.0);
    [3, 4, 5].iter().for_each(|&i| {
        assert_eq!(
            fit.positions[i].map(f32::to_bits),
            points[i].map(f32::to_bits)
        )
    });
    let t = 2.0_f64.sqrt() / 3.0;
    let expected = 1.0 + 0.25 * (1.0 - t * t * (3.0 - 2.0 * t));
    assert!((f64::from(fit.positions[1][2]) - expected).abs() < 1e-7);
}

#[test]
fn conflicting_alias_targets_are_refused_before_any_source_mutation() {
    let points = [[0., 0., 0.], [-0.0, 0., 0.]];
    let anchors = [
        SeamAnchor {
            vertex: 0,
            target: [0., 0., 0.1],
        },
        SeamAnchor {
            vertex: 1,
            target: [0., 0., 0.2],
        },
    ];
    assert_eq!(
        fit_seam(&points, &[], &anchors, &options()),
        Err(SeamError::ConflictingTargets { vertex: 1 })
    );
    assert_eq!(points[1][0].to_bits(), (-0.0_f32).to_bits());
}

#[test]
fn correspondence_order_does_not_change_equal_distance_ties() {
    let points = [[-1., 0., 0.], [0., 0., 0.], [1., 0., 0.]];
    let a = SeamAnchor {
        vertex: 0,
        target: [-1., 0., 0.1],
    };
    let b = SeamAnchor {
        vertex: 2,
        target: [1., 0., 0.2],
    };
    let first = fit_seam(&points, &[[0, 1, 2]], &[a, b], &options()).unwrap();
    let second = fit_seam(&points, &[[0, 1, 2]], &[b, a], &options()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.positions[0], a.target);
    assert_eq!(first.positions[2], b.target);
}

#[test]
fn explicit_guards_reject_invalid_inputs_and_excessive_motion() {
    let p = [[0., 0., 0.]];
    let a = [SeamAnchor {
        vertex: 0,
        target: [0., 0., 0.25],
    }];
    assert_eq!(
        fit_seam(
            &p,
            &[],
            &a,
            &SeamOptions {
                max_shift: 0.2,
                ..options()
            }
        ),
        Err(SeamError::ShiftLimit { vertex: 0 })
    );
    [0., -1., f32::INFINITY, f32::NAN]
        .iter()
        .for_each(|&radius| {
            assert_eq!(
                fit_seam(
                    &p,
                    &[],
                    &a,
                    &SeamOptions {
                        collar_radius: radius,
                        ..options()
                    }
                ),
                Err(SeamError::InvalidOptions)
            );
        });
    [0., -1., f32::INFINITY, f32::NAN]
        .iter()
        .for_each(|&max_shift| {
            assert_eq!(
                fit_seam(
                    &p,
                    &[],
                    &a,
                    &SeamOptions {
                        max_shift,
                        ..options()
                    }
                ),
                Err(SeamError::InvalidOptions)
            );
        });
    assert_eq!(
        fit_seam(&[], &[], &a, &options()),
        Err(SeamError::EmptyInput)
    );
    assert_eq!(
        fit_seam(
            &[[1e38, 0., 0.], [3e38, 0., 0.], [3e38, 1e38, 0.]],
            &[[0, 1, 2]],
            &[SeamAnchor {
                vertex: 0,
                target: [3e38, 0., 0.]
            }],
            &SeamOptions {
                collar_radius: f32::MAX,
                max_shift: f32::MAX
            }
        ),
        Err(SeamError::NonFiniteOutput { vertex: 1 })
    );
    assert_eq!(
        fit_seam(&p, &[[0, 0, 1]], &a, &options()),
        Err(SeamError::InvalidIndex { vertex: 1 })
    );
    assert_eq!(
        fit_seam(
            &p,
            &[],
            &[SeamAnchor {
                vertex: 1,
                target: [0.; 3]
            }],
            &options()
        ),
        Err(SeamError::InvalidIndex { vertex: 1 })
    );
    assert_eq!(
        fit_seam(
            &p,
            &[],
            &[SeamAnchor {
                vertex: 0,
                target: [f32::NAN; 3]
            }],
            &options()
        ),
        Err(SeamError::NonFinite { vertex: 0 })
    );
    assert_eq!(
        fit_seam(&[[f32::NAN; 3]], &[], &a, &options()),
        Err(SeamError::NonFinite { vertex: 0 })
    );
    assert_eq!(
        fit_seam(&p, &[], &[], &options()),
        Err(SeamError::EmptyInput)
    );
}

#[test]
fn vertex_at_radius_is_unchanged_and_reapplying_targets_is_idempotent() {
    let points = [[0., 0., 0.], [3., 0., 0.], [0., 4., 0.]];
    let anchors = [SeamAnchor {
        vertex: 0,
        target: [0., 0., 0.25],
    }];
    let fit = fit_seam(&points, &[[0, 1, 2]], &anchors, &options()).unwrap();
    assert_eq!(fit.changed_vertices, [0]);
    let again = fit_seam(&fit.positions, &[[0, 1, 2]], &anchors, &options()).unwrap();
    assert_eq!(again.positions, fit.positions);
    assert!(again.changed_vertices.is_empty());
}
