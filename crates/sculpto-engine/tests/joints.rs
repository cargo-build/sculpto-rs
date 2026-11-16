//! Joint articulation schema: parse, validate, and refuse with numbers.
//!
//! The schema is product data (`crates/sculpto-engine/src/joints.toml`); these
//! tests pin its completeness (65 Mixamo joints, every role resolvable), the
//! swing-twist math the validator uses, and the violation rows a backwards
//! knee or an over-flexed elbow must produce. @req DECL0013

use std::collections::BTreeSet;

use glam::{DMat4, DQuat, DVec3};
use sculpto_core::mesh::{MeshAsset, PartitionId, Skin};
use sculpto_core::rig::rest::{RestBinding, SkinRestSource};
use sculpto_core::rig::{Joint, JointId, LocalTransform, Rig, RigFrame};
use sculpto_core::skin::SkinWeights;
use sculpto_engine::joints::{
    self, Articulation, Basis, Severity, check_pose, schema, signed_twist,
};
use sculpto_engine::retarget::role_of;
use sculpto_gltf::pack::{PackWriter, Track};
use sculpto_gltf::{PartMaterial, write_partitioned_rigged_glb};
use sculpto_mesh::actor::{Actor, Pose};

const IDENTITY_F32: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[test]
fn schema_covers_all_sixty_five_mixamo_joints() {
    let schema = schema().expect("the shipped schema parses");
    assert_eq!(schema.len(), 65, "the Mixamo palette has 65 joints");
    let names: BTreeSet<_> = schema
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(names.len(), 65, "joint names are unique");
    for name in [
        "mixamorig:Hips",
        "mixamorig:Spine",
        "mixamorig:Spine1",
        "mixamorig:Spine2",
        "mixamorig:Neck",
        "mixamorig:Head",
        "mixamorig:HeadTop_End",
    ] {
        assert!(names.contains(name), "missing {name}");
    }
    for side in ["Left", "Right"] {
        for stem in ["Shoulder", "Arm", "ForeArm", "Hand"] {
            assert!(names.contains(&format!("mixamorig:{side}{stem}")));
        }
        for digit in ["Thumb", "Index", "Middle", "Ring", "Pinky"] {
            for segment in 1..=4 {
                assert!(names.contains(&format!("mixamorig:{side}Hand{digit}{segment}")));
            }
        }
        for stem in ["UpLeg", "Leg", "Foot", "ToeBase", "Toe_End"] {
            assert!(names.contains(&format!("mixamorig:{side}{stem}")));
        }
    }
}

#[test]
fn every_role_label_resolves_and_foreign_names_find_entries() {
    let schema = schema().expect("schema");
    for entry in &schema.entries {
        let role = role_of(&entry.name)
            .unwrap_or_else(|| panic!("{} carries no role", entry.name))
            .label();
        assert_eq!(
            role, entry.role,
            "{}: schema role disagrees with the retarget role resolver",
            entry.name
        );
    }
    for (foreign, mixamo) in [
        ("NPC L Calf [LCaf]", "mixamorig:LeftLeg"),
        ("NPC R Forearm [RLar]", "mixamorig:RightForeArm"),
        ("NPC Pelvis [Pelv]", "mixamorig:Hips"),
        ("NPC Spine1 [Spn1]", "mixamorig:Spine1"),
        ("L lower leg", "mixamorig:LeftLeg"),
    ] {
        assert_eq!(
            schema.entry_for(foreign).map(|entry| entry.name.as_str()),
            Some(mixamo),
            "{foreign} must resolve by role onto {mixamo}"
        );
    }
}

#[test]
fn hinge_families_carry_cited_ranges_and_the_pelvis_is_root() {
    let schema = schema().expect("schema");
    for name in ["mixamorig:LeftLeg", "mixamorig:RightLeg"] {
        let entry = schema.entry_for(name).expect("knee");
        assert_eq!(entry.articulation, Articulation::Hinge);
        let flexion = entry
            .axes
            .iter()
            .find(|axis| axis.name == "flexion")
            .expect("knee flexion axis");
        assert_eq!(flexion.basis, Basis::Anatomical);
        assert!((flexion.max_deg - 135.0).abs() < 1e-9, "AAOS knee 135");
    }
    for name in ["mixamorig:LeftForeArm", "mixamorig:RightForeArm"] {
        let entry = schema.entry_for(name).expect("elbow");
        let flexion = entry
            .axes
            .iter()
            .find(|axis| axis.name == "flexion")
            .expect("elbow flexion axis");
        assert!((flexion.max_deg - 150.0).abs() < 1e-9, "AAOS elbow 150");
        assert!(flexion.min_deg >= 0.0, "no hyperextension by default");
    }
    let hips = schema.entry_for("mixamorig:Hips").expect("hips");
    assert_eq!(hips.articulation, Articulation::Root);
    assert!(hips.axes.is_empty(), "the root invents no range");
    for entry in &schema.entries {
        if entry.note.contains("unresolved") {
            assert!(
                entry.note.contains("settle") || entry.note.contains("MOCAP"),
                "{} names an unresolved split without a settling measurement",
                entry.name
            );
        }
    }
    // Every listed axis is a unit vector and a finite, ordered range.
    for entry in &schema.entries {
        for axis in &entry.axes {
            assert!(
                (axis.axis.length() - 1.0).abs() < 1e-6,
                "{} axis",
                entry.name
            );
            assert!(
                (axis.anchor.length() - 1.0).abs() < 1e-6,
                "{} anchor",
                entry.name
            );
            assert!(axis.min_deg <= axis.max_deg);
        }
    }
}

#[test]
fn signed_twist_matches_manual_axis_angle() {
    let axis = DVec3::new(0.0, 0.0, 1.0);
    for degrees in [-170.0_f64, -90.0, -12.0, 0.0, 30.0, 150.0, 179.0] {
        let rotation = DQuat::from_axis_angle(axis, degrees.to_radians());
        let measured = signed_twist(rotation, axis);
        assert!(
            (measured - degrees).abs() < 1e-9,
            "{degrees} degrees measured as {measured}"
        );
        let antipode = DQuat::from_xyzw(-rotation.x, -rotation.y, -rotation.z, -rotation.w);
        assert!((signed_twist(antipode, axis) - degrees).abs() < 1e-9);
    }
    let swing = DQuat::from_axis_angle(DVec3::X, 1.2);
    assert!(signed_twist(swing, axis).abs() < 1e-9);
    let near_pi = DQuat::from_axis_angle(axis, 179.5_f64.to_radians());
    assert!((signed_twist(near_pi, axis) - 179.5).abs() < 1e-6);
}

// --- synthetic actor --------------------------------------------------------

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

/// A small Mixamo-palette actor: identity rest rotations (the bone frames equal
/// the world frame), one flesh triangle per bone, and one constant clip that
/// applies the requested local rotations.
fn actor_with_poses(poses: &[(&str, DQuat)]) -> Actor {
    let mut joints = vec![
        joint("mixamorig:Hips", None, [0.0, 1.0, 0.0]),
        joint("mixamorig:Spine", Some(0), [0.0, 0.1, 0.0]),
    ];
    let up_leg = joints.len() as u32;
    joints.push(joint("mixamorig:LeftUpLeg", Some(0), [0.1, -0.4, 0.0]));
    let leg = joints.len() as u32;
    joints.push(joint("mixamorig:LeftLeg", Some(up_leg), [0.0, -0.4, 0.0]));
    let foot = joints.len() as u32;
    joints.push(joint("mixamorig:LeftFoot", Some(leg), [0.0, -0.4, 0.0]));
    joints.push(joint(
        "mixamorig:LeftToeBase",
        Some(foot),
        [0.0, -0.05, 0.05],
    ));
    let arm = joints.len() as u32;
    joints.push(joint("mixamorig:LeftArm", Some(1), [0.3, 0.0, 0.0]));
    joints.push(joint("mixamorig:LeftForeArm", Some(arm), [0.3, 0.0, 0.0]));
    let rig = Rig::new(joints, RigFrame::YUp).expect("the rig forms");

    let count = rig.joints().len();
    let mut positions = Vec::new();
    let mut triangles = Vec::new();
    let mut offsets = vec![0_u32];
    let mut joint_ids = Vec::new();
    let mut values = Vec::new();
    for slot in 0..count {
        let world =
            DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(slot as u32)).expect("rest"));
        let centre = world.w_axis.truncate();
        let base = positions.len() as u32;
        for offset in [[0.01_f64, 0.0, 0.0], [0.0, 0.01, 0.0], [0.0, 0.0, 0.01]] {
            let point = centre + DVec3::from(offset);
            positions.push([point.x as f32, point.y as f32, point.z as f32]);
            // One influence row per vertex, weighted fully to this bone.
            let mut row = vec![0_u16; count];
            row[slot] = slot as u16;
            joint_ids.extend_from_slice(&row);
            values.extend((0..count).map(|index| (index == slot) as u8 as f32));
            offsets.push(joint_ids.len() as u32);
        }
        triangles.push([base, base + 1, base + 2]);
    }
    let skin = Skin {
        bones: rig
            .joints()
            .iter()
            .map(|j| j.name.clone().unwrap_or_default())
            .collect(),
        bind_inverse: (0..count)
            .map(|slot| {
                DMat4::from_cols_array_2d(&rig.rest_world(JointId::new(slot as u32)).expect("rest"))
                    .inverse()
                    .to_cols_array_2d()
                    .map(|column| column.map(|value| value as f32))
            })
            .collect(),
        influences: SkinWeights::new(offsets, joint_ids, values, count).expect("rows"),
    };
    let binding = RestBinding::prove_with_basis(
        &rig,
        RigFrame::YUp,
        &[SkinRestSource {
            shape: 0,
            root: rig.roots()[0],
            positions: &positions,
            skin: &skin,
            global_to_skin: IDENTITY_F32,
        }],
        DMat4::IDENTITY.to_cols_array_2d(),
    )
    .expect("a checked rest");
    let asset = MeshAsset {
        name: "probe".into(),
        positions: positions.clone(),
        triangles: triangles.clone(),
        triangle_partitions: vec![PartitionId(0); triangles.len()],
        skin: Some(skin.clone()),
        ..Default::default()
    };
    let materials = [PartMaterial {
        partition: 0,
        name: "body".into(),
        color: [0.8, 0.8, 0.8, 1.0],
    }];
    let glb = write_partitioned_rigged_glb(&asset, &binding, 0, &[], &materials).expect("written");

    // The clip keys glTF node indices, which the writer assigns; rig slots and
    // node indices differ by the leading mesh node, so resolve by name.
    let base = Actor::from_glb(&glb).expect("the base actor");
    if poses.is_empty() {
        return base;
    }
    let tracks = poses
        .iter()
        .map(|(name, rotation)| Track::Rotation {
            node: base
                .document()
                .nodes()
                .find(|node| node.name() == Some(*name))
                .expect("the posed node exists")
                .index(),
            values: vec![rotation.to_array().map(|value| value as f32); 2],
        })
        .collect::<Vec<_>>();
    let mut writer = PackWriter::from_glb(&glb).expect("pack");
    writer
        .insert_animation(1, "Walk", &[0.0, 1.0], &tracks)
        .expect("insert");
    let posed = writer.finish().expect("finish");
    Actor::from_glb(&posed).expect("the posed actor")
}

/// The schema's world anchor for one axis: on this synthetic rig (identity rest
/// rotations) the anchor is exactly the bone-frame axis the validator uses.
fn anchor_axis(joint_name: &str, axis_name: &str) -> DVec3 {
    schema()
        .expect("schema")
        .entry_for(joint_name)
        .expect("joint")
        .axes
        .iter()
        .find(|axis| axis.name == axis_name)
        .expect("axis")
        .anchor
}

#[test]
fn backwards_knee_is_a_violation_with_numbers() {
    let axis = anchor_axis("mixamorig:LeftLeg", "flexion");
    let actor = actor_with_poses(&[(
        "mixamorig:LeftLeg",
        DQuat::from_axis_angle(axis, -12.0_f64.to_radians()),
    )]);
    let check = check_pose(
        &actor,
        Pose::Clip {
            name: "Walk",
            time: 0.0,
        },
    )
    .expect("check");
    let row = check
        .rows
        .iter()
        .find(|row| row.joint == "mixamorig:LeftLeg" && row.axis == "flexion")
        .expect("the knee flexion row");
    assert_eq!(row.severity, Severity::Violation);
    assert!(
        (row.value_deg - (-12.0)).abs() < 0.5,
        "value {} should read about -12",
        row.value_deg
    );
    let expected_excess = row.min_deg - (-12.0);
    assert!(
        (row.excess_deg - expected_excess).abs() < 0.5,
        "excess {} should be min {} minus the value (about {expected_excess})",
        row.excess_deg,
        row.min_deg
    );
}

#[test]
fn overflexed_elbow_reports_excess_over_150() {
    let axis = anchor_axis("mixamorig:LeftForeArm", "flexion");
    let actor = actor_with_poses(&[(
        "mixamorig:LeftForeArm",
        DQuat::from_axis_angle(axis, 170.0_f64.to_radians()),
    )]);
    let check = check_pose(
        &actor,
        Pose::Clip {
            name: "Walk",
            time: 0.0,
        },
    )
    .expect("check");
    let row = check
        .rows
        .iter()
        .find(|row| row.joint == "mixamorig:LeftForeArm" && row.axis == "flexion")
        .expect("the elbow flexion row");
    assert_eq!(row.severity, Severity::Violation);
    assert!(
        (row.excess_deg - 20.0).abs() < 0.6,
        "excess {} should be about 20",
        row.excess_deg
    );
}

#[test]
fn healthy_flexion_inside_range_is_ok_and_visible() {
    let axis = anchor_axis("mixamorig:LeftLeg", "flexion");
    let actor = actor_with_poses(&[(
        "mixamorig:LeftLeg",
        DQuat::from_axis_angle(axis, 60.0_f64.to_radians()),
    )]);
    let check = check_pose(
        &actor,
        Pose::Clip {
            name: "Walk",
            time: 0.0,
        },
    )
    .expect("check");
    let row = check
        .rows
        .iter()
        .find(|row| row.joint == "mixamorig:LeftLeg" && row.axis == "flexion")
        .expect("the knee flexion row");
    assert_eq!(row.severity, Severity::Ok);
    assert_eq!(row.excess_deg, 0.0);
}

#[test]
fn off_axis_swing_at_a_hinge_is_reported() {
    // 30 degrees about an axis perpendicular to the knee's flexion axis: the
    // flexion value stays small, the off-axis residual must carry the report.
    let axis = anchor_axis("mixamorig:LeftLeg", "flexion");
    let off = axis.cross(DVec3::Y).normalize();
    let actor = actor_with_poses(&[(
        "mixamorig:LeftLeg",
        DQuat::from_axis_angle(off, 30.0_f64.to_radians()),
    )]);
    let check = check_pose(
        &actor,
        Pose::Clip {
            name: "Walk",
            time: 0.0,
        },
    )
    .expect("check");
    let residual = check
        .residuals
        .iter()
        .find(|row| row.joint == "mixamorig:LeftLeg")
        .expect("the knee residual row");
    assert_eq!(residual.severity, Severity::Violation, "30 deg off-axis");
    assert!(
        (residual.residual_deg - 30.0).abs() < 0.6,
        "residual {} should be about 30",
        residual.residual_deg
    );
}

#[test]
fn root_rotation_is_info_never_violation() {
    let actor = actor_with_poses(&[("mixamorig:Hips", DQuat::from_axis_angle(DVec3::Y, 0.9))]);
    let check = check_pose(
        &actor,
        Pose::Clip {
            name: "Walk",
            time: 0.0,
        },
    )
    .expect("check");
    let hips = check
        .rows
        .iter()
        .find(|row| row.joint == "mixamorig:Hips")
        .expect("the hips row exists");
    assert_eq!(hips.severity, Severity::Info);
    assert!(
        !check
            .rows
            .iter()
            .any(|row| row.joint == "mixamorig:Hips" && row.severity == Severity::Violation)
    );
}

#[test]
fn clamp_table_names_the_actor_nodes_and_ranges() {
    let actor = actor_with_poses(&[]);
    let table = joints::clamp_table(&actor).expect("clamps");
    let knee = table
        .iter()
        .find(|row| row["joint"] == "mixamorig:LeftLeg")
        .expect("the knee clamp row");
    assert_eq!(knee["node_name"], "mixamorig:LeftLeg");
    assert!(!knee["axes"].as_array().unwrap().is_empty());
    let flexion = &knee["axes"][0];
    assert!(flexion["min_deg"].as_f64().unwrap() < 0.0);
    assert!(flexion["max_deg"].as_f64().unwrap() > 100.0);
    let rest = flexion["axis_local"].as_array().unwrap();
    let length: f64 = rest
        .iter()
        .map(|value| value.as_f64().unwrap().powi(2))
        .sum::<f64>()
        .sqrt();
    assert!((length - 1.0).abs() < 1e-6, "the clamp axis is unit");
}
