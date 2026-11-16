#[path = "../../sculpto-mesh/tests/common/mod.rs"]
#[allow(dead_code)] // the shared builder carries helpers these tests do not use
mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use common::Fixture;
use serde_json::{Value, json};

fn temp_root(tag: &str) -> std::path::PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sculpto-{tag}-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    root
}

fn sculpto(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sculpto"))
        .args(args)
        .output()
        .unwrap()
}

fn pose_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Root", "translation": [0., 0., 0.], "children": [1]},
        {"name": "J0", "translation": [0., 0., 0.], "children": [2]},
        {"name": "J1", "translation": [1., 0., 0.]},
        {"name": "Body", "mesh": 0, "skin": 0},
    ]);
    fixture.json["skins"] = json!([{"joints": [1, 2]}]);
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

fn gait_fixture() -> Fixture {
    let mut fixture = Fixture::triangle();
    fixture.json["nodes"] = json!([
        {"name": "Scene Root", "children": [1]},
        {"name": "NPC COM [COM ]", "translation": [0., 0.96, 0.], "children": [2]},
        {
            "name": "NPC Pelvis [Pelv]",
            "translation": [0., 0., 0.],
            "children": [3, 7, 11, 13, 15],
        },
        {"name": "NPC L Thigh [LThg]", "translation": [-0.1, 0., 0.], "children": [4]},
        {"name": "NPC L Calf [LClf]", "translation": [0., -0.45, 0.], "children": [5]},
        {"name": "NPC L Foot [Lft ]", "translation": [0., -0.4, 0.], "children": [6]},
        {"name": "NPC L Toe0 [LToe]", "translation": [0., -0.05, 0.12]},
        {"name": "NPC R Thigh [RThg]", "translation": [0.1, 0., 0.], "children": [8]},
        {"name": "NPC R Calf [RClf]", "translation": [0., -0.45, 0.], "children": [9]},
        {"name": "NPC R Foot [Rft ]", "translation": [0., -0.4, 0.], "children": [10]},
        {"name": "NPC R Toe0 [RToe]", "translation": [0., -0.05, 0.12]},
        {"name": "NPC L UpperArm [LUar]", "translation": [-0.2, 0.5, 0.], "children": [12]},
        {"name": "NPC L Forearm [LLar]", "translation": [0., -0.3, 0.]},
        {"name": "NPC R UpperArm [RUar]", "translation": [0.2, 0.5, 0.], "children": [14]},
        {"name": "NPC R Forearm [RLar]", "translation": [0., -0.3, 0.]},
        {"name": "NPC Spine [Spn0]", "translation": [0., 0.2, 0.], "children": [16]},
        {"name": "NPC Spine2 [Spn2]", "translation": [0., 0.2, 0.], "children": [17]},
        {"name": "NPC Head [Head]", "translation": [0., 0.1, 0.]},
    ]);
    fixture.json["meshes"] = json!([]);
    fixture
}

#[test]
fn actor_pose_writes_a_probe_clip_report_and_obj() {
    let root = temp_root("pose");
    let input = root.join("actor.glb");
    let output = root.join("posed.glb");
    let report = root.join("posed.json");
    let obj = root.join("posed.obj");
    fs::write(&input, pose_fixture().bytes()).unwrap();
    let result = sculpto(&[
        "actor",
        "pose",
        "--input",
        input.to_str().unwrap(),
        "--joint",
        "J1",
        "--angle",
        "90",
        "--axis",
        "z",
        "--out",
        output.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--obj",
        obj.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "{result:?}");
    let metadata: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(metadata["verb"], "actor pose");
    assert_eq!(metadata["clip"]["name"], "Pose");
    let joint = &metadata["joints"][0];
    assert_eq!(joint["name"], "J1");
    assert_eq!(joint["axis_frame"], "world");
    assert_eq!(joint["moved_vertices"], 3);
    let displacement = joint["max_displacement_m"].as_f64().unwrap();
    assert!((displacement - 2.0 * 2.0_f64.sqrt()).abs() < 1e-6);
    // The posed OBJ carries the rotated geometry.
    assert!(fs::read_to_string(&obj).unwrap().contains("v "));
    // The written GLB re-parses and carries the probe clip.
    let actor = sculpto_mesh::actor::Actor::from_glb(&fs::read(&output).unwrap()).unwrap();
    assert!(actor.clip_names().contains(&Some("Pose")));
}

#[test]
fn actor_pose_requires_an_angle_per_joint() {
    let root = temp_root("pose-err");
    let input = root.join("actor.glb");
    let output = root.join("posed.glb");
    fs::write(&input, pose_fixture().bytes()).unwrap();
    let result = sculpto(&[
        "actor",
        "pose",
        "--input",
        input.to_str().unwrap(),
        "--joint",
        "J1",
        "--out",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--angle"));
}

#[test]
fn actor_gait_writes_a_symmetric_cycle_and_report() {
    let root = temp_root("gait");
    let input = root.join("actor.glb");
    let output = root.join("gait.glb");
    let report = root.join("gait.json");
    fs::write(&input, gait_fixture().bytes()).unwrap();
    let result = sculpto(&[
        "actor",
        "gait",
        "--input",
        input.to_str().unwrap(),
        "--cycle",
        "1.0",
        "--samples",
        "29",
        "--bob",
        "0.03",
        "--out",
        output.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "{result:?}");
    let metadata: Value = serde_json::from_slice(&fs::read(&report).unwrap()).unwrap();
    assert_eq!(metadata["verb"], "actor gait");
    assert_eq!(metadata["clip"]["name"], "Gait");
    let measures = &metadata["measures"];
    let left = measures["foot_lift_left_m"].as_f64().unwrap();
    let right = measures["foot_lift_right_m"].as_f64().unwrap();
    assert!((left - right).abs() / left.max(right) < 0.02);
    let bob = measures["pelvis_vertical_range_m"].as_f64().unwrap();
    assert!((bob - 0.03).abs() < 1e-6);
    assert!(measures["arm_swing_left_deg_pp"].as_f64().unwrap() > 10.0);
    // The pelvis yaw reaches the chest and the head counter holds the face.
    assert!(measures["chest_yaw_deg_pp"].as_f64().unwrap() > 5.0);
    assert!(measures["head_yaw_deg_pp"].as_f64().unwrap() < 0.5);
    // The written GLB carries the gait clip at the walk slot.
    let actor = sculpto_mesh::actor::Actor::from_glb(&fs::read(&output).unwrap()).unwrap();
    assert!(actor.clip_names().contains(&Some("Gait")));
}

#[test]
fn actor_gait_refuses_an_actor_without_the_standard_palette() {
    let root = temp_root("gait-err");
    let input = root.join("actor.glb");
    let output = root.join("gait.glb");
    fs::write(&input, pose_fixture().bytes()).unwrap();
    let result = sculpto(&[
        "actor",
        "gait",
        "--input",
        input.to_str().unwrap(),
        "--cycle",
        "1.0",
        "--samples",
        "29",
        "--out",
        output.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(!Path::new(&output).exists());
}
