//! The neutral rig graph: roots, ordering, rest composition and refusals.
//!
//! Every expectation here is hand-computed, so a change in the composition rule
//! fails a number rather than a snapshot.

use std::f32::consts::FRAC_1_SQRT_2;

use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigError, RigFrame, TransformDefect};

const FRAME: RigFrame = RigFrame::SourceZUp;
const IDENTITY_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn rest(translation: [f32; 3], rotation: [f32; 4], scale: f32) -> LocalTransform {
    LocalTransform {
        translation,
        rotation,
        scale,
    }
}

fn named(name: &str, parent: Option<JointId>) -> Joint {
    Joint {
        name: Some(name.to_owned()),
        parent,
        rest: LocalTransform::default(),
    }
}

fn unnamed(parent: Option<JointId>) -> Joint {
    Joint {
        name: None,
        parent,
        rest: LocalTransform::default(),
    }
}

fn translation_of(world: [[f64; 4]; 4]) -> [f64; 3] {
    [world[3][0], world[3][1], world[3][2]]
}

fn basis_of(world: [[f64; 4]; 4], column: usize) -> [f64; 3] {
    [world[column][0], world[column][1], world[column][2]]
}

fn length(vector: [f64; 3]) -> f64 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

fn close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "{what}: {actual} != {expected}"
    );
}

fn assert_translation(world: [[f64; 4]; 4], expected: [f64; 3]) {
    let actual = translation_of(world);
    for axis in 0..3 {
        close(actual[axis], expected[axis], "translation axis");
    }
}

#[test]
fn roots_are_every_parentless_joint_and_order_is_parent_first() {
    let rig = Rig::new(
        vec![
            unnamed(None),
            named("pelvis", None),
            unnamed(Some(JointId::new(1))),
            unnamed(Some(JointId::new(2))),
        ],
        FRAME,
    )
    .expect("a two-root forest is a valid rig");

    assert_eq!(rig.roots(), &[JointId::new(0), JointId::new(1)]);
    assert_eq!(rig.joints().len(), 4);

    let mut placed = vec![false; rig.joints().len()];
    for id in rig.topological_order() {
        if let Some(parent) = rig.joint(*id).expect("every ordered id is a joint").parent {
            assert!(placed[parent.index()], "parent {parent} must precede {id}");
        }
        placed[id.index()] = true;
    }

    let mut sorted = rig.topological_order().to_vec();
    sorted.sort();
    assert_eq!(
        sorted,
        (0..4_u32).map(JointId::new).collect::<Vec<_>>(),
        "the order names every joint exactly once"
    );
}

#[test]
fn an_empty_graph_is_refused() {
    assert_eq!(Rig::new(Vec::new(), FRAME), Err(RigError::EmptyGraph));
}

#[test]
fn a_rotated_chain_composes_to_hand_computed_worlds() {
    // Root translates +X; the child adds +Y, turns a quarter turn about +Z; the
    // grandchild adds +X in the turned frame.
    let rig = Rig::new(
        vec![
            Joint {
                name: None,
                parent: None,
                rest: rest([1.0, 0.0, 0.0], IDENTITY_ROTATION, 1.0),
            },
            Joint {
                name: None,
                parent: Some(JointId::new(0)),
                rest: rest(
                    [0.0, 2.0, 0.0],
                    [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
                    1.0,
                ),
            },
            Joint {
                name: None,
                parent: Some(JointId::new(1)),
                rest: rest([3.0, 0.0, 0.0], IDENTITY_ROTATION, 1.0),
            },
        ],
        FRAME,
    )
    .expect("the chain is a valid rig");

    let root = rig.rest_world(JointId::new(0)).expect("root world");
    assert_translation(root, [1.0, 0.0, 0.0]);

    let middle = rig.rest_world(JointId::new(1)).expect("middle world");
    assert_translation(middle, [1.0, 2.0, 0.0]);
    // Rz(90) maps local +X to +Y and local +Y to -X.
    for (axis, column) in [([0.0, 1.0, 0.0], 0_usize), ([-1.0, 0.0, 0.0], 1)] {
        let actual = basis_of(middle, column);
        for (lane, expected) in actual.iter().zip(axis) {
            close(*lane, expected, "quarter turn basis");
        }
    }

    let leaf = rig.rest_world(JointId::new(2)).expect("leaf world");
    assert_translation(leaf, [1.0, 5.0, 0.0]);
    close(length(basis_of(leaf, 0)), 1.0, "leaf scale");
}

#[test]
fn a_parent_scale_multiplies_child_translation() {
    // Root scale 2, child +X, grandchild +Y with scale 0.5.
    let rig = Rig::new(
        vec![
            Joint {
                name: None,
                parent: None,
                rest: rest([0.0, 0.0, 0.0], IDENTITY_ROTATION, 2.0),
            },
            Joint {
                name: None,
                parent: Some(JointId::new(0)),
                rest: rest([1.0, 0.0, 0.0], IDENTITY_ROTATION, 1.0),
            },
            Joint {
                name: None,
                parent: Some(JointId::new(1)),
                rest: rest([0.0, 1.0, 0.0], IDENTITY_ROTATION, 0.5),
            },
        ],
        FRAME,
    )
    .expect("the scaled chain is a valid rig");

    let child = rig.rest_world(JointId::new(1)).expect("child world");
    assert_translation(child, [2.0, 0.0, 0.0]);
    close(
        length(basis_of(child, 0)),
        2.0,
        "child accumulates the root scale",
    );

    let leaf = rig.rest_world(JointId::new(2)).expect("leaf world");
    assert_translation(leaf, [2.0, 2.0, 0.0]);
    close(
        length(basis_of(leaf, 0)),
        1.0,
        "scales multiply along the chain",
    );
}

#[test]
fn a_scaled_rotated_root_places_a_child_in_its_own_frame() {
    // Root at (1, 2, 3), a quarter turn about +Z, uniform scale 2; the child adds
    // +X in that frame and halves the scale.
    let rig = Rig::new(
        vec![
            Joint {
                name: None,
                parent: None,
                rest: rest(
                    [1.0, 2.0, 3.0],
                    [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
                    2.0,
                ),
            },
            Joint {
                name: None,
                parent: Some(JointId::new(0)),
                rest: rest([1.0, 0.0, 0.0], IDENTITY_ROTATION, 0.5),
            },
        ],
        FRAME,
    )
    .expect("the scaled, rotated chain is a valid rig");

    let child = rig.rest_world(JointId::new(1)).expect("child world");
    assert_translation(child, [1.0, 4.0, 3.0]);
    // The child inherits the root rotation, so its own +X points along +Y.
    let turned = basis_of(child, 0);
    close(turned[0], 0.0, "inherited quarter turn");
    close(turned[1], 1.0, "inherited quarter turn");
    close(length(turned), 1.0, "scales multiply along the chain");
}

#[test]
fn local_matrix_is_translate_rotate_scale() {
    let local = rest(
        [1.0, 2.0, 3.0],
        [0.0, 0.0, FRAC_1_SQRT_2, FRAC_1_SQRT_2],
        2.0,
    );
    let matrix = local.matrix().expect("a valid transform");
    assert_translation(matrix, [1.0, 2.0, 3.0]);
    close(
        length(basis_of(matrix, 0)),
        2.0,
        "scale applies before rotation",
    );
    let turned = basis_of(matrix, 0);
    close(turned[0], 0.0, "local +X turns into +Y");
    close(turned[1], 2.0, "local +X turns into +Y");
}

#[test]
fn unnamed_ancestors_are_retained_and_never_named() {
    let rig = Rig::new(
        vec![unnamed(None), named("pelvis", Some(JointId::new(0)))],
        FRAME,
    )
    .expect("an unnamed root is valid source data");

    assert!(
        rig.joint(JointId::new(0)).expect("root").name.is_none(),
        "an unnamed node stays unnamed"
    );
    assert_eq!(
        rig.joint_by_name("pelvis"),
        Ok(JointId::new(1)),
        "a named joint resolves exactly"
    );
    assert_eq!(
        rig.joint_by_name(""),
        Err(RigError::UnknownJointName {
            name: String::new()
        }),
        "the empty query matches no unnamed joint"
    );
}

#[test]
fn blank_names_are_refused() {
    for blank in ["", " ", "  \t "] {
        let error = Rig::new(vec![named(blank, None)], FRAME).expect_err("a blank name is invalid");
        assert_eq!(
            error,
            RigError::EmptyJointName {
                joint: JointId::new(0)
            }
        );
    }
}

#[test]
fn invalid_parents_are_named() {
    assert_eq!(
        Rig::new(vec![unnamed(None), unnamed(Some(JointId::new(7)))], FRAME),
        Err(RigError::ParentOutOfRange {
            joint: JointId::new(1),
            parent: JointId::new(7),
            joints: 2,
        })
    );
    assert_eq!(
        Rig::new(vec![unnamed(Some(JointId::new(0)))], FRAME),
        Err(RigError::SelfParent {
            joint: JointId::new(0)
        })
    );
}

#[test]
fn parent_cycles_are_refused() {
    let pair = Rig::new(
        vec![
            unnamed(Some(JointId::new(1))),
            unnamed(Some(JointId::new(0))),
        ],
        FRAME,
    );
    assert!(
        matches!(&pair, Err(RigError::Cycle { .. })),
        "a two-joint cycle must be refused, got {pair:?}"
    );

    let ring = Rig::new(
        vec![
            unnamed(Some(JointId::new(2))),
            unnamed(Some(JointId::new(0))),
            unnamed(Some(JointId::new(1))),
        ],
        FRAME,
    );
    assert!(
        matches!(&ring, Err(RigError::Cycle { .. })),
        "a three-joint ring must be refused, got {ring:?}"
    );
}

#[test]
fn non_finite_and_non_positive_transforms_are_refused() {
    let cases = [
        (
            rest([f32::NAN, 0.0, 0.0], IDENTITY_ROTATION, 1.0),
            TransformDefect::NonFiniteTranslation,
        ),
        (
            rest([0.0; 3], [0.0, 0.0, f32::NAN, 1.0], 1.0),
            TransformDefect::NonFiniteRotation,
        ),
        (
            rest([0.0; 3], IDENTITY_ROTATION, f32::INFINITY),
            TransformDefect::NonFiniteScale,
        ),
        (
            rest([0.0; 3], IDENTITY_ROTATION, 0.0),
            TransformDefect::NonPositiveScale { scale: 0.0 },
        ),
        (
            rest([0.0; 3], IDENTITY_ROTATION, -1.0),
            TransformDefect::NonPositiveScale { scale: -1.0 },
        ),
    ];
    for (transform, expected) in cases {
        assert_eq!(transform.validate(), Err(expected), "defect {expected:?}");
        let joint = Joint {
            name: None,
            parent: None,
            rest: transform,
        };
        assert_eq!(
            Rig::new(vec![joint], FRAME),
            Err(RigError::Transform {
                joint: JointId::new(0),
                defect: expected,
            })
        );
    }
}

#[test]
fn degenerate_and_non_unit_rotations_are_refused() {
    for rotation in [
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 2.0],
        [0.0, 0.0, 0.0, 1.008],
    ] {
        let transform = rest([0.0; 3], rotation, 1.0);
        assert!(
            matches!(
                transform.validate(),
                Err(TransformDefect::RotationNotUnit { .. })
            ),
            "rotation {rotation:?} must be refused"
        );
    }
    let rounding = rest([0.0; 3], [0.0, 0.0, 0.0, 1.000_02], 1.0);
    assert_eq!(
        rounding.validate(),
        Ok(()),
        "f32 rounding inside the tolerance is accepted"
    );
}

#[test]
fn name_lookup_is_exact_and_refuses_ambiguity() {
    let rig = Rig::new(
        vec![
            named("spine", None),
            named("spine", None),
            named("Spine", None),
        ],
        FRAME,
    )
    .expect("duplicate names are data, not a graph defect");

    assert_eq!(
        rig.joint_by_name("spine"),
        Err(RigError::AmbiguousJointName {
            name: "spine".to_owned(),
            matches: 2,
        })
    );
    assert_eq!(
        rig.joint_by_name("Spine"),
        Ok(JointId::new(2)),
        "lookup is case sensitive"
    );
    assert_eq!(
        rig.joint_by_name("spinex"),
        Err(RigError::UnknownJointName {
            name: "spinex".to_owned()
        })
    );
}

#[test]
fn is_ancestor_is_strict_and_refuses_unknown_ids() {
    let rig = Rig::new(
        vec![
            unnamed(None),
            unnamed(Some(JointId::new(0))),
            unnamed(Some(JointId::new(1))),
        ],
        FRAME,
    )
    .expect("the chain is a valid rig");

    let (root, middle, leaf) = (JointId::new(0), JointId::new(1), JointId::new(2));
    assert_eq!(rig.is_ancestor(root, leaf), Ok(true));
    assert_eq!(rig.is_ancestor(middle, leaf), Ok(true));
    assert_eq!(rig.is_ancestor(root, middle), Ok(true));
    assert_eq!(rig.is_ancestor(leaf, root), Ok(false));
    assert_eq!(
        rig.is_ancestor(root, root),
        Ok(false),
        "not its own ancestor"
    );
    assert_eq!(rig.is_ancestor(leaf, leaf), Ok(false));

    let outside = JointId::new(9);
    assert_eq!(
        rig.is_ancestor(outside, root),
        Err(RigError::UnknownJoint {
            joint: outside,
            joints: 3,
        })
    );
    assert_eq!(
        rig.is_ancestor(root, outside),
        Err(RigError::UnknownJoint {
            joint: outside,
            joints: 3,
        })
    );
    assert_eq!(
        rig.rest_world(outside),
        Err(RigError::UnknownJoint {
            joint: outside,
            joints: 3,
        })
    );
    assert!(rig.joint(outside).is_none());
}

#[test]
fn the_single_identity_root_has_an_identity_rest_world() {
    let rig = Rig::new(vec![unnamed(None)], FRAME).expect("a one-joint rig is valid");
    let world = rig.rest_world(JointId::new(0)).expect("root world");
    for (column, column_values) in world.iter().enumerate() {
        for (row, value) in column_values.iter().enumerate() {
            let expected = f64::from(u8::from(column == row));
            close(*value, expected, "identity entry");
        }
    }
}

#[test]
fn the_frame_is_declared_and_never_converted() {
    let joints = || {
        vec![
            Joint {
                name: None,
                parent: None,
                rest: rest([0.0, 1.0, 2.0], IDENTITY_ROTATION, 1.0),
            },
            unnamed(Some(JointId::new(0))),
        ]
    };
    let source = Rig::new(joints(), RigFrame::SourceZUp).expect("source rig");
    let renderer = Rig::new(joints(), RigFrame::YUp).expect("renderer rig");

    assert_eq!(source.frame(), RigFrame::SourceZUp);
    assert_eq!(renderer.frame(), RigFrame::YUp);
    assert_ne!(source.frame(), renderer.frame());
    assert_eq!(
        source.rest_world(JointId::new(1)),
        renderer.rest_world(JointId::new(1)),
        "the declared frame is metadata, not a conversion"
    );
}

#[test]
fn a_long_chain_builds_without_recursion() {
    const LENGTH: usize = 65_536;
    const STEP: f32 = 0.001;
    let joints: Vec<Joint> = (0..LENGTH)
        .map(|index| Joint {
            name: None,
            parent: match index {
                0 => None,
                previous => Some(JointId::new(
                    u32::try_from(previous - 1).expect("chain index"),
                )),
            },
            rest: rest([STEP, 0.0, 0.0], IDENTITY_ROTATION, 1.0),
        })
        .collect();

    let rig = Rig::new(joints, FRAME).expect("a long chain is a valid rig");
    assert_eq!(rig.roots(), &[JointId::new(0)]);
    assert_eq!(rig.topological_order().len(), LENGTH);

    let last = JointId::new(u32::try_from(LENGTH - 1).expect("chain index"));
    let world = rig.rest_world(last).expect("last joint world");
    close(
        world[3][0],
        f64::from(STEP) * LENGTH as f64,
        "chain translation",
    );
    assert_eq!(rig.is_ancestor(JointId::new(0), last), Ok(true));
}
