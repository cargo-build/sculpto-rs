#[path = "../../sculpto-mesh/tests/common/mod.rs"]
mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn rig_run(input: &Path, output: &Path, parts: &[&str], joints: &[&str], pose: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sculpto"));
    command.args(["actor", "rig", "--input"]).arg(input);
    parts.iter().for_each(|part| {
        command.arg("--part").arg(part);
    });
    joints.iter().for_each(|joint| {
        command.arg("--joint").arg(joint);
    });
    command.arg("--out").arg(output).args(pose);
    command.output().unwrap()
}
/// The shared branching graph: Root connects Spine/Hand/Fingers and a Toe
/// through an unweighted connector; the body is skinned onto the fingers.
fn branching() -> common::Fixture {
    let mut fixture = common::Fixture::triangle();
    fixture.json["nodes"] = serde_json::json!([
        {"name": "Root", "translation": [10., 0., 0.], "children": [1, 6, 5]},
        {"name": "Spine", "translation": [0., 1., 0.], "children": [2]},
        {"name": "Hand", "translation": [0., 2., 0.], "children": [3, 4]},
        {"name": "FingerL", "translation": [0., 0., 3.]},
        {"name": "FingerR", "translation": [0., 0., 4.]},
        {"name": "Body", "mesh": 0, "skin": 0, "translation": [99., 0., 0.]},
        {"name": "Bridge", "translation": [0., 5., 0.], "children": [7]},
        {"name": "Toe", "translation": [0., 0., 7.]},
    ]);
    fixture.json["skins"] = serde_json::json!([{"joints": [3, 4]}]);
    let joints = fixture.accessor(&[0, 1, 0, 0].repeat(3), 5121, "VEC4", 3);
    let weights = fixture.floats(&[0.25, 0.75, 0., 0.].repeat(3), "VEC4", 3);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"] =
        serde_json::json!(joints);
    fixture.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] =
        serde_json::json!(weights);
    fixture
}

#[test]
fn actor_rig_reports_graph_influences_and_argument_errors() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("sculpto-actor-rig-{}-{stamp}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("actor.glb");
    let output = root.join("rig.json");
    let mut fixture = branching();
    fixture.channel(2, "translation", &[0., 1.], &[0., 2., 0., 1., 2., 0.]);
    fs::write(&input, fixture.bytes()).unwrap();
    let result = rig_run(&input, &output, &["Body"], &["Hand", "Toe"], &["--rest"]);
    assert!(result.status.success(), "{result:?}");
    let report: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(report["coordinates"], "glTF Y-up world space");
    assert_eq!(report["pose"]["kind"], "rest");
    assert_eq!(report["vertex_count"], 3);
    assert_eq!(report["parts"][0]["name"], "Body");
    assert_eq!(report["parts"][0]["node"], 5);
    assert_eq!(report["parts"][0]["vertex_start"], 0);
    assert_eq!(report["parts"][0]["vertex_count"], 3);
    assert_eq!(report["vertices"][0].as_array().unwrap().len(), 3);
    assert_eq!(report["triangles"][0], serde_json::json!([0, 1, 2]));
    let rig: Vec<&str> = report["rig"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    assert_eq!(rig, ["Root", "Spine", "Bridge", "Hand", "Toe"]);
    let roles: Vec<&str> = report["rig"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        [
            "connector",
            "connector",
            "connector",
            "requested",
            "requested"
        ],
        "with only Hand and Toe requested, Spine stays a path connector"
    );
    let hand_world: Vec<f64> = report["rig"][3]["world_column_major"][3]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    assert_eq!(hand_world, [10., 3., 0., 1.]);
    assert_eq!(report["requested"][0]["name"], "Hand");
    assert_eq!(report["requested"][0]["node"], 2);
    assert_eq!(report["requested"][0]["rig"], 3);
    assert_eq!(report["requested"][1]["name"], "Toe");
    let binding = &report["influences"][0]["bindings"];
    assert_eq!(
        *binding,
        serde_json::json!([{"rig": 3, "weight": 1.0}]),
        "both finger influences collapse onto the retained Hand, weights summed"
    );
    let run = rig_run(
        &input,
        &output,
        &["Body"],
        &["Hand", "Toe"],
        &["--clip", "Run", "--time", "0.5"],
    );
    assert!(run.status.success(), "{run:?}");
    let animated: serde_json::Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    let animated_hand: Vec<f64> = animated["rig"][3]["world_column_major"][3]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    assert_eq!(animated_hand, [10.5, 3., 0., 1.]);
    assert_eq!(animated["pose"]["kind"], "clip");
    let absent = root.join("absent.json");
    [
        vec!["--clip", "missing", "--time", "0"],
        vec!["--clip", "Run", "--time", "2"],
    ]
    .into_iter()
    .for_each(|pose| {
        assert!(
            !rig_run(&input, &absent, &["Body"], &["Hand"], &pose)
                .status
                .success()
        );
        assert!(!absent.exists());
    });
    assert_eq!(
        rig_run(&input, &absent, &["Body"], &["Hand", "Hand"], &["--rest"])
            .status
            .code(),
        Some(1)
    );
    assert!(!absent.exists());
    [
        vec!["--rest", "--clip", "Run", "--time", "0"],
        vec!["--clip", "Run", "--time", "NaN"],
        vec!["--rest", "--rest"],
    ]
    .into_iter()
    .for_each(|pose| {
        assert_eq!(
            rig_run(&input, &absent, &["Body"], &["Hand"], &pose)
                .status
                .code(),
            Some(2),
            "argument errors use exit two"
        );
        assert!(!absent.exists());
    });
    assert_eq!(
        rig_run(&input, &absent, &[], &["Hand"], &["--rest"])
            .status
            .code(),
        Some(2)
    );
    #[cfg(target_os = "linux")]
    {
        let result = rig_run(
            &input,
            Path::new("/dev/full"),
            &["Body"],
            &["Hand"],
            &["--rest"],
        );
        assert!(!result.status.success());
        assert!(
            String::from_utf8(result.stderr)
                .unwrap()
                .contains("/dev/full")
        );
    }
    let mut classic = common::Fixture::triangle();
    classic.skin();
    classic.json["nodes"][1]["name"] = serde_json::json!("Joint");
    classic.json["nodes"][3]["name"] = serde_json::json!("Tip");
    let classic_input = root.join("classic.glb");
    fs::write(&classic_input, classic.bytes()).unwrap();
    let result = rig_run(&classic_input, &output, &["Body"], &["Joint"], &["--rest"]);
    assert!(result.status.success(), "{result:?}");
    let classic_report: serde_json::Value =
        serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(classic_report["rig"][0]["name"], "Joint");
    assert_eq!(
        classic_report["influences"][0]["bindings"],
        serde_json::json!([{"rig": 0, "weight": 1.0}]),
        "the classic shared skin fixture collapses onto one retained joint"
    );
    fs::remove_dir_all(root).unwrap();
}
