use glam::{DMat4, DQuat, DVec3};
use sculpto_core::mesh::Skin;
use sculpto_core::rig::rest::{RestBindError, RestBinding, SkinRestDefect, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;

const FRAME: RigFrame = RigFrame::SourceZUp;
const ROOT: JointId = JointId::new(0);

fn matrix(value: DMat4) -> [[f32; 4]; 4] {
    value
        .to_cols_array_2d()
        .map(|column| column.map(|x| x as f32))
}

fn named(name: &str, parent: Option<JointId>, rest: LocalTransform) -> Joint {
    Joint {
        name: Some(name.into()),
        parent,
        rest,
    }
}

fn source<'a>(
    shape: usize,
    positions: &'a [[f32; 3]],
    skin: &'a Skin,
    global: DMat4,
) -> SkinRestSource<'a> {
    SkinRestSource {
        shape,
        positions,
        skin,
        root: ROOT,
        global_to_skin: matrix(global),
    }
}

fn skin(bones: &[&str], binds: &[DMat4], joint: u16) -> Skin {
    Skin {
        bones: bones.iter().map(|name| (*name).into()).collect(),
        bind_inverse: binds.iter().copied().map(matrix).collect(),
        influences: SkinWeights::from_quads(&[[joint, 0, 0, 0]], &[[0.9, 0.0, 0.0, 0.0]])
            .expect("a positive source row"),
    }
}

#[test]
fn rest_uses_skin_to_bone_before_the_joint_world() {
    let world = DMat4::from_rotation_translation(
        DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2),
        DVec3::new(2.0, 0.0, 0.0),
    );
    let global = DMat4::from_translation(DVec3::NEG_Y);
    let rig = Rig::new(
        vec![named(
            "root",
            None,
            LocalTransform {
                rotation: [
                    0.0,
                    0.0,
                    std::f32::consts::FRAC_1_SQRT_2,
                    std::f32::consts::FRAC_1_SQRT_2,
                ],
                translation: [2.0, 0.0, 0.0],
                scale: 1.0,
            },
        )],
        FRAME,
    )
    .expect("valid root");
    let skin = skin(&["root"], &[world.inverse() * global.inverse()], 0);
    let point = [[2.0, 1.0, 1.0]];
    let binding = RestBinding::prove(&rig, FRAME, &[source(0, &point, &skin, global)])
        .expect("authored transforms agree");
    assert_eq!(binding.basis(), DMat4::IDENTITY.to_cols_array_2d());
    assert_eq!(binding.skins()[0].palette(), &[ROOT]);
    assert!(binding.skins()[0].measurement().max_vertex < 1.0e-6);
    assert_eq!(
        binding.skins()[0].source().skin.influences.row_sum(0),
        Some(0.9)
    );
}

#[test]
fn the_candidate_basis_must_work_for_every_shape() {
    let rig =
        Rig::new(vec![named("root", None, LocalTransform::default())], FRAME).expect("valid root");
    let skin = skin(&["root"], &[DMat4::IDENTITY], 0);
    let point = [[2.0, 1.0, 1.0]];
    let global = DMat4::from_translation(DVec3::NEG_Z);
    let shapes = [
        source(0, &point, &skin, global),
        source(4, &point, &skin, global),
    ];
    let binding = RestBinding::prove(&rig, FRAME, &shapes).expect("one common correction");
    assert_eq!(binding.basis(), global.inverse().to_cols_array_2d());
    assert_eq!(binding.skins()[1].source().shape, 4);

    let conflicting = [shapes[0], source(4, &point, &skin, DMat4::IDENTITY)];
    assert_eq!(
        RestBinding::prove(&rig, FRAME, &conflicting).expect_err("separate bases are inconsistent"),
        RestBindError::Shape {
            shape: 4,
            defect: SkinRestDefect::VertexRest {
                vertex: 0,
                error: 1.0
            },
        },
    );
}

#[test]
fn a_small_joint_error_can_still_fail_on_a_real_vertex() {
    let rig = Rig::new(
        vec![
            named("root", None, LocalTransform::default()),
            named(
                "child",
                Some(ROOT),
                LocalTransform {
                    scale: 1.00001,
                    ..LocalTransform::default()
                },
            ),
        ],
        FRAME,
    )
    .expect("valid hierarchy");
    let skin = skin(&["root", "child"], &[DMat4::IDENTITY; 2], 1);
    let point = [[1000.0, 0.0, 0.0]];
    assert!(matches!(
        RestBinding::prove(&rig, FRAME, &[source(0, &point, &skin, DMat4::IDENTITY)]),
        Err(RestBindError::Shape {
            shape: 0,
            defect: SkinRestDefect::VertexRest { vertex: 0, error },
        }) if error > 0.009
    ));
}

#[test]
fn opposing_bad_binds_cannot_cancel_each_other_at_rest() {
    let rig = Rig::new(
        vec![
            named("root", None, LocalTransform::default()),
            named("left", Some(ROOT), LocalTransform::default()),
            named("right", Some(ROOT), LocalTransform::default()),
        ],
        FRAME,
    )
    .expect("valid hierarchy");
    let mut skin = skin(
        &["root", "left", "right"],
        &[
            DMat4::IDENTITY,
            DMat4::from_translation(DVec3::X),
            DMat4::from_translation(DVec3::NEG_X),
        ],
        0,
    );
    skin.influences = SkinWeights::from_quads(&[[1, 2, 0, 0]], &[[0.5, 0.5, 0.0, 0.0]])
        .expect("two equally weighted joints");
    assert_eq!(
        RestBinding::prove(
            &rig,
            FRAME,
            &[source(0, &[[0.0; 3]], &skin, DMat4::IDENTITY)]
        )
        .expect_err("cancellation must not hide the wrong individual binds"),
        RestBindError::Shape {
            shape: 0,
            defect: SkinRestDefect::InfluenceRest {
                vertex: 0,
                bone: 1,
                error: 0.5
            },
        },
    );
}

#[test]
fn missing_or_singular_binds_are_refused() {
    let rig =
        Rig::new(vec![named("root", None, LocalTransform::default())], FRAME).expect("valid root");
    let point = [[1.0, 0.0, 0.0]];
    for (binds, expected) in [
        (&[][..], SkinRestDefect::BindCount { bones: 1, binds: 0 }),
        (&[DMat4::ZERO][..], SkinRestDefect::Bind { bone: 0 }),
    ] {
        let skin = skin(&["root"], binds, 0);
        assert_eq!(
            RestBinding::prove(&rig, FRAME, &[source(3, &point, &skin, DMat4::IDENTITY)])
                .expect_err("required authored bind is invalid"),
            RestBindError::Shape {
                shape: 3,
                defect: expected
            },
        );
    }
}
