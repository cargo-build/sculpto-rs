//! The multi-shape bundle contract: body, hands and feet travel together only
//! when names are unique, palettes are sound and every skinned shape repeats
//! one bone list in the same order.

use sculpto_core::mesh::{
    BundleError, MeshAsset, MeshError, PartitionId, Skin, Transform, validate_bundle,
};
use sculpto_core::role::{ShapeRole, validate_required_roles};
use sculpto_core::skin::SkinWeights;

/// The shared skeleton every bundled shape repeats, in bind order.
const SKELETON: [&str; 6] = ["root", "spine", "hand_l", "hand_r", "foot_l", "foot_r"];

/// One skinned triangle weighted to the first bone; the palette gate derives
/// an exact one-bone palette from it.
fn skinned_shape(name: &str, bones: &[&str]) -> MeshAsset {
    MeshAsset {
        name: name.into(),
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        triangle_partitions: vec![PartitionId(0)],
        skin: Some(Skin {
            bones: bones.iter().map(|bone| (*bone).into()).collect(),
            bind_inverse: Vec::new(),
            influences: SkinWeights::from_quads(&[[0, 0, 0, 0]; 3], &[[1.0, 0.0, 0.0, 0.0]; 3])
                .expect("one influence per vertex"),
        }),
        ..MeshAsset::default()
    }
}

/// Body, hands and feet over one skeleton, the hands and feet placed by a
/// local transform.
fn body_hands_feet() -> Vec<MeshAsset> {
    let mut shapes = vec![
        skinned_shape("body", &SKELETON),
        skinned_shape("hands", &SKELETON),
        skinned_shape("feet", &SKELETON),
    ];
    shapes[1].transform = Some(Transform {
        translation: [0.0, 0.0, 1.0],
        ..Transform::default()
    });
    shapes[2].transform = Some(Transform {
        translation: [0.0, 0.0, -1.0],
        scale: 0.5,
        ..Transform::default()
    });
    shapes
}

#[test]
fn valid_body_hands_feet_bundle_passes() {
    let shapes = body_hands_feet();
    for shape in &shapes {
        shape
            .validate()
            .expect("each fixture shape is a valid model");
    }
    validate_bundle(&shapes).expect("one skeleton, unique names, sound palettes");
}

#[test]
fn single_shape_bundle_passes() {
    let shapes = vec![skinned_shape("body", &SKELETON)];
    validate_bundle(&shapes).expect("a one-shape bundle is still a bundle");
}

#[test]
fn empty_bundle_rejected() {
    assert_eq!(validate_bundle(&[]), Err(BundleError::Empty));
}

#[test]
fn duplicate_name_rejected() {
    let mut shapes = body_hands_feet();
    shapes[1].name = "body".into();
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::DuplicateName {
            shape: 1,
            name: "body".into(),
        })
    );
}

#[test]
fn mismatched_bone_order_rejected() {
    let mut shapes = body_hands_feet();
    shapes[2].skin.as_mut().expect("skin").bones = vec![
        "spine".into(),
        "root".into(),
        "hand_l".into(),
        "hand_r".into(),
        "foot_l".into(),
        "foot_r".into(),
    ];
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::BoneListMismatch { shape: 2 })
    );
}

#[test]
fn mismatched_bone_name_rejected() {
    let mut shapes = body_hands_feet();
    shapes[2].skin.as_mut().expect("skin").bones =
        vec!["root".into(), "spine".into(), "tail".into()];
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::BoneListMismatch { shape: 2 })
    );
}

#[test]
fn first_skinned_shape_sets_the_bone_reference() {
    let mut unskinned = skinned_shape("cloth", &SKELETON);
    unskinned.skin = None;
    let shapes = vec![
        unskinned,
        skinned_shape("body", &["root", "spine"]),
        skinned_shape("feet", &["root"]),
    ];
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::BoneListMismatch { shape: 2 })
    );
}

#[test]
fn unskinned_shapes_carry_no_bone_constraint() {
    let mut cloth = skinned_shape("cloth", &SKELETON);
    cloth.skin = None;
    let shapes = vec![skinned_shape("body", &SKELETON), cloth];
    validate_bundle(&shapes).expect("an unskinned shape joins a skinned bundle");

    let mut bare = skinned_shape("bare", &SKELETON);
    bare.skin = None;
    validate_bundle(&[bare]).expect("a bundle without skins has no bone reference");
}

#[test]
fn invalid_palettes_rejected() {
    let mut broken = skinned_shape("hands", &SKELETON);
    // Three vertices but two influence rows: the derived palette cannot cover
    // the triangle, so the palette owner refuses the shape.
    broken.skin.as_mut().expect("skin").influences =
        SkinWeights::from_quads(&[[0, 0, 0, 0]; 2], &[[1.0, 0.0, 0.0, 0.0]; 2])
            .expect("two four-lane rows");
    let shapes = vec![skinned_shape("body", &SKELETON), broken];
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::Palettes {
            shape: 1,
            error: MeshError::SkinLengthMismatch,
        })
    );
}

#[test]
fn duplicate_name_is_reported_before_bone_mismatch() {
    let mut shapes = body_hands_feet();
    shapes[1].name = "body".into();
    shapes[2].skin.as_mut().expect("skin").bones = vec!["root".into()];
    assert_eq!(
        validate_bundle(&shapes),
        Err(BundleError::DuplicateName {
            shape: 1,
            name: "body".into(),
        })
    );
}

#[test]
fn transform_default_is_identity_and_optional() {
    let identity = Transform::default();
    assert_eq!(identity.translation, [0.0; 3]);
    assert_eq!(
        identity.rotation,
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    );
    assert_eq!(identity.scale, 1.0);
    assert_eq!(MeshAsset::default().transform, None);
}

/// Body, hands, feet and a head over one skeleton: the complete character the
/// required-part gate expects.
fn body_hands_feet_head() -> Vec<(ShapeRole, MeshAsset)> {
    let mut shapes = body_hands_feet();
    shapes.push(skinned_shape("head", &SKELETON));
    vec![
        (ShapeRole::Body, shapes[0].clone()),
        (ShapeRole::Hands, shapes[1].clone()),
        (ShapeRole::Feet, shapes[2].clone()),
        (ShapeRole::Head, shapes[3].clone()),
    ]
}

#[test]
fn role_assignment_complete_character_passes_both_gates() {
    let assigned = body_hands_feet_head();
    let roles: Vec<ShapeRole> = assigned.iter().map(|(role, _)| *role).collect();
    validate_required_roles(&roles).expect("body, hands, feet and head are all present");
    let meshes: Vec<MeshAsset> = assigned.into_iter().map(|(_, mesh)| mesh).collect();
    validate_bundle(&meshes).expect("one skeleton, unique names, sound palettes");
}

#[test]
fn role_assignment_missing_head_is_named() {
    let assigned = body_hands_feet_head();
    let roles: Vec<ShapeRole> = assigned
        .iter()
        .filter(|(role, _)| *role != ShapeRole::Head)
        .map(|(role, _)| *role)
        .collect();
    assert_eq!(
        validate_required_roles(&roles),
        Err(sculpto_core::role::RoleError::MissingRoles(vec![
            ShapeRole::Head
        ]))
    );
}

#[test]
fn role_assignment_missing_parts_are_all_named() {
    // Only the body survives: the gate must name hands, feet and head together.
    assert_eq!(
        validate_required_roles(&[ShapeRole::Body]),
        Err(sculpto_core::role::RoleError::MissingRoles(vec![
            ShapeRole::Hands,
            ShapeRole::Feet,
            ShapeRole::Head,
        ]))
    );
}

#[test]
fn role_assignment_stamps_each_parts_default_partition() {
    for (role, mut mesh) in body_hands_feet_head() {
        let partition = role.default_partition();
        mesh.triangle_partitions = vec![partition; mesh.triangles.len()];
        assert!(
            mesh.triangle_partitions.iter().all(|p| *p == partition),
            "{role} keeps its default partition"
        );
        mesh.validate()
            .expect("stamping a partition keeps the model valid");
    }
    assert_eq!(ShapeRole::Head.default_partition(), PartitionId(30));
    assert_eq!(ShapeRole::Feet.default_partition(), PartitionId(37));
}

#[test]
fn role_assignment_garment_is_optional() {
    let mut assigned = body_hands_feet_head();
    assigned.push((ShapeRole::Garment, skinned_shape("robe", &SKELETON)));
    let roles: Vec<ShapeRole> = assigned.iter().map(|(role, _)| *role).collect();
    validate_required_roles(&roles).expect("a garment does not break the required contract");
}
