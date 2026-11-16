#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use common::Fixture;
use sculpto_mesh::actor::{Actor, Axis, JointPose, Pose, probe, write_probe_clip};
use serde_json::json;

/// A two-joint chain with a triangle rigidly weighted to the child joint J1.
fn pose_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [0., 0., 0.], "children": [1]},
        {"name": "J0", "translation": [0., 0., 0.], "children": [2]},
        {"name": "J1", "translation": [1., 0., 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1, 2]}]);
    // Triangle one unit away from the J1 pivot, fully weighted to J1 (skin lane 1).
    let positions = fixture.floats(&[1., 0., 0., 2., 0., 0., 1., 1., 0.], "VEC3", 3);
    fixture.json["accessors"][positions]["min"] = json!([1., 0., 0.]);
    fixture.json["accessors"][positions]["max"] = json!([2., 1., 0.]);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
    let joints = fixture.accessor(&[1, 0, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[1., 0., 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    fixture
}

#[test]
fn probe_rotates_only_the_named_joint_and_reports_world_displacement() {
    let actor = Actor::from_glb(&pose_fixture().bytes()).unwrap();
    let outcome = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "J1",
            axis: Axis::Z,
            angle_deg: 90.0,
        }],
    )
    .unwrap();
    let joint = &outcome.probes[0];
    assert_eq!(joint.name, "J1");
    assert_eq!(joint.moved_vertices, 3);
    // Identity inverse binds leave the vertex two units from the J1 pivot, so a
    // quarter turn moves the farthest vertex 2*sqrt(2) units.
    assert!((joint.max_displacement - 2.0 * 2.0_f64.sqrt()).abs() < 1e-9);
    assert_eq!(joint.world_pivot, [1.0, 0.0, 0.0]);
    // The applied delta is a quarter turn about Z.
    let delta = joint.delta_quaternion;
    assert!((delta[2] - (45.0_f64).to_radians().sin()).abs() < 1e-9);
    assert!((delta[3] - (45.0_f64).to_radians().cos()).abs() < 1e-9);
}

#[test]
fn zero_angle_probe_leaves_geometry_at_rest() {
    let actor = Actor::from_glb(&pose_fixture().bytes()).unwrap();
    let outcome = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "J1",
            axis: Axis::X,
            angle_deg: 0.0,
        }],
    )
    .unwrap();
    assert_eq!(outcome.probes[0].moved_vertices, 0);
    assert!(outcome.probes[0].max_displacement < 1e-6);
}

/// The standard palette's hip joint rests under a −90° X rotation, so its local
/// `y` is the world's horizontal `−z`. A joint like that is the reason the probe
/// axis names the world axis: a yaw written as a local `y` would roll the body.
fn rolled_pelvis() -> Fixture {
    let mut fixture = Fixture::triangle();
    let half = std::f32::consts::FRAC_1_SQRT_2;
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [0., 0., 0.], "children": [1]},
        {
            "name": "Pelvis",
            "translation": [0., 0., 0.],
            "rotation": [-half, 0., 0., half],
            "children": [2],
        },
        {"name": "Spine", "translation": [0., 0.5, 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1, 2]}]);
    // Three vertices above the spine pivot in the bone's own frame, fully
    // weighted to the spine (skin lane 1), so a world yaw swings them.
    let positions = fixture.floats(&[0.2, 0.5, 0.1, 0.3, 0.5, -0.1, 0.2, 0.7, 0.0], "VEC3", 3);
    fixture.json["accessors"][positions]["min"] = json!([0.2, 0.5, -0.1]);
    fixture.json["accessors"][positions]["max"] = json!([0.3, 0.7, 0.1]);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
    let joints = fixture.accessor(&[1, 0, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[1., 0., 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] = json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    fixture
}

#[test]
fn the_probe_axis_is_the_world_axis_through_the_joint() {
    let actor = Actor::from_glb(&rolled_pelvis().bytes()).unwrap();
    let outcome = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "Spine",
            axis: Axis::Y,
            angle_deg: 90.0,
        }],
    )
    .unwrap();
    let joint = &outcome.probes[0];
    assert_eq!(joint.moved_vertices, 3);
    assert_eq!(joint.axis_world, [0.0, 1.0, 0.0]);
    // Local +Z is world +Y under the pelvis's −90° X rest, and the probe reports
    // the axis it actually wrote instead of pretending the two frames agree.
    assert!(
        (joint.axis_local[2] - 1.0).abs() < 1e-9,
        "{:?}",
        joint.axis_local
    );
    assert!(joint.axis_local[0].abs() < 1e-9, "{:?}", joint.axis_local);
    // A rotation about the world's vertical keeps every height: this is a yaw,
    // where the same letter read as a local axis would have rolled the figure.
    assert!(
        (joint.farthest_rest[1] - joint.farthest_posed[1]).abs() < 1e-9,
        "world yaw changed the height: {:?} -> {:?}",
        joint.farthest_rest,
        joint.farthest_posed
    );
}

#[test]
fn a_world_axis_letter_is_not_the_joint_local_axis() {
    let actor = Actor::from_glb(&rolled_pelvis().bytes()).unwrap();
    let yaw = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "Spine",
            axis: Axis::Y,
            angle_deg: 90.0,
        }],
    )
    .unwrap();
    let tilt = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "Spine",
            axis: Axis::Z,
            angle_deg: 90.0,
        }],
    )
    .unwrap();
    let height_change = |outcome: &sculpto_mesh::actor::PoseOutcome| {
        (outcome.probes[0].farthest_rest[1] - outcome.probes[0].farthest_posed[1]).abs()
    };
    assert!(height_change(&yaw) < 1e-9, "`y` is the yaw");
    assert!(
        height_change(&tilt) > 1e-3,
        "`z` is the world's horizontal axis, so it tips the figure"
    );
}

#[test]
fn written_probe_clip_reproduces_the_measured_pose() {
    let bytes = pose_fixture().bytes();
    let actor = Actor::from_glb(&bytes).unwrap();
    let requests = [JointPose {
        name: "J1",
        axis: Axis::Z,
        angle_deg: 90.0,
    }];
    let outcome = probe(&actor, &["Body"], &requests).unwrap();
    let (glb, index) = write_probe_clip(&bytes, &outcome, "Pose").unwrap();
    assert_eq!(index, 0); // no prior clip, so the probe lands at the first slot
    let written = Actor::from_glb(&glb).unwrap();
    assert!(written.clip_names().contains(&Some("Pose")));
    let rest = written.sample(&["Body"], Pose::Rest).unwrap();
    let posed = written
        .sample(
            &["Body"],
            Pose::Clip {
                name: "Pose",
                time: 0.0,
            },
        )
        .unwrap();
    // The posed sample matches the probe's farthest-vertex displacement.
    let displacement = rest
        .vertices
        .iter()
        .zip(&posed.vertices)
        .map(|(a, b)| {
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        })
        .fold(0.0_f64, f64::max);
    assert!((displacement - outcome.probes[0].max_displacement).abs() < 1e-6);
}

#[test]
fn probe_refuses_missing_duplicate_and_matrix_joints() {
    let actor = Actor::from_glb(&pose_fixture().bytes()).unwrap();
    let missing = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "Nope",
            axis: Axis::X,
            angle_deg: 10.0,
        }],
    );
    assert!(missing.is_err());
    let duplicate = probe(
        &actor,
        &["Body"],
        &[
            JointPose {
                name: "J1",
                axis: Axis::X,
                angle_deg: 10.0,
            },
            JointPose {
                name: "J1",
                axis: Axis::X,
                angle_deg: 20.0,
            },
        ],
    );
    assert!(duplicate.is_err());
    let mut matrix = pose_fixture();
    matrix.json["nodes"][2] = json!({"name": "J1", "matrix": [
        1.,0.,0.,0., 0.,1.,0.,0., 0.,0.,1.,0., 1.,0.,0.,1.
    ]});
    let actor = Actor::from_glb(&matrix.bytes()).unwrap();
    let refusal = probe(
        &actor,
        &["Body"],
        &[JointPose {
            name: "J1",
            axis: Axis::X,
            angle_deg: 10.0,
        }],
    );
    assert!(refusal.is_err());
}
