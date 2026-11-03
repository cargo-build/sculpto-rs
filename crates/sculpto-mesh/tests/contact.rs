//! Bounded local contact separation: policy, bounds, coupling and invariance.

use sculpto_mesh::check::{self, WitnessKind};
use sculpto_mesh::control::contact::{ContactError, ContactMode, ContactPolicy, separate};

/// One large triangle in the z = 0 plane and one smaller triangle crossing it.
fn crossing() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    (
        vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.2, 0.2, -0.2],
            [0.2, 0.2, 0.2],
            [0.5, 0.2, 0.2],
        ],
        vec![[0, 1, 2], [3, 4, 5]],
    )
}

fn policy(mode: ContactMode) -> ContactPolicy {
    ContactPolicy {
        mode,
        clearance: 1e-3,
        max_displacement: 0.5,
        max_pairs: 1,
    }
}

#[test]
fn counting_and_listing_share_one_predicate() {
    let (positions, triangles) = crossing();
    let witnesses = check::self_intersection_witnesses(&positions, &triangles);
    assert_eq!(
        witnesses.len(),
        check::self_intersection_pairs(&positions, &triangles)
    );
    assert_eq!(witnesses.len(), 1);
    assert_eq!(witnesses[0].kind, WitnessKind::Crossing);
    let clean = vec![[0, 1, 2]];
    assert!(check::self_intersection_witnesses(&positions, &clean).is_empty());
}

#[test]
fn a_contact_free_surface_needs_no_separation() {
    let (positions, _) = crossing();
    let triangles = vec![[0, 1, 2]];
    assert_eq!(
        separate(&positions, &triangles, &policy(ContactMode::Separate)),
        Ok(None)
    );
}

#[test]
fn one_local_crossing_is_separated_within_the_bound() {
    let (positions, triangles) = crossing();
    let outcome = separate(&positions, &triangles, &policy(ContactMode::Separate))
        .unwrap()
        .expect("one witnessed pair is separable");
    assert_eq!(
        outcome.positions.len(),
        positions.len(),
        "separation never reorders or adds vertices"
    );
    let separation = &outcome.separation;
    assert_eq!(separation.pairs, 1);
    assert_eq!(separation.moving_face, 1, "the smaller face moves");
    assert_eq!(separation.fixed_face, 0);
    assert!(separation.translation > 0.0);
    assert!(separation.max_displacement <= 0.5);
    assert!(separation.min_incident_area_ratio > 0.0);
    assert!(!separation.moved.is_empty());
    assert!(
        check::self_intersection_witnesses(&outcome.positions, &triangles).is_empty(),
        "a completed separation leaves no contact"
    );
    assert_eq!(
        outcome.positions[1], positions[1],
        "vertices outside the local patch are untouched"
    );
}

#[test]
fn a_bound_too_small_refuses_instead_of_searching() {
    let (positions, triangles) = crossing();
    let strict = ContactPolicy {
        clearance: 1e-5,
        max_displacement: 1e-4,
        ..policy(ContactMode::Separate)
    };
    match separate(&positions, &triangles, &strict) {
        Err(ContactError::DisplacementBeyondBound { required, limit }) => {
            assert!(required > limit);
        }
        other => panic!("expected a bound refusal, got {other:?}"),
    }
}

#[test]
fn coupled_contacts_exceed_the_declared_pair_limit() {
    let (mut positions, mut triangles) = crossing();
    let offset = [3.0, 0.0, 0.0];
    let base = positions.len() as u32;
    let mut second = positions
        .iter()
        .map(|point| {
            [
                point[0] + offset[0],
                point[1] + offset[1],
                point[2] + offset[2],
            ]
        })
        .collect::<Vec<_>>();
    positions.append(&mut second);
    triangles.extend([[base, base + 1, base + 2], [base + 3, base + 4, base + 5]]);
    assert_eq!(check::self_intersection_pairs(&positions, &triangles), 2);
    assert_eq!(
        separate(&positions, &triangles, &policy(ContactMode::Separate)),
        Err(ContactError::CoupledContacts { pairs: 2, limit: 1 })
    );
    let generous = ContactPolicy {
        max_pairs: 2,
        ..policy(ContactMode::Separate)
    };
    assert_eq!(
        separate(&positions, &triangles, &generous),
        Err(ContactError::CoupledContacts { pairs: 2, limit: 1 })
    );
}

#[test]
fn a_reject_policy_never_moves_geometry() {
    let (positions, triangles) = crossing();
    match separate(&positions, &triangles, &policy(ContactMode::Reject)) {
        Err(ContactError::CoupledContacts { pairs, limit }) => {
            assert_eq!(pairs, 1);
            assert_eq!(limit, 0);
        }
        other => panic!("expected a reject-policy refusal, got {other:?}"),
    }
}

#[test]
fn duplicate_faces_are_not_separated_by_moving_geometry() {
    let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let triangles = vec![[0, 1, 2], [0, 1, 2]];
    assert_eq!(
        separate(&positions, &triangles, &policy(ContactMode::Separate)),
        Err(ContactError::DuplicateFace { left: 0, right: 1 })
    );
}

#[test]
fn transformed_and_reindexed_geometry_decides_the_same_way() {
    let (positions, triangles) = crossing();
    let base = separate(&positions, &triangles, &policy(ContactMode::Separate))
        .unwrap()
        .expect("separable");

    // Translate, scale and relabel the same geometry: no fixture-specific branch.
    let factor = 3.0f32;
    let offset = [-7.0, 2.5, 1.25];
    let moved = positions
        .iter()
        .map(|point| {
            [
                point[0] * factor + offset[0],
                point[1] * factor + offset[1],
                point[2] * factor + offset[2],
            ]
        })
        .collect::<Vec<_>>();
    // Reverse the vertex numbering inside each triangle (same surface, new ids).
    let relabelled = triangles
        .iter()
        .map(|[a, b, c]| [*c, *b, *a])
        .collect::<Vec<_>>();
    let scaled_policy = ContactPolicy {
        clearance: 1e-3 * factor,
        max_displacement: 0.5 * factor,
        ..policy(ContactMode::Separate)
    };
    let transformed = separate(&moved, &relabelled, &scaled_policy)
        .unwrap()
        .expect("the same crossing is separable after a transform");
    assert_eq!(
        transformed.separation.moving_face,
        base.separation.moving_face
    );
    assert_eq!(
        transformed.separation.fixed_face,
        base.separation.fixed_face
    );
    assert_eq!(transformed.separation.axis, base.separation.axis);
    let ratio = transformed.separation.translation / base.separation.translation.max(f32::EPSILON);
    assert!(
        (ratio - factor).abs() < 0.01,
        "translation scales with the geometry, got {ratio}"
    );
    assert!(check::self_intersection_witnesses(&transformed.positions, &relabelled).is_empty());
}

#[test]
fn invalid_inputs_and_policies_are_reported() {
    let (positions, triangles) = crossing();
    assert_eq!(
        separate(&positions, &[], &policy(ContactMode::Separate)),
        Err(ContactError::InvalidInput)
    );
    assert_eq!(
        separate(&positions, &[[0, 1, 99]], &policy(ContactMode::Separate)),
        Err(ContactError::InvalidInput)
    );
    let bad_clearance = ContactPolicy {
        clearance: 0.0,
        ..policy(ContactMode::Separate)
    };
    assert!(matches!(
        separate(&positions, &triangles, &bad_clearance),
        Err(ContactError::InvalidPolicy {
            field: "contact.clearance",
            ..
        })
    ));
    let too_clear = ContactPolicy {
        clearance: 1.0,
        max_displacement: 0.5,
        ..policy(ContactMode::Separate)
    };
    assert!(matches!(
        separate(&positions, &triangles, &too_clear),
        Err(ContactError::InvalidPolicy { .. })
    ));
    let no_pairs = ContactPolicy {
        max_pairs: 0,
        ..policy(ContactMode::Separate)
    };
    assert!(matches!(
        separate(&positions, &triangles, &no_pairs),
        Err(ContactError::InvalidPolicy {
            field: "contact.max_pairs",
            ..
        })
    ));
}

#[test]
fn a_contact_free_but_invalid_surface_is_rejected() {
    let clean = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    [f32::NAN, f32::INFINITY].into_iter().for_each(|value| {
        let mut positions = clean;
        positions[0][0] = value;
        assert_eq!(
            separate(&positions, &[[0, 1, 2]], &policy(ContactMode::Separate)),
            Err(ContactError::InvalidInput)
        );
    });
    assert_eq!(
        separate(&clean, &[[0, 0, 2]], &policy(ContactMode::Separate)),
        Err(ContactError::InvalidInput)
    );
}

#[test]
fn coplanar_overlap_separates_without_an_input_triggered_panic() {
    let positions = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.2, 0.2, 0.0],
        [0.4, 0.2, 0.0],
        [0.2, 0.4, 0.0],
    ];
    let triangles = [[0, 1, 2], [3, 4, 5]];
    assert_eq!(check::self_intersection_pairs(&positions, &triangles), 1);
    let result = separate(&positions, &triangles, &policy(ContactMode::Separate))
        .expect("a plane normal supplies a separation axis")
        .expect("the overlap needs separation");
    assert!(check::self_intersection_witnesses(&result.positions, &triangles).is_empty());
}
