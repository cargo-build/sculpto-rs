//! `actor tpose` end-to-end on a synthetic pair: an A-posed body is re-aimed onto
//! a reference's T-pose rest, its own forward axis decides the side, and the
//! written actor keeps its topology, its weights and no clip. @req DECL0013

use std::path::PathBuf;

use glam::{DMat4, DQuat};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::adapt::actor_rig;
use sculpto_engine::rest::{TPoseParams, run};
use sculpto_gltf::{PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::Actor;

const IDENTITY_F32: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn joint(name: &str, parent: Option<u32>, translation: [f32; 3]) -> Joint {
    Joint {
        name: Some(name.into()),
        parent: parent.map(JointId::new),
        rest: LocalTransform {
            translation,
            ..Default::default()
        },
    }
}

fn f32_of(value: DMat4) -> [[f32; 4]; 4] {
    value
        .to_cols_array_2d()
        .map(|column| column.map(|x| x as f32))
}

/// A rigged GLB in world Y-up with an identity basis, one shared pool, one part.
fn actor_glb(
    name: &str,
    rig: &Rig,
    palette: &[&str],
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    rows: &[(&[u16], &[f32])],
    color: [f32; 4],
) -> Vec<u8> {
    let mut offsets = vec![0_u32];
    let mut joints = Vec::new();
    let mut values = Vec::new();
    for (slots, weights) in rows {
        joints.extend_from_slice(slots);
        values.extend_from_slice(weights);
        offsets.push(joints.len() as u32);
    }
    let skin = Skin {
        bones: palette.iter().map(|bone| (*bone).into()).collect(),
        bind_inverse: palette
            .iter()
            .map(|bone| {
                let joint = rig.joint_by_name(bone).expect("palette bone");
                f32_of(
                    DMat4::from_cols_array_2d(&rig.rest_world(joint).expect("rest world"))
                        .inverse(),
                )
            })
            .collect(),
        influences: SkinWeights::new(offsets, joints, values, palette.len()).expect("rows"),
    };
    let binding = RestBinding::prove_with_basis(
        rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: rig.roots()[0],
            positions,
            skin: &skin,
            global_to_skin: IDENTITY_F32,
        }],
        DMat4::IDENTITY.to_cols_array_2d(),
    )
    .expect("a checked rest");
    let asset = MeshAsset {
        name: name.into(),
        positions: positions.to_vec(),
        triangles: triangles.to_vec(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let materials = [PartMaterial {
        partition: 0,
        name: "body".into(),
        color,
    }];
    write_partitioned_rigged_glb(&asset, &binding, 0, &[], &materials).expect("a written actor")
}

/// An A-posed body: its left side is `−x` and its toes point `−z`, the opposite
/// way round from the Mixamo reference below. That is the measured layout of the
/// real actors, and it is what the verb must carry the target directions through.
fn a_pose_body() -> Rig {
    let mut joints = vec![
        joint("NPC Pelvis [Pelv]", None, [0.0, 1.0, 0.0]),
        joint("NPC Spine2 [Spn2]", Some(0), [0.0, 0.45, 0.0]),
        joint("NPC Head [Head]", Some(1), [0.0, 0.28, 0.0]),
    ];
    for (suffix, side) in [("L", -1.0_f32), ("R", 1.0)] {
        let clavicle = joints.len() as u32;
        joints.push(joint(
            &format!("NPC {suffix} Clavicle [{suffix}Clv]"),
            Some(1),
            [side * 0.15, 0.06, 0.0],
        ));
        let arm = joints.len() as u32;
        joints.push(joint(
            &format!("NPC {suffix} UpperArm [{suffix}Uar]"),
            Some(clavicle),
            [side * 0.12, -0.05, 0.0],
        ));
        let forearm = joints.len() as u32;
        joints.push(joint(
            &format!("NPC {suffix} Forearm [{suffix}Lar]"),
            Some(arm),
            [side * 0.02, -0.24, 0.0],
        ));
        let hand = joints.len() as u32;
        joints.push(joint(
            &format!("NPC {suffix} Hand [{suffix}Hnd]"),
            Some(forearm),
            [0.0, -0.10, 0.0],
        ));
        joints.push(joint(
            &format!("NPC {suffix} Finger20 [{suffix}F20]"),
            Some(hand),
            [0.0, -0.08, 0.0],
        ));
        // The three roots that span the palm plane: thumb forward, index and
        // pinky either side of the limb, so the plane is defined.
        for (digit, translation) in [
            ("00", [0.0, -0.03, 0.05]),
            ("10", [0.0, -0.06, 0.05]),
            ("40", [0.0, -0.06, -0.05]),
        ] {
            joints.push(joint(
                &format!("NPC {suffix} Finger{digit} [{suffix}F{digit}]"),
                Some(hand),
                translation,
            ));
        }
        let foot = joints.len() as u32;
        joints.push(joint(
            &format!("NPC {suffix} Foot [{suffix}Foot]"),
            Some(0),
            [side * 0.08, -0.95, 0.0],
        ));
        joints.push(joint(
            &format!("NPC {suffix} Toe0 [{suffix}Toe0]"),
            Some(foot),
            [0.0, -0.02, -0.12],
        ));
    }
    Rig::new(joints, RigFrame::YUp).expect("the A-posed body forms a rig")
}

/// The reference's T-pose rest: left side `+x`, toes `+z`, arms horizontal.
fn t_pose_reference() -> Rig {
    let mut joints = vec![
        joint("mixamorig:Hips", None, [0.0, 1.0, 0.0]),
        joint("mixamorig:Spine2", Some(0), [0.0, 0.45, 0.0]),
        joint("mixamorig:Head", Some(1), [0.0, 0.28, 0.0]),
    ];
    for (suffix, side) in [("Left", 1.0_f32), ("Right", -1.0)] {
        let shoulder = joints.len() as u32;
        joints.push(joint(
            &format!("mixamorig:{suffix}Shoulder"),
            Some(1),
            [side * 0.15, 0.06, 0.0],
        ));
        let arm = joints.len() as u32;
        joints.push(joint(
            &format!("mixamorig:{suffix}Arm"),
            Some(shoulder),
            [side * 0.25, 0.0, 0.0],
        ));
        let forearm = joints.len() as u32;
        joints.push(joint(
            &format!("mixamorig:{suffix}ForeArm"),
            Some(arm),
            [side * 0.25, 0.0, 0.0],
        ));
        let hand = joints.len() as u32;
        joints.push(joint(
            &format!("mixamorig:{suffix}Hand"),
            Some(forearm),
            [side * 0.10, 0.0, 0.0],
        ));
        joints.push(joint(
            &format!("mixamorig:{suffix}HandMiddle1"),
            Some(hand),
            [side * 0.05, 0.0, 0.0],
        ));
        for (digit, translation) in [
            ("Thumb1", [side * 0.05, 0.0, 0.03]),
            ("Index1", [side * 0.06, 0.0, 0.02]),
            ("Pinky1", [side * 0.06, 0.0, -0.02]),
        ] {
            joints.push(joint(
                &format!("mixamorig:{suffix}Hand{digit}"),
                Some(hand),
                translation,
            ));
        }
        let foot = joints.len() as u32;
        joints.push(joint(
            &format!("mixamorig:{suffix}Foot"),
            Some(0),
            [side * 0.08, -0.95, 0.0],
        ));
        joints.push(joint(
            &format!("mixamorig:{suffix}ToeBase"),
            Some(foot),
            [0.0, -0.02, 0.12],
        ));
    }
    Rig::new(joints, RigFrame::YUp).expect("the reference forms a rig")
}

/// Rebuild a rig with every joint's world Y axis along its own bone.
///
/// Mixamo's skeleton is built that way — Y along the limb, Z down — and the verb
/// refuses a reference that is pure translation because no roll can be carried
/// out of one. The synthetic reference must carry the same convention, and the
/// world pivots must not move while it does: every bone vector is re-expressed in
/// its parent's new frame instead of being rotated with it.
fn y_along_bones(rig: &Rig) -> Rig {
    let count = rig.joints().len();
    let worlds: Vec<DMat4> = (0..count)
        .map(|slot| {
            DMat4::from_cols_array_2d(
                &rig.rest_world(JointId::new(slot as u32))
                    .expect("rest world"),
            )
        })
        .collect();
    let pivots: Vec<glam::DVec3> = worlds
        .iter()
        .map(|world| world.transform_point3(glam::DVec3::ZERO))
        .collect();
    let parents: Vec<Option<usize>> = (0..count)
        .map(|slot| {
            rig.joint(JointId::new(slot as u32))
                .and_then(|joint| joint.parent)
                .map(|parent| parent.index())
        })
        .collect();
    let first_child: Vec<Option<usize>> = {
        let mut children: Vec<Option<usize>> = vec![None; count];
        for (slot, parent) in parents.iter().enumerate() {
            if let Some(parent) = parent
                && children[*parent].is_none()
            {
                children[*parent] = Some(slot);
            }
        }
        children
    };
    // World rotation per joint: Y along its bone, or the parent's for a leaf.
    let mut rotations: Vec<DQuat> = vec![DQuat::IDENTITY; count];
    for slot in rig.topological_order().iter().map(|id| id.index()) {
        let rotation = match first_child[slot] {
            Some(child) => {
                let bone = (pivots[child] - pivots[slot]).normalize();
                DQuat::from_rotation_arc(glam::DVec3::Y, bone).normalize()
            }
            None => parents[slot]
                .map(|parent| rotations[parent])
                .unwrap_or(DQuat::IDENTITY),
        };
        rotations[slot] = rotation;
    }
    let joints: Vec<Joint> = (0..count)
        .map(|slot| {
            let joint = rig.joint(JointId::new(slot as u32)).expect("joint").clone();
            let parent_rotation = parents[slot]
                .map(|parent| rotations[parent])
                .unwrap_or(DQuat::IDENTITY);
            let translation = match parents[slot] {
                Some(parent) => parent_rotation.inverse() * (pivots[slot] - pivots[parent]),
                None => pivots[slot],
            };
            Joint {
                name: joint.name,
                parent: joint.parent,
                rest: LocalTransform {
                    translation: translation.to_array().map(|value| value as f32),
                    rotation: (parent_rotation.inverse() * rotations[slot])
                        .normalize()
                        .to_array()
                        .map(|value| value as f32),
                    scale: 1.0,
                },
            }
        })
        .collect();
    Rig::new(joints, RigFrame::YUp).expect("a Y-aligned reference")
}

fn palette(rig: &Rig) -> Vec<&str> {
    (0..rig.joints().len())
        .map(|slot| {
            rig.joint(JointId::new(slot as u32))
                .and_then(|joint| joint.name.as_deref())
                .expect("a named joint")
        })
        .collect()
}

/// Four triangles, one pair off each shoulder, so the bake has flesh to move.
fn body_positions() -> Vec<[f32; 3]> {
    vec![
        [-0.15, 1.51, 0.0],
        [-0.27, 1.46, 0.0],
        [-0.29, 1.22, 0.0],
        [-0.29, 1.12, 0.0],
        [0.0, 1.45, 0.06],
        [0.0, 1.30, 0.06],
        [0.15, 1.51, 0.0],
        [0.27, 1.46, 0.0],
        [0.29, 1.22, 0.0],
        [0.29, 1.12, 0.0],
        [0.0, 1.45, -0.06],
        [0.0, 1.30, -0.06],
    ]
}

/// One vertex per arm bone per side, plus two torso vertices: the pool must
/// weight every aimed bone or the verb refuses the aim as inert.
fn weight_rows(palette: &[&str]) -> Vec<(Vec<u16>, Vec<f32>)> {
    let slot = |name: &str| -> u16 {
        palette
            .iter()
            .position(|bone| *bone == name)
            .expect("palette bone") as u16
    };
    vec![
        (vec![slot("NPC L UpperArm [LUar]")], vec![1.0]),
        (vec![slot("NPC L Forearm [LLar]")], vec![1.0]),
        (vec![slot("NPC L Hand [LHnd]")], vec![1.0]),
        (vec![slot("NPC L Finger20 [LF20]")], vec![1.0]),
        (vec![slot("NPC Spine2 [Spn2]")], vec![1.0]),
        (vec![slot("NPC Spine2 [Spn2]")], vec![1.0]),
        (vec![slot("NPC R UpperArm [RUar]")], vec![1.0]),
        (vec![slot("NPC R Forearm [RLar]")], vec![1.0]),
        (vec![slot("NPC R Hand [RHnd]")], vec![1.0]),
        (vec![slot("NPC R Finger20 [RF20]")], vec![1.0]),
        (vec![slot("NPC Spine2 [Spn2]")], vec![1.0]),
        (vec![slot("NPC Spine2 [Spn2]")], vec![1.0]),
    ]
}

/// Write the body and the reference as the GLB pair the verb reads.
///
/// The reference carries the body's weight rows: the verb reads the reference's
/// rest geometry only, and one pool keeps the fixture a single object.
fn write_pair(label: &str, body: &Rig, reference: &Rig) -> (PathBuf, PathBuf) {
    let positions = body_positions();
    let triangles = [[0_u32, 1, 2], [3, 4, 5], [6, 7, 8], [9, 10, 11]];
    let body_palette = palette(body);
    let reference_palette = palette(reference);
    let rows = weight_rows(&body_palette);
    let rows: Vec<(&[u16], &[f32])> = rows
        .iter()
        .map(|(joints, weights)| (joints.as_slice(), weights.as_slice()))
        .collect();
    let actor_path = temp_path(&format!("{label}-actor"));
    let reference_path = temp_path(&format!("{label}-reference"));
    std::fs::write(
        &actor_path,
        actor_glb(
            "TestBody",
            body,
            &body_palette,
            &positions,
            &triangles,
            &rows,
            [0.9, 0.8, 0.7, 1.0],
        ),
    )
    .expect("write actor");
    std::fs::write(
        &reference_path,
        actor_glb(
            "ReferenceBody",
            reference,
            &reference_palette,
            &positions,
            &triangles,
            &rows,
            [0.5, 0.5, 0.5, 1.0],
        ),
    )
    .expect("write reference");
    (actor_path, reference_path)
}

fn temp_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!("sculpto-tpose-{}-{label}.glb", std::process::id()))
}

#[test]
fn the_arm_chain_is_re_aimed_onto_the_reference_t_pose() {
    let body = a_pose_body();
    let reference = y_along_bones(&t_pose_reference());
    let (actor_path, reference_path) = write_pair("aim", &body, &reference);
    let out_path = temp_path("out");

    let report = run(&TPoseParams {
        actor: actor_path,
        reference: reference_path,
        out: Some(out_path.clone()),
        report: None,
        pose: None,
        pose_clip: None,
        pose_time: None,
        pose_scan: false,
        pose_rest: None,
        straighten_arms: false,
    })
    .expect("the bake succeeds");

    // The bar this lane is graded on, from the report itself.
    assert_eq!(report["gates"]["arm_direction"]["pass"], true);
    assert!(
        report["gates"]["arm_direction"]["max_deviation_deg"]
            .as_f64()
            .unwrap()
            < 1e-6
    );
    assert_eq!(report["gates"]["bone_lengths"]["pass"], true);
    assert_eq!(report["gates"]["bone_axis"]["pass"], true);
    // The roll is the point of the second bake: the chain must receive a
    // non-zero, measured roll and the hand's plane must land on the reference's.
    assert_eq!(report["gates"]["roll"]["pass"], true);
    assert!(
        report["gates"]["roll"]["max_roll_residual_deg"]
            .as_f64()
            .unwrap()
            < 1e-6
    );
    let applied = report["gates"]["roll"]["sides"][0]["roll_applied_deg"]
        .as_f64()
        .unwrap();
    assert!(applied.abs() > 1.0, "a roll was applied: {applied}");
    assert_eq!(report["gates"]["no_crossing"]["pass"], true);
    assert_eq!(report["gates"]["weights"]["pass"], true);
    assert_eq!(report["gates"]["rest"]["pass"], true);
    assert_eq!(report["input"]["vertices"], 12);
    assert_eq!(report["input"]["triangles"], 4);
    assert_eq!(report["output"]["clips"], 0);

    // Both bodies' own forward axes decided the side: our left is `−x`, the
    // reference's is `+x`, and the baked left arm must end up lateral on ours.
    let yaw = report["bodies"]["yaw_deg"].as_f64().unwrap();
    assert!((yaw - 180.0).abs() < 1.0, "yaw {yaw}");
    let out_bytes = std::fs::read(&out_path).expect("read output");
    let out = Actor::from_glb(&out_bytes).expect("output actor");
    assert_eq!(out.clip_names().len(), 0);
    let (baked_rig, _) = actor_rig(&out).expect("baked rig");
    let direction = |joint: &str, child: &str| {
        let a = baked_rig.joint_by_name(joint).expect("joint");
        let b = baked_rig.joint_by_name(child).expect("child");
        let world = |id| DMat4::from_cols_array_2d(&baked_rig.rest_world(id).unwrap());
        world(b).transform_point3(glam::DVec3::ZERO) - world(a).transform_point3(glam::DVec3::ZERO)
    };
    let left = direction("NPC L UpperArm [LUar]", "NPC L Forearm [LLar]").normalize();
    let right = direction("NPC R UpperArm [RUar]", "NPC R Forearm [RLar]").normalize();
    assert!(
        left.x < -0.99,
        "the left arm must be lateral on our body: {left:?}"
    );
    assert!(right.x > 0.99, "the right arm must be lateral: {right:?}");
    assert!(
        left.y.abs() < 1e-6 && right.y.abs() < 1e-6,
        "arms not horizontal"
    );
    // Bone lengths survive the turn.
    let length = |joint: &str, child: &str| direction(joint, child).length();
    // The bone vector is the forearm's own authored offset: 0.02, −0.24, 0.
    let authored = (0.02_f64 * 0.02 + 0.24 * 0.24).sqrt();
    assert!((length("NPC L UpperArm [LUar]", "NPC L Forearm [LLar]") - authored).abs() < 1e-6);
    let _ = std::fs::remove_file(&out_path);
}

/// An A-posed body whose spine leans `degrees` sideways from the vertical.
///
/// Emily's measured lean is forward (`Hips → Head = (−0.000, +1.000, +0.030)`,
/// `1.723°` from vertical). The sideways shape is what makes the rule visible: a
/// lateral arm is no longer perpendicular to the tilted axis once the tilt has a
/// lateral component, so the world horizontal and the body's own T stop agreeing.
fn leaning_body(degrees: f32) -> Rig {
    let lean = 0.73 * degrees.to_radians().tan();
    let joints = a_pose_body()
        .joints()
        .iter()
        .map(|joint| {
            let mut joint = joint.clone();
            if joint.name.as_deref() == Some("NPC Head [Head]") {
                joint.rest.translation = [lean, 0.28, 0.0];
            }
            joint
        })
        .collect();
    Rig::new(joints, RigFrame::YUp).expect("a leaning body forms a rig")
}

#[test]
fn the_arms_are_squared_to_the_body_spine_not_to_the_world() {
    let lean = 1.72_f32;
    let body = leaning_body(lean);
    let reference = y_along_bones(&t_pose_reference());
    let (actor_path, reference_path) = write_pair("leaning", &body, &reference);
    let out_path = temp_path("leaning-out");

    let report = run(&TPoseParams {
        actor: actor_path,
        reference: reference_path,
        out: Some(out_path.clone()),
        report: None,
        pose: None,
        pose_clip: None,
        pose_time: None,
        pose_scan: false,
        pose_rest: None,
        straighten_arms: false,
    })
    .expect("the bake succeeds");

    // The axis the bake states is the body's own, and the shoulder line is
    // squared to it: aiming at the world horizontal would leave this arm `1.72°`
    // off perpendicular, which is the defect this bar exists to catch.
    let tilt = report["spine"]["tilt_from_vertical_deg"]
        .as_f64()
        .expect("a measured spine tilt");
    assert!((tilt - f64::from(lean)).abs() < 0.01, "spine tilt {tilt}°");
    assert_eq!(report["gates"]["arm_perpendicular"]["pass"], true);
    let perpendicular = report["gates"]["arm_perpendicular"]["max_deviation_deg"]
        .as_f64()
        .unwrap();
    assert!(perpendicular < 1e-6, "off perpendicular by {perpendicular}°");
    // The arms follow the lean instead of being flattened onto the world: the
    // world-horizontal figure is the lean itself, and inside its own bar.
    assert_eq!(report["gates"]["arm_horizontal"]["pass"], true);
    let horizontal = report["gates"]["arm_horizontal"]["max_below_horizontal_deg"]
        .as_f64()
        .unwrap();
    assert!(
        (horizontal - f64::from(lean)).abs() < 0.05,
        "arms {horizontal}° below horizontal, where the lean is {tilt}°"
    );

    // The artifact's own numbers, taken from its own Hips → Head axis.
    let out_bytes = std::fs::read(&out_path).expect("read output");
    let out = Actor::from_glb(&out_bytes).expect("output actor");
    let (baked_rig, _) = actor_rig(&out).expect("baked rig");
    let pivot = |joint: &str| {
        let id = baked_rig.joint_by_name(joint).expect("joint");
        DMat4::from_cols_array_2d(&baked_rig.rest_world(id).unwrap())
            .transform_point3(glam::DVec3::ZERO)
    };
    let axis = (pivot("NPC Head [Head]") - pivot("NPC Pelvis [Pelv]")).normalize();
    for (upper, child) in [
        ("NPC L UpperArm [LUar]", "NPC L Forearm [LLar]"),
        ("NPC R UpperArm [RUar]", "NPC R Forearm [RLar]"),
    ] {
        let arm = (pivot(child) - pivot(upper)).normalize();
        let off = 90.0 - axis.angle_between(arm).to_degrees();
        assert!(
            off.abs() < 1e-5,
            "{upper} sits {off}° off the spine's perpendicular"
        );
    }
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn a_reference_that_is_not_a_t_pose_is_refused() {    let mut reference = t_pose_reference();
    // Drop the arms 60° below the horizontal: an A-pose with another name.
    let joints: Vec<Joint> = (0..reference.joints().len())
        .map(|slot| {
            let joint = reference.joint(JointId::new(slot as u32)).expect("joint");
            let mut joint = joint.clone();
            if joint
                .name
                .as_deref()
                .is_some_and(|name| name.ends_with("Arm"))
            {
                joint.rest.translation[1] = -0.20;
                joint.rest.translation[0] = 0.15;
            }
            joint
        })
        .collect();
    reference = Rig::new(joints, RigFrame::YUp).expect("a drooping reference");
    let body = a_pose_body();
    let (actor_path, reference_path) = write_pair("refused", &body, &reference);
    let error = run(&TPoseParams {
        actor: actor_path,
        reference: reference_path,
        out: None,
        report: None,
        pose: None,
        pose_clip: None,
        pose_time: None,
        pose_scan: false,
        pose_rest: None,
        straighten_arms: false,
    })
    .expect_err("a drooping reference is not a T-pose");
    assert!(
        error.to_string().contains("not a T-pose"),
        "unexpected refusal: {error}"
    );
}

/// A T-pose reference whose elbow and wrist are bent in the horizontal plane.
///
/// A bone's direction is the offset to its child, so the elbow is the `Hand`
/// node's placement and the wrist the `HandMiddle1` node's. Every segment still
/// lies horizontal, so the verb accepts the reference as a T — and the aimed
/// chain keeps both bends, which is what the fall pose gives us and what the
/// straightening step exists to remove.
fn bent_arm_reference(degrees: f32) -> Rig {
    let direction = |angle: f32, length: f32, sign: f32| {
        let (sin, cos) = angle.to_radians().sin_cos();
        [sign * length * cos, 0.0, length * sin]
    };
    let joints: Vec<Joint> = t_pose_reference()
        .joints()
        .iter()
        .map(|joint| {
            let mut joint = joint.clone();
            let Some(name) = joint.name.as_deref() else {
                return joint;
            };
            let sign = match name.starts_with("mixamorig:Left") {
                true => 1.0,
                false => -1.0,
            };
            match name.ends_with("Hand") {
                true => joint.rest.translation = direction(degrees, 0.10, sign),
                false => {
                    if name.ends_with("HandMiddle1") {
                        joint.rest.translation = direction(2.0 * degrees, 0.05, sign);
                    }
                }
            }
            joint
        })
        .collect();
    y_along_bones(&Rig::new(joints, RigFrame::YUp).expect("a bent reference"))
}

#[test]
fn a_bent_elbow_is_straightened_about_its_own_hinge() {
    let body = a_pose_body();
    let reference = bent_arm_reference(20.0);
    let (actor_path, reference_path) = write_pair("straighten", &body, &reference);
    let out_path = temp_path("straighten-out");
    let run_once = |out: &PathBuf| {
        run(&TPoseParams {
            actor: actor_path.clone(),
            reference: reference_path.clone(),
            out: Some(out.clone()),
            report: None,
            pose: None,
            pose_clip: None,
            pose_time: None,
            pose_scan: false,
            pose_rest: None,
            straighten_arms: true,
        })
        .expect("the bake succeeds")
    };

    let report = run_once(&out_path);
    // The aggregate gate is graded on a real body (its stretch row needs flesh
    // that spans the chain); here every row the fixture can carry is checked.
    assert!(
        report["gates"]["straighten"]["edge_stretch_not_worse"].is_boolean(),
        "the stretch comparison is reported on every bake"
    );
    let rotations = report["straighten"]["rotations"]
        .as_array()
        .expect("a rotation ledger");
    assert_eq!(rotations.len(), 4, "two joints per side");
    for row in rotations {
        // The bend the aim left is 20° at both joints; the step removes all of
        // it, and reports the axis it turned about in world and in the bone.
        // The reference's own translations are f32, so the bend it carries is
        // 20° to a part per million rather than exactly.
        let before = row["angle_before_deg"].as_f64().unwrap();
        assert!((before - 20.0).abs() < 1e-5, "before {before}");
        assert!(row["angle_after_deg"].as_f64().unwrap() < 1e-5);
        assert!(row["residual_to_target_deg"].as_f64().unwrap() < 1e-5);
        assert!((row["rotation_deg"].as_f64().unwrap() + before).abs() < 1e-5);
        assert!(row["hinge_world"].is_array(), "the hinge is reported");
        assert!(row["hinge_local"].is_array());
        // The authored A-rest has a bent elbow and a straight wrist, so the rest
        // hinge is reported for the elbow and may be null for the hand.
        if row["joint"] == "Forearm" {
            assert!(
                row["rest_hinge_world"].is_array(),
                "the A-rest's elbow is bent, so it has a hinge"
            );
        }
    }
    // Nothing outside the arm subtrees may move, and the upper arm — the
    // shoulder line the pose gave us — must survive the step untouched.
    assert_eq!(
        report["straighten"]["outside_arm_subtree"]["max_rotation_deg"],
        0.0
    );
    assert_eq!(
        report["straighten"]["outside_arm_subtree"]["max_pivot_shift_m"],
        0.0
    );
    assert_eq!(report["straighten"]["upper_arm_max_delta_deg"], 0.0);
    assert_eq!(report["straighten"]["elbow_max_deg"], 0.0);
    assert_eq!(report["straighten"]["wrist_max_deg"], 0.0);

    // The same input writes the same bytes, twice.
    let first = std::fs::read(&out_path).expect("read the written actor");
    run_once(&out_path);
    let second = std::fs::read(&out_path).expect("read the written actor again");
    assert_eq!(first, second, "the same input must write the same bytes");
    let _ = std::fs::remove_file(&out_path);
}
